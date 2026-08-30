use crate::models::AccountProfile;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    io::Write,
    path::{Path, PathBuf},
};
use tokio::sync::Mutex;

const HISTORY_FILE: &str = "history.jsonl";
const DAY_MS: i64 = 86_400_000;
const WEEK_MS: i64 = 7 * DAY_MS;
/// Most buckets the activity overview may hold before coarsening day -> week
/// -> month.
const MAX_ACTIVITY_BUCKETS: i64 = 120;
const RATING_BAND_WIDTH: i64 = 200;
const MAX_RATING_POINTS: usize = 500;
const MAX_TOP_OPPONENTS: usize = 10;
const MAX_OPENINGS: usize = 8;
/// Eval snapshot (our perspective) at or above which a game counts as winning.
const LAB_WINNING_EVAL_CP: i32 = 300;
/// Eval snapshot at or below which a game counts as (nearly) lost.
const LAB_LOSING_EVAL_CP: i32 = -300;
/// Final eval at or above which a time loss counts as "flagged while winning".
const LAB_FLAG_EVAL_CP: i32 = 150;
/// Eval drop between consecutive snapshots that counts as a blunder.
const BLUNDER_DROP_CP: i32 = 200;
const MAX_LAB_GAMES: usize = 10;
const MAX_CONFIG_LINES: usize = 8;

/// Fixed presentation order for real-time Lichess rating pools.
pub(crate) const PERF_ORDER: [&str; 5] = ["ultraBullet", "bullet", "blitz", "rapid", "classical"];

/// Maps a real-time clock to the Lichess rating pool it plays in, using the
/// same estimated-duration formula Lichess documents (limit + 40 * increment).
pub fn perf_key_for_clock(limit: u32, increment: u32) -> &'static str {
    let estimated_seconds = limit.saturating_add(increment.saturating_mul(40));
    if estimated_seconds < 30 {
        "ultraBullet"
    } else if estimated_seconds < 180 {
        "bullet"
    } else if estimated_seconds < 480 {
        "blitz"
    } else if estimated_seconds < 1500 {
        "rapid"
    } else {
        "classical"
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct GameRecord {
    pub id: String,
    pub account_id: String,
    pub account_username: String,
    pub engine_id: Option<String>,
    pub engine_name: Option<String>,
    pub opponent: String,
    pub opponent_rating: Option<i64>,
    pub our_rating: Option<i64>,
    /// "white" | "black"
    pub color: String,
    /// "win" | "draw" | "loss"
    pub result: String,
    /// Lichess terminal status: mate/resign/outoftime/timeout/draw/stalemate/…
    pub status: String,
    pub rated: bool,
    /// Seconds.
    pub clock_limit: Option<i64>,
    pub clock_increment: Option<i64>,
    /// ultraBullet/bullet/blitz/rapid/classical (or the Lichess speed key).
    pub perf: String,
    pub moves_count: i64,
    pub finished_at_ms: i64,
    /// "queenui" | "import"
    pub source: String,
    pub opening: Option<String>,
    /// Engine-side telemetry captured while QueenUI played the game.
    /// Always None for imported games and for records written before phase 2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telemetry: Option<GameTelemetry>,
}

/// Engine-side observations for one QueenUI-played game. Lichess never sees
/// any of this; it exists purely for operator insight.
#[derive(Clone, Debug, Default, Deserialize, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct GameTelemetry {
    /// Our-perspective eval snapshot at each of our move submissions, clamped
    /// to [-1000, 1000]; mate scores map to +/-1000. Book moves (and searches
    /// without a score) repeat the previous entry, or 0 when none exists yet,
    /// so the series stays aligned one entry per submitted move.
    pub eval_series_cp: Vec<i32>,
    pub avg_depth: Option<f64>,
    pub min_depth: Option<i64>,
    pub avg_move_time_ms: Option<f64>,
    pub max_move_time_ms: Option<i64>,
    /// Our clock (milliseconds) when the game ended.
    pub end_clock_ms: Option<i64>,
    /// How many of our moves came from the opening book.
    pub book_plies: i64,
    pub engine_restarts: i64,
    pub submission_retries: i64,
    pub stream_reconnects: i64,
    /// Searches QueenUI interrupted only because the active clock had reached
    /// its last-resort flag-safety margin.
    #[serde(default)]
    pub flag_safety_stops: i64,
    /// We resigned because the engine failed and could not be recovered.
    pub failure_resign: bool,
    pub max_eval_cp: Option<i32>,
    pub min_eval_cp: Option<i32>,
    /// Drops >= 200cp between consecutive entries of eval_series_cp.
    pub blunders: i64,
    /// Stable short hex hash of engine id + sorted UCI option values + book
    /// configuration, for grouping games played under the same setup.
    pub config_fingerprint: Option<String>,
}

/// Counts drops of at least BLUNDER_DROP_CP between consecutive eval snapshots.
pub fn count_blunders(series: &[i32]) -> i64 {
    series
        .windows(2)
        .filter(|pair| pair[0].saturating_sub(pair[1]) >= BLUNDER_DROP_CP)
        .count() as i64
}

/// Maps a PGN-style result string plus our color to a Scorebook outcome.
/// Returns None for "*" (aborted/noStart) and anything unrecognized.
pub fn result_from_pgn(result: &str, color: &str) -> Option<&'static str> {
    match (result, color) {
        ("1-0", "white") | ("0-1", "black") => Some("win"),
        ("1-0", "black") | ("0-1", "white") => Some("loss"),
        ("1/2-1/2", _) => Some("draw"),
        _ => None,
    }
}

fn player_matches(player: Option<&Value>, account: &AccountProfile) -> bool {
    let Some(user) = player.and_then(|player| player.get("user")) else {
        return false;
    };
    ["id", "name"]
        .iter()
        .filter_map(|key| user.get(key).and_then(Value::as_str))
        .any(|value| {
            value.eq_ignore_ascii_case(&account.id) || value.eq_ignore_ascii_case(&account.username)
        })
}

fn export_player_name(player: Option<&Value>) -> String {
    player
        .and_then(|player| player.get("user"))
        .and_then(|user| user.get("name").or_else(|| user.get("id")))
        .and_then(Value::as_str)
        .unwrap_or("Anonymous")
        .to_string()
}

fn export_player_rating(player: Option<&Value>) -> Option<i64> {
    player?.get("rating")?.as_i64()
}

/// Maps one game object from the Lichess games-export NDJSON stream to a
/// GameRecord for `account`. Returns None for games that must not be recorded:
/// unfinished/aborted games, games the account did not play in, and games
/// whose result cannot be determined.
pub fn record_from_lichess_export(game: &Value, account: &AccountProfile) -> Option<GameRecord> {
    let id = game.get("id").and_then(Value::as_str)?.to_string();
    let status = game.get("status").and_then(Value::as_str)?.to_string();
    if matches!(
        status.as_str(),
        "created" | "started" | "aborted" | "noStart"
    ) {
        return None;
    }
    let white = game.pointer("/players/white");
    let black = game.pointer("/players/black");
    let color = if player_matches(white, account) {
        "white"
    } else if player_matches(black, account) {
        "black"
    } else {
        return None;
    };
    let (ours, theirs) = if color == "white" {
        (white, black)
    } else {
        (black, white)
    };
    let result = match game.get("winner").and_then(Value::as_str) {
        Some(winner) if winner == color => "win",
        Some(_) => "loss",
        None if matches!(status.as_str(), "draw" | "stalemate") => "draw",
        None => return None,
    };
    let speed = game.get("speed").and_then(Value::as_str);
    // In the games-export API the clock fields are expressed in seconds.
    let (clock_limit, clock_increment) = if speed == Some("correspondence") {
        (None, None)
    } else {
        (
            game.pointer("/clock/initial").and_then(Value::as_i64),
            game.pointer("/clock/increment").and_then(Value::as_i64),
        )
    };
    let perf = match speed {
        Some(speed) => speed.to_string(),
        None => match clock_limit {
            Some(limit) => perf_key_for_clock(
                limit.clamp(0, u32::MAX as i64) as u32,
                clock_increment.unwrap_or(0).clamp(0, u32::MAX as i64) as u32,
            )
            .to_string(),
            None => "classical".to_string(),
        },
    };
    let finished_at_ms = game
        .get("lastMoveAt")
        .or_else(|| game.get("createdAt"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    Some(GameRecord {
        id,
        account_id: account.id.clone(),
        account_username: account.username.clone(),
        engine_id: None,
        engine_name: None,
        opponent: export_player_name(theirs),
        opponent_rating: export_player_rating(theirs),
        our_rating: export_player_rating(ours),
        color: color.to_string(),
        result: result.to_string(),
        status,
        rated: game.get("rated").and_then(Value::as_bool).unwrap_or(false),
        clock_limit,
        clock_increment,
        perf,
        moves_count: game.get("turns").and_then(Value::as_i64).unwrap_or(0),
        finished_at_ms,
        source: "import".into(),
        opening: game
            .pointer("/opening/name")
            .and_then(Value::as_str)
            .map(str::to_string),
        telemetry: None,
    })
}

#[derive(Clone, Debug, Deserialize, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub imported: i64,
    pub skipped: i64,
    pub scanned: i64,
}

#[derive(Default)]
struct HistoryInner {
    records: Vec<GameRecord>,
    ids: HashSet<String>,
}

/// Append-only JSONL store of finished games, kept fully in memory.
pub struct HistoryStore {
    path: PathBuf,
    inner: Mutex<HistoryInner>,
}

impl HistoryStore {
    pub fn path_in(app_data_dir: &Path) -> PathBuf {
        app_data_dir.join(HISTORY_FILE)
    }

    /// Loads every readable record from the JSONL file. Corrupt lines are
    /// skipped so one bad write can never take the whole history down.
    pub fn load(path: PathBuf) -> Self {
        let mut records = Vec::new();
        let mut ids = HashSet::new();
        let mut corrupt = 0usize;
        if let Ok(content) = std::fs::read_to_string(&path) {
            for line in content.lines() {
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<GameRecord>(line) {
                    Ok(record) => {
                        if ids.insert(record.id.clone()) {
                            records.push(record);
                        }
                    }
                    Err(_) => corrupt += 1,
                }
            }
        }
        if corrupt > 0 {
            crate::diagnostics::record(
                crate::diagnostics::DiagnosticEntry::warn(
                    "storage",
                    format!("Skipped {corrupt} corrupt game-history line(s)"),
                )
                .with_detail(path.display().to_string()),
            );
        }
        crate::diagnostics::record(crate::diagnostics::DiagnosticEntry::info(
            "app",
            format!("Loaded {} recorded game(s)", records.len()),
        ));
        Self {
            path,
            inner: Mutex::new(HistoryInner { records, ids }),
        }
    }

    /// Appends one record, deduplicating by game id. Returns Ok(false) when a
    /// record with the same id already exists (nothing is written).
    pub async fn append(&self, record: GameRecord) -> Result<bool, String> {
        let line = serde_json::to_string(&record)
            .map_err(|error| format!("Could not serialize a game record: {error}"))?;
        {
            let mut inner = self.inner.lock().await;
            if !inner.ids.insert(record.id.clone()) {
                return Ok(false);
            }
            inner.records.push(record);
        }
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || append_line(&path, &line))
            .await
            .map_err(|error| format!("Could not write the game history: {error}"))??;
        Ok(true)
    }

    pub async fn records(&self) -> Vec<GameRecord> {
        self.inner.lock().await.records.clone()
    }
}

fn append_line(path: &Path, line: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("Could not create the QueenUI data directory: {error}"))?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("Could not open the game history file: {error}"))?;
    writeln!(file, "{line}")
        .map_err(|error| format!("Could not append to the game history: {error}"))?;
    file.flush()
        .map_err(|error| format!("Could not flush the game history: {error}"))
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct ScorebookFilter {
    #[serde(default)]
    pub account_id: Option<String>,
    #[serde(default)]
    pub engine_id: Option<String>,
    #[serde(default)]
    pub perf: Option<String>,
    /// Inclusive epoch-ms window on finished_at_ms. Applies to every
    /// aggregation except `activity`, which is the brush overview and always
    /// covers the full span of the account/engine/perf-filtered history.
    #[serde(default)]
    pub from_ms: Option<i64>,
    #[serde(default)]
    pub to_ms: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct Streak {
    /// "win" | "draw" | "loss" | "none"
    pub kind: String,
    pub length: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct EngineLine {
    pub engine_id: Option<String>,
    pub engine_name: String,
    pub games: i64,
    pub wins: i64,
    pub draws: i64,
    pub losses: i64,
    pub score_percent: f64,
    pub avg_opponent_rating: Option<f64>,
    pub performance_rating: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct ColorLine {
    pub color: String,
    pub games: i64,
    pub wins: i64,
    pub draws: i64,
    pub losses: i64,
    pub score_percent: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct PerfLine {
    pub perf: String,
    pub games: i64,
    pub wins: i64,
    pub draws: i64,
    pub losses: i64,
    pub score_percent: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct BandLine {
    /// e.g. "1800–1999"
    pub label: String,
    pub min_rating: i64,
    pub games: i64,
    pub wins: i64,
    pub draws: i64,
    pub losses: i64,
    pub score_percent: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct TerminationLine {
    pub status: String,
    pub games: i64,
    pub wins: i64,
    pub draws: i64,
    pub losses: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct OpponentLine {
    pub name: String,
    pub games: i64,
    pub wins: i64,
    pub draws: i64,
    pub losses: i64,
    pub score_percent: f64,
    pub last_played_at_ms: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct DayLine {
    pub day_start_ms: i64,
    pub games: i64,
    pub score_points: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct RatingPoint {
    pub at_ms: i64,
    pub rating: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct OpeningLine {
    pub name: String,
    pub games: i64,
    pub score_percent: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct AccountRef {
    pub id: String,
    pub username: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct EngineRef {
    pub id: String,
    pub name: String,
}

/// One game surfaced in an Engine-lab list. For thrown wins peak_eval_cp is
/// the maximum eval reached; for steals it carries the MINIMUM eval (the depth
/// of the hole). The name is kept unified for the frontend.
#[derive(Clone, Debug, Deserialize, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct LabGame {
    pub id: String,
    pub opponent: String,
    pub opponent_rating: Option<i64>,
    pub finished_at_ms: i64,
    pub peak_eval_cp: i32,
    pub result: String,
    pub engine_name: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct EngineLabLine {
    pub engine_id: Option<String>,
    pub engine_name: String,
    pub games: i64,
    pub avg_depth: Option<f64>,
    pub avg_blunders: f64,
    pub conversion_rate: Option<f64>,
    pub avg_move_time_ms: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct DepthLine {
    pub perf: String,
    pub games: i64,
    pub avg_depth: Option<f64>,
    pub min_depth: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct BookLab {
    pub games_with_book: i64,
    pub games_without: i64,
    pub score_with: f64,
    pub score_without: Option<f64>,
    pub avg_book_plies: Option<f64>,
    /// Mean of the eval snapshot taken right after the last book move
    /// (eval_series_cp[book_plies]), over with-book games where it exists.
    pub avg_exit_eval_cp: Option<f64>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct ReliabilityTotals {
    pub engine_restarts: i64,
    pub submission_retries: i64,
    pub stream_reconnects: i64,
    /// Defaults across desktop/runner version skew where the older peer did
    /// not yet report this additive reliability counter.
    #[serde(default)]
    pub flag_safety_stops: i64,
    pub failure_resigns: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct ConfigLine {
    pub fingerprint: String,
    pub engine_name: String,
    pub games: i64,
    pub score_percent: f64,
    pub first_seen_ms: i64,
    pub last_seen_ms: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct ScorebookLab {
    pub telemetry_games: i64,
    /// max_eval_cp >= 300 but the game did not end in a win; most recent first.
    pub thrown_wins: Vec<LabGame>,
    /// min_eval_cp <= -300 but the game did not end in a loss; most recent first.
    pub steals: Vec<LabGame>,
    /// Among games with max_eval_cp >= 300: percent ending "win".
    pub conversion_rate: Option<f64>,
    /// Among games with min_eval_cp <= -300: percent ending "win" or "draw".
    pub defense_rate: Option<f64>,
    pub avg_blunders_per_game: Option<f64>,
    pub by_engine_lab: Vec<EngineLabLine>,
    pub depth_by_perf: Vec<DepthLine>,
    /// Losses by outoftime/timeout whose final eval snapshot was >= 150.
    pub flagged_winning: i64,
    pub avg_end_clock_ms: Option<f64>,
    pub book: Option<BookLab>,
    pub reliability: ReliabilityTotals,
    pub by_config: Vec<ConfigLine>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct ScorebookStats {
    pub total_games: i64,
    pub wins: i64,
    pub draws: i64,
    pub losses: i64,
    pub score_percent: f64,
    pub streak: Streak,
    pub avg_opponent_rating: Option<f64>,
    pub performance_rating: Option<f64>,
    pub by_engine: Vec<EngineLine>,
    pub by_color: Vec<ColorLine>,
    pub by_perf: Vec<PerfLine>,
    pub by_opponent_band: Vec<BandLine>,
    pub by_termination: Vec<TerminationLine>,
    pub time_losses: i64,
    pub top_opponents: Vec<OpponentLine>,
    /// Full-span brush overview; unaffected by the from_ms/to_ms window.
    pub activity: Vec<DayLine>,
    /// "day" | "week" | "month" — the bucket size activity was computed with.
    pub activity_bucket: String,
    pub rating_series: Vec<RatingPoint>,
    pub openings: Vec<OpeningLine>,
    pub accounts: Vec<AccountRef>,
    pub engines: Vec<EngineRef>,
    pub imported: i64,
    pub recorded: i64,
    /// None when the filtered set has no telemetry games.
    pub lab: Option<ScorebookLab>,
}

#[derive(Clone, Copy, Default)]
struct Tally {
    games: i64,
    wins: i64,
    draws: i64,
    losses: i64,
}

impl Tally {
    fn add(&mut self, result: &str) {
        self.games += 1;
        match result {
            "win" => self.wins += 1,
            "draw" => self.draws += 1,
            _ => self.losses += 1,
        }
    }

    fn score_percent(&self) -> f64 {
        if self.games == 0 {
            0.0
        } else {
            (self.wins as f64 + self.draws as f64 / 2.0) / self.games as f64 * 100.0
        }
    }
}

/// Average opponent rating and linear performance rating (avg + 400 * (W-L)/N)
/// over the subset of records whose opponent has a rating.
fn rating_summary(records: &[&GameRecord]) -> (Option<f64>, Option<f64>) {
    let mut sum = 0i64;
    let mut tally = Tally::default();
    for record in records {
        if let Some(rating) = record.opponent_rating {
            sum += rating;
            tally.add(&record.result);
        }
    }
    if tally.games == 0 {
        return (None, None);
    }
    let average = sum as f64 / tally.games as f64;
    let performance = average + 400.0 * (tally.wins - tally.losses) as f64 / tally.games as f64;
    (Some(average), Some(performance))
}

fn mean(values: impl Iterator<Item = f64>) -> Option<f64> {
    let mut sum = 0.0f64;
    let mut count = 0usize;
    for value in values {
        sum += value;
        count += 1;
    }
    (count > 0).then(|| sum / count as f64)
}

fn rate_percent(hits: i64, total: i64) -> Option<f64> {
    (total > 0).then(|| hits as f64 / total as f64 * 100.0)
}

fn lab_game(record: &GameRecord, peak_eval_cp: i32) -> LabGame {
    LabGame {
        id: record.id.clone(),
        opponent: record.opponent.clone(),
        opponent_rating: record.opponent_rating,
        finished_at_ms: record.finished_at_ms,
        peak_eval_cp,
        result: record.result.clone(),
        engine_name: record.engine_name.clone(),
    }
}

/// Aggregates Engine-lab insights over the telemetry-carrying subset of the
/// already-filtered records (which must be sorted oldest first). Returns None
/// when no record carries telemetry.
fn compute_lab(filtered: &[&GameRecord]) -> Option<ScorebookLab> {
    let games: Vec<(&GameRecord, &GameTelemetry)> = filtered
        .iter()
        .filter_map(|record| {
            record
                .telemetry
                .as_ref()
                .map(|telemetry| (*record, telemetry))
        })
        .collect();
    if games.is_empty() {
        return None;
    }

    let winning: Vec<&(&GameRecord, &GameTelemetry)> = games
        .iter()
        .filter(|(_, telemetry)| {
            telemetry
                .max_eval_cp
                .is_some_and(|eval| eval >= LAB_WINNING_EVAL_CP)
        })
        .collect();
    let losing: Vec<&(&GameRecord, &GameTelemetry)> = games
        .iter()
        .filter(|(_, telemetry)| {
            telemetry
                .min_eval_cp
                .is_some_and(|eval| eval <= LAB_LOSING_EVAL_CP)
        })
        .collect();

    let thrown_wins = winning
        .iter()
        .rev()
        .filter(|(record, _)| record.result != "win")
        .map(|(record, telemetry)| lab_game(record, telemetry.max_eval_cp.unwrap_or(0)))
        .take(MAX_LAB_GAMES)
        .collect();
    let steals = losing
        .iter()
        .rev()
        .filter(|(record, _)| record.result != "loss")
        .map(|(record, telemetry)| lab_game(record, telemetry.min_eval_cp.unwrap_or(0)))
        .take(MAX_LAB_GAMES)
        .collect();

    let conversion_rate = rate_percent(
        winning
            .iter()
            .filter(|(record, _)| record.result == "win")
            .count() as i64,
        winning.len() as i64,
    );
    let defense_rate = rate_percent(
        losing
            .iter()
            .filter(|(record, _)| record.result != "loss")
            .count() as i64,
        losing.len() as i64,
    );

    let avg_blunders_per_game = mean(games.iter().map(|(_, telemetry)| telemetry.blunders as f64));

    let by_engine_lab = {
        type EngineGroup<'a> = (Option<String>, Vec<(&'a GameRecord, &'a GameTelemetry)>);
        let mut groups: HashMap<Option<String>, EngineGroup> = HashMap::new();
        for (record, telemetry) in &games {
            let entry = groups
                .entry(record.engine_id.clone())
                .or_insert_with(|| (None, Vec::new()));
            if entry.0.is_none() {
                entry.0 = record.engine_name.clone();
            }
            entry.1.push((record, telemetry));
        }
        let mut lines: Vec<EngineLabLine> = groups
            .into_iter()
            .map(|(engine_id, (name, group))| {
                let group_winning: Vec<_> = group
                    .iter()
                    .filter(|(_, telemetry)| {
                        telemetry
                            .max_eval_cp
                            .is_some_and(|eval| eval >= LAB_WINNING_EVAL_CP)
                    })
                    .collect();
                EngineLabLine {
                    engine_id,
                    engine_name: name.unwrap_or_else(|| "Imported / unknown".into()),
                    games: group.len() as i64,
                    avg_depth: mean(
                        group
                            .iter()
                            .filter_map(|(_, telemetry)| telemetry.avg_depth),
                    ),
                    avg_blunders: group
                        .iter()
                        .map(|(_, telemetry)| telemetry.blunders)
                        .sum::<i64>() as f64
                        / group.len() as f64,
                    conversion_rate: rate_percent(
                        group_winning
                            .iter()
                            .filter(|(record, _)| record.result == "win")
                            .count() as i64,
                        group_winning.len() as i64,
                    ),
                    avg_move_time_ms: mean(
                        group
                            .iter()
                            .filter_map(|(_, telemetry)| telemetry.avg_move_time_ms),
                    ),
                }
            })
            .collect();
        lines.sort_by(|left, right| {
            right
                .games
                .cmp(&left.games)
                .then_with(|| left.engine_name.cmp(&right.engine_name))
        });
        lines
    };

    let depth_by_perf = PERF_ORDER
        .iter()
        .filter_map(|perf| {
            let group: Vec<_> = games
                .iter()
                .filter(|(record, _)| record.perf == *perf)
                .collect();
            if group.is_empty() {
                return None;
            }
            Some(DepthLine {
                perf: (*perf).into(),
                games: group.len() as i64,
                avg_depth: mean(
                    group
                        .iter()
                        .filter_map(|(_, telemetry)| telemetry.avg_depth),
                ),
                min_depth: group
                    .iter()
                    .filter_map(|(_, telemetry)| telemetry.min_depth)
                    .min(),
            })
        })
        .collect();

    let flagged_winning = games
        .iter()
        .filter(|(record, telemetry)| {
            record.result == "loss"
                && matches!(record.status.as_str(), "outoftime" | "timeout")
                && telemetry
                    .eval_series_cp
                    .last()
                    .is_some_and(|eval| *eval >= LAB_FLAG_EVAL_CP)
        })
        .count() as i64;

    let avg_end_clock_ms = mean(
        games
            .iter()
            .filter_map(|(_, telemetry)| telemetry.end_clock_ms.map(|clock| clock as f64)),
    );

    let book = {
        let with: Vec<_> = games
            .iter()
            .filter(|(_, telemetry)| telemetry.book_plies > 0)
            .collect();
        let without: Vec<_> = games
            .iter()
            .filter(|(_, telemetry)| telemetry.book_plies == 0)
            .collect();
        (!with.is_empty()).then(|| {
            let mut with_tally = Tally::default();
            for (record, _) in &with {
                with_tally.add(&record.result);
            }
            let mut without_tally = Tally::default();
            for (record, _) in &without {
                without_tally.add(&record.result);
            }
            BookLab {
                games_with_book: with.len() as i64,
                games_without: without.len() as i64,
                score_with: with_tally.score_percent(),
                score_without: (!without.is_empty()).then(|| without_tally.score_percent()),
                avg_book_plies: mean(
                    with.iter()
                        .map(|(_, telemetry)| telemetry.book_plies as f64),
                ),
                avg_exit_eval_cp: mean(with.iter().filter_map(|(_, telemetry)| {
                    telemetry
                        .eval_series_cp
                        .get(telemetry.book_plies.max(0) as usize)
                        .map(|eval| *eval as f64)
                })),
            }
        })
    };

    let reliability = games.iter().fold(
        ReliabilityTotals::default(),
        |mut totals, (_, telemetry)| {
            totals.engine_restarts += telemetry.engine_restarts;
            totals.submission_retries += telemetry.submission_retries;
            totals.stream_reconnects += telemetry.stream_reconnects;
            totals.flag_safety_stops += telemetry.flag_safety_stops;
            totals.failure_resigns += i64::from(telemetry.failure_resign);
            totals
        },
    );

    let by_config = {
        let mut groups: HashMap<&str, (Option<String>, Tally, i64, i64)> = HashMap::new();
        for (record, telemetry) in &games {
            if let Some(fingerprint) = telemetry.config_fingerprint.as_deref() {
                let entry = groups
                    .entry(fingerprint)
                    .or_insert_with(|| (None, Tally::default(), i64::MAX, i64::MIN));
                if entry.0.is_none() {
                    entry.0 = record.engine_name.clone();
                }
                entry.1.add(&record.result);
                entry.2 = entry.2.min(record.finished_at_ms);
                entry.3 = entry.3.max(record.finished_at_ms);
            }
        }
        let mut lines: Vec<ConfigLine> = groups
            .into_iter()
            .map(
                |(fingerprint, (name, tally, first_seen_ms, last_seen_ms))| ConfigLine {
                    fingerprint: fingerprint.into(),
                    engine_name: name.unwrap_or_else(|| "Unknown engine".into()),
                    games: tally.games,
                    score_percent: tally.score_percent(),
                    first_seen_ms,
                    last_seen_ms,
                },
            )
            .collect();
        lines.sort_by(|left, right| {
            right
                .games
                .cmp(&left.games)
                .then_with(|| left.fingerprint.cmp(&right.fingerprint))
        });
        lines.truncate(MAX_CONFIG_LINES);
        lines
    };

    Some(ScorebookLab {
        telemetry_games: games.len() as i64,
        thrown_wins,
        steals,
        conversion_rate,
        defense_rate,
        avg_blunders_per_game,
        by_engine_lab,
        depth_by_perf,
        flagged_winning,
        avg_end_clock_ms,
        book,
        reliability,
        by_config,
    })
}

fn matches_filter(record: &GameRecord, filter: &ScorebookFilter) -> bool {
    filter
        .account_id
        .as_ref()
        .is_none_or(|id| &record.account_id == id)
        && filter
            .engine_id
            .as_ref()
            .is_none_or(|id| record.engine_id.as_ref() == Some(id))
        && filter.perf.as_ref().is_none_or(|perf| &record.perf == perf)
}

/// The inclusive [from_ms, to_ms] time window, applied separately from
/// matches_filter because `activity` must ignore it.
fn in_window(record: &GameRecord, filter: &ScorebookFilter) -> bool {
    filter
        .from_ms
        .is_none_or(|from| record.finished_at_ms >= from)
        && filter.to_ms.is_none_or(|to| record.finished_at_ms <= to)
}

/// UTC day start (midnight) containing `ms`.
fn day_start_ms(ms: i64) -> i64 {
    ms - ms.rem_euclid(DAY_MS)
}

/// UTC Monday-start week containing `ms`. 1970-01-01 (epoch day 0) was a
/// Thursday, so `(day + 3) mod 7` yields the weekday with Monday = 0.
fn week_start_ms(ms: i64) -> i64 {
    let day = ms.div_euclid(DAY_MS);
    let weekday_from_monday = (day + 3).rem_euclid(7);
    (day - weekday_from_monday) * DAY_MS
}

/// Civil date from days since the Unix epoch (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month)
}

/// Days since the Unix epoch for the first day of `month` in `year`
/// (Howard Hinnant's algorithm, day fixed to 1).
fn days_from_civil_month(year: i64, month: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// UTC calendar-month start containing `ms`.
fn month_start_ms(ms: i64) -> i64 {
    let (year, month) = civil_from_days(ms.div_euclid(DAY_MS));
    days_from_civil_month(year, month) * DAY_MS
}

/// Start of the month after the month containing `ms`.
fn next_month_start_ms(ms: i64) -> i64 {
    let (year, month) = civil_from_days(ms.div_euclid(DAY_MS));
    let (year, month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    days_from_civil_month(year, month) * DAY_MS
}

/// Full-span activity histogram with adaptive bucketing: daily buckets when
/// the span fits in 120 of them, otherwise weekly (UTC, Monday start) when
/// those fit in 120, otherwise UTC calendar months. Empty buckets across the
/// span are included. `records` must be sorted by finished_at_ms ascending.
fn compute_activity(records: &[&GameRecord]) -> (Vec<DayLine>, String) {
    let (Some(first), Some(last)) = (records.first(), records.last()) else {
        return (Vec::new(), "day".into());
    };
    let first_ms = first.finished_at_ms;
    let last_ms = last.finished_at_ms;

    let day_buckets = (day_start_ms(last_ms) - day_start_ms(first_ms)) / DAY_MS + 1;
    let week_buckets = (week_start_ms(last_ms) - week_start_ms(first_ms)) / WEEK_MS + 1;
    let (bucket, starts): (&str, Vec<i64>) = if day_buckets <= MAX_ACTIVITY_BUCKETS {
        (
            "day",
            (0..day_buckets)
                .map(|index| day_start_ms(first_ms) + index * DAY_MS)
                .collect(),
        )
    } else if week_buckets <= MAX_ACTIVITY_BUCKETS {
        (
            "week",
            (0..week_buckets)
                .map(|index| week_start_ms(first_ms) + index * WEEK_MS)
                .collect(),
        )
    } else {
        let mut starts = Vec::new();
        let mut start = month_start_ms(first_ms);
        let last_start = month_start_ms(last_ms);
        while start <= last_start {
            starts.push(start);
            start = next_month_start_ms(start);
        }
        ("month", starts)
    };

    let indices: HashMap<i64, usize> = starts
        .iter()
        .enumerate()
        .map(|(index, start)| (*start, index))
        .collect();
    let mut lines: Vec<DayLine> = starts
        .into_iter()
        .map(|day_start_ms| DayLine {
            day_start_ms,
            games: 0,
            score_points: 0.0,
        })
        .collect();
    for record in records {
        let start = match bucket {
            "day" => day_start_ms(record.finished_at_ms),
            "week" => week_start_ms(record.finished_at_ms),
            _ => month_start_ms(record.finished_at_ms),
        };
        if let Some(index) = indices.get(&start) {
            lines[*index].games += 1;
            lines[*index].score_points += match record.result.as_str() {
                "win" => 1.0,
                "draw" => 0.5,
                _ => 0.0,
            };
        }
    }
    (lines, bucket.into())
}

pub fn compute_stats(
    records: &[GameRecord],
    filter: &ScorebookFilter,
    accounts: Vec<AccountRef>,
    engines: Vec<EngineRef>,
) -> ScorebookStats {
    // The brush overview (`activity`) spans the account/engine/perf-filtered
    // history regardless of the time window; everything else also applies the
    // inclusive [from_ms, to_ms] window.
    let mut span: Vec<&GameRecord> = records
        .iter()
        .filter(|record| matches_filter(record, filter))
        .collect();
    span.sort_by(|left, right| {
        left.finished_at_ms
            .cmp(&right.finished_at_ms)
            .then_with(|| left.id.cmp(&right.id))
    });
    let (activity, activity_bucket) = compute_activity(&span);
    let filtered: Vec<&GameRecord> = span
        .iter()
        .filter(|record| in_window(record, filter))
        .copied()
        .collect();

    let mut totals = Tally::default();
    for record in &filtered {
        totals.add(&record.result);
    }

    let streak = {
        let mut kind = "none".to_string();
        let mut length = 0i64;
        for record in filtered.iter().rev() {
            if length == 0 {
                kind = record.result.clone();
                length = 1;
            } else if record.result == kind {
                length += 1;
            } else {
                break;
            }
        }
        Streak { kind, length }
    };

    let (avg_opponent_rating, performance_rating) = rating_summary(&filtered);

    let by_engine = {
        let mut groups: HashMap<Option<String>, (Option<String>, Vec<&GameRecord>)> =
            HashMap::new();
        for record in &filtered {
            let entry = groups
                .entry(record.engine_id.clone())
                .or_insert_with(|| (None, Vec::new()));
            if entry.0.is_none() {
                entry.0 = record.engine_name.clone();
            }
            entry.1.push(record);
        }
        let mut lines: Vec<EngineLine> = groups
            .into_iter()
            .map(|(engine_id, (name, group))| {
                let mut tally = Tally::default();
                for record in &group {
                    tally.add(&record.result);
                }
                let (avg, performance) = rating_summary(&group);
                EngineLine {
                    engine_id,
                    engine_name: name.unwrap_or_else(|| "Imported / unknown".into()),
                    games: tally.games,
                    wins: tally.wins,
                    draws: tally.draws,
                    losses: tally.losses,
                    score_percent: tally.score_percent(),
                    avg_opponent_rating: avg,
                    performance_rating: performance,
                }
            })
            .collect();
        lines.sort_by(|left, right| {
            right
                .games
                .cmp(&left.games)
                .then_with(|| left.engine_name.cmp(&right.engine_name))
        });
        lines
    };

    let by_color = {
        let mut white = Tally::default();
        let mut black = Tally::default();
        for record in &filtered {
            if record.color == "white" {
                white.add(&record.result);
            } else {
                black.add(&record.result);
            }
        }
        [("white", white), ("black", black)]
            .into_iter()
            .map(|(color, tally)| ColorLine {
                color: color.into(),
                games: tally.games,
                wins: tally.wins,
                draws: tally.draws,
                losses: tally.losses,
                score_percent: tally.score_percent(),
            })
            .collect()
    };

    let by_perf = {
        let mut tallies: HashMap<&str, Tally> = HashMap::new();
        for record in &filtered {
            if let Some(perf) = PERF_ORDER.iter().find(|perf| **perf == record.perf) {
                tallies.entry(perf).or_default().add(&record.result);
            }
        }
        PERF_ORDER
            .iter()
            .filter_map(|perf| {
                let tally = tallies.get(perf)?;
                Some(PerfLine {
                    perf: (*perf).into(),
                    games: tally.games,
                    wins: tally.wins,
                    draws: tally.draws,
                    losses: tally.losses,
                    score_percent: tally.score_percent(),
                })
            })
            .collect()
    };

    let by_opponent_band = {
        let mut bands: HashMap<i64, Tally> = HashMap::new();
        for record in &filtered {
            if let Some(rating) = record.opponent_rating {
                let band = rating.div_euclid(RATING_BAND_WIDTH) * RATING_BAND_WIDTH;
                bands.entry(band).or_default().add(&record.result);
            }
        }
        let mut lines: Vec<BandLine> = bands
            .into_iter()
            .map(|(min_rating, tally)| BandLine {
                label: format!("{min_rating}–{}", min_rating + RATING_BAND_WIDTH - 1),
                min_rating,
                games: tally.games,
                wins: tally.wins,
                draws: tally.draws,
                losses: tally.losses,
                score_percent: tally.score_percent(),
            })
            .collect();
        lines.sort_by_key(|line| line.min_rating);
        lines
    };

    let by_termination = {
        let mut tallies: HashMap<&str, Tally> = HashMap::new();
        for record in &filtered {
            tallies
                .entry(record.status.as_str())
                .or_default()
                .add(&record.result);
        }
        let mut lines: Vec<TerminationLine> = tallies
            .into_iter()
            .map(|(status, tally)| TerminationLine {
                status: status.into(),
                games: tally.games,
                wins: tally.wins,
                draws: tally.draws,
                losses: tally.losses,
            })
            .collect();
        lines.sort_by(|left, right| {
            right
                .games
                .cmp(&left.games)
                .then_with(|| left.status.cmp(&right.status))
        });
        lines
    };

    let time_losses = filtered
        .iter()
        .filter(|record| {
            record.result == "loss" && matches!(record.status.as_str(), "outoftime" | "timeout")
        })
        .count() as i64;

    let top_opponents = {
        let mut groups: HashMap<String, (String, Tally, i64)> = HashMap::new();
        for record in &filtered {
            let entry = groups
                .entry(record.opponent.to_lowercase())
                .or_insert_with(|| (record.opponent.clone(), Tally::default(), i64::MIN));
            entry.1.add(&record.result);
            entry.2 = entry.2.max(record.finished_at_ms);
        }
        let mut lines: Vec<OpponentLine> = groups
            .into_values()
            .map(|(name, tally, last_played_at_ms)| OpponentLine {
                name,
                games: tally.games,
                wins: tally.wins,
                draws: tally.draws,
                losses: tally.losses,
                score_percent: tally.score_percent(),
                last_played_at_ms,
            })
            .collect();
        lines.sort_by(|left, right| {
            right
                .games
                .cmp(&left.games)
                .then_with(|| right.last_played_at_ms.cmp(&left.last_played_at_ms))
                .then_with(|| left.name.cmp(&right.name))
        });
        lines.truncate(MAX_TOP_OPPONENTS);
        lines
    };

    let rating_series = {
        let points: Vec<RatingPoint> = filtered
            .iter()
            .filter_map(|record| {
                record.our_rating.map(|rating| RatingPoint {
                    at_ms: record.finished_at_ms,
                    rating,
                })
            })
            .collect();
        if points.len() > MAX_RATING_POINTS {
            let last = points.len() - 1;
            let mut sampled = Vec::with_capacity(MAX_RATING_POINTS);
            let mut previous = usize::MAX;
            for step in 0..MAX_RATING_POINTS {
                let index = step * last / (MAX_RATING_POINTS - 1);
                if index != previous {
                    sampled.push(points[index].clone());
                    previous = index;
                }
            }
            sampled
        } else {
            points
        }
    };

    let openings = {
        let mut tallies: HashMap<&str, Tally> = HashMap::new();
        for record in &filtered {
            if let Some(opening) = record.opening.as_deref() {
                tallies.entry(opening).or_default().add(&record.result);
            }
        }
        let mut lines: Vec<OpeningLine> = tallies
            .into_iter()
            .map(|(name, tally)| OpeningLine {
                name: name.into(),
                games: tally.games,
                score_percent: tally.score_percent(),
            })
            .collect();
        lines.sort_by(|left, right| {
            right
                .games
                .cmp(&left.games)
                .then_with(|| left.name.cmp(&right.name))
        });
        lines.truncate(MAX_OPENINGS);
        lines
    };

    let imported = filtered
        .iter()
        .filter(|record| record.source == "import")
        .count() as i64;
    let recorded = filtered
        .iter()
        .filter(|record| record.source == "queenui")
        .count() as i64;

    let lab = compute_lab(&filtered);

    ScorebookStats {
        total_games: totals.games,
        wins: totals.wins,
        draws: totals.draws,
        losses: totals.losses,
        score_percent: totals.score_percent(),
        streak,
        avg_opponent_rating,
        performance_rating,
        by_engine,
        by_color,
        by_perf,
        by_opponent_band,
        by_termination,
        time_losses,
        top_opponents,
        activity,
        activity_bucket,
        rating_series,
        openings,
        accounts,
        engines,
        imported,
        recorded,
        lab,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        compute_stats, count_blunders, perf_key_for_clock, record_from_lichess_export,
        result_from_pgn, GameRecord, GameTelemetry, HistoryStore, ReliabilityTotals,
        ScorebookFilter,
    };
    use crate::models::AccountProfile;
    use serde_json::json;
    use std::path::PathBuf;

    fn account() -> AccountProfile {
        AccountProfile {
            id: "queenbot".into(),
            username: "QueenBot".into(),
            engine_id: "engine-1".into(),
            rating: Some(2100),
            enabled: false,
        }
    }

    fn export_game(overrides: serde_json::Value) -> serde_json::Value {
        let mut game = json!({
            "id": "game0001",
            "rated": true,
            "variant": "standard",
            "speed": "blitz",
            "perf": "blitz",
            "createdAt": 1_750_000_000_000i64,
            "lastMoveAt": 1_750_000_600_000i64,
            "status": "mate",
            "players": {
                "white": { "user": { "name": "QueenBot", "id": "queenbot" }, "rating": 2105 },
                "black": { "user": { "name": "Rival", "id": "rival" }, "rating": 2050 }
            },
            "winner": "white",
            "opening": { "eco": "C50", "name": "Italian Game" },
            "clock": { "initial": 300, "increment": 3, "totalTime": 420 },
            "turns": 61
        });
        game.as_object_mut()
            .unwrap()
            .extend(overrides.as_object().unwrap().clone());
        game
    }

    fn record(id: &str, result: &str, finished_at_ms: i64) -> GameRecord {
        GameRecord {
            id: id.into(),
            account_id: "queenbot".into(),
            account_username: "QueenBot".into(),
            engine_id: Some("engine-1".into()),
            engine_name: Some("Stockfish".into()),
            opponent: "Rival".into(),
            opponent_rating: Some(2000),
            our_rating: Some(2100),
            color: "white".into(),
            result: result.into(),
            status: "mate".into(),
            rated: true,
            clock_limit: Some(300),
            clock_increment: Some(3),
            perf: "blitz".into(),
            moves_count: 40,
            finished_at_ms,
            source: "queenui".into(),
            opening: None,
            telemetry: None,
        }
    }

    fn telemetry(max_eval_cp: i32, min_eval_cp: i32, series: Vec<i32>) -> GameTelemetry {
        GameTelemetry {
            eval_series_cp: series,
            max_eval_cp: Some(max_eval_cp),
            min_eval_cp: Some(min_eval_cp),
            ..GameTelemetry::default()
        }
    }

    #[test]
    fn maps_clocks_to_rating_pools() {
        assert_eq!(perf_key_for_clock(15, 0), "ultraBullet");
        assert_eq!(perf_key_for_clock(29, 0), "ultraBullet");
        assert_eq!(perf_key_for_clock(30, 0), "bullet");
        assert_eq!(perf_key_for_clock(60, 0), "bullet");
        assert_eq!(perf_key_for_clock(180, 2), "blitz");
        assert_eq!(perf_key_for_clock(300, 3), "blitz");
        assert_eq!(perf_key_for_clock(600, 0), "rapid");
        assert_eq!(perf_key_for_clock(1800, 0), "classical");
    }

    #[test]
    fn maps_pgn_results_to_outcomes() {
        assert_eq!(result_from_pgn("1-0", "white"), Some("win"));
        assert_eq!(result_from_pgn("1-0", "black"), Some("loss"));
        assert_eq!(result_from_pgn("0-1", "black"), Some("win"));
        assert_eq!(result_from_pgn("0-1", "white"), Some("loss"));
        assert_eq!(result_from_pgn("1/2-1/2", "white"), Some("draw"));
        assert_eq!(result_from_pgn("*", "white"), None);
    }

    #[test]
    fn maps_an_exported_win_as_white() {
        let game = export_game(json!({}));
        let record = record_from_lichess_export(&game, &account()).expect("record");
        assert_eq!(record.id, "game0001");
        assert_eq!(record.color, "white");
        assert_eq!(record.result, "win");
        assert_eq!(record.status, "mate");
        assert_eq!(record.opponent, "Rival");
        assert_eq!(record.opponent_rating, Some(2050));
        assert_eq!(record.our_rating, Some(2105));
        assert_eq!(record.perf, "blitz");
        assert_eq!(record.clock_limit, Some(300));
        assert_eq!(record.clock_increment, Some(3));
        assert_eq!(record.moves_count, 61);
        assert_eq!(record.finished_at_ms, 1_750_000_600_000);
        assert_eq!(record.source, "import");
        assert_eq!(record.opening.as_deref(), Some("Italian Game"));
        assert!(record.rated);
        assert!(record.engine_id.is_none());
    }

    #[test]
    fn maps_an_exported_time_loss_as_black() {
        let game = export_game(json!({
            "id": "game0002",
            "status": "outoftime",
            "winner": "white",
            "players": {
                "white": { "user": { "name": "Rival", "id": "rival" }, "rating": 2050 },
                "black": { "user": { "name": "QueenBot", "id": "queenbot" }, "rating": 2105 }
            }
        }));
        let record = record_from_lichess_export(&game, &account()).expect("record");
        assert_eq!(record.color, "black");
        assert_eq!(record.result, "loss");
        assert_eq!(record.status, "outoftime");
        assert_eq!(record.opponent, "Rival");
        assert_eq!(record.opponent_rating, Some(2050));
        assert_eq!(record.our_rating, Some(2105));
    }

    #[test]
    fn maps_an_exported_draw_without_a_winner() {
        let mut game = export_game(json!({ "id": "game0003", "status": "draw" }));
        game.as_object_mut().unwrap().remove("winner");
        let record = record_from_lichess_export(&game, &account()).expect("record");
        assert_eq!(record.result, "draw");
    }

    #[test]
    fn skips_aborted_foreign_and_undecidable_exports() {
        let aborted = export_game(json!({ "status": "aborted" }));
        assert!(record_from_lichess_export(&aborted, &account()).is_none());

        let foreign = export_game(json!({
            "players": {
                "white": { "user": { "name": "SomeoneElse", "id": "someoneelse" }, "rating": 1900 },
                "black": { "user": { "name": "Rival", "id": "rival" }, "rating": 2050 }
            }
        }));
        assert!(record_from_lichess_export(&foreign, &account()).is_none());

        let mut undecidable = export_game(json!({ "status": "unknownFinish" }));
        undecidable.as_object_mut().unwrap().remove("winner");
        assert!(record_from_lichess_export(&undecidable, &account()).is_none());
    }

    #[test]
    fn computes_totals_score_and_streak() {
        let records = vec![
            record("a", "loss", 1_000),
            record("b", "draw", 2_000),
            record("c", "win", 3_000),
            record("d", "win", 4_000),
        ];
        let stats = compute_stats(
            &records,
            &ScorebookFilter::default(),
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(stats.total_games, 4);
        assert_eq!(stats.wins, 2);
        assert_eq!(stats.draws, 1);
        assert_eq!(stats.losses, 1);
        assert!((stats.score_percent - 62.5).abs() < f64::EPSILON);
        assert_eq!(stats.streak.kind, "win");
        assert_eq!(stats.streak.length, 2);
        assert_eq!(stats.recorded, 4);
        assert_eq!(stats.imported, 0);
    }

    #[test]
    fn empty_history_scores_zero_with_no_streak() {
        let stats = compute_stats(&[], &ScorebookFilter::default(), Vec::new(), Vec::new());
        assert_eq!(stats.total_games, 0);
        assert_eq!(stats.score_percent, 0.0);
        assert_eq!(stats.streak.kind, "none");
        assert_eq!(stats.streak.length, 0);
        assert!(stats.avg_opponent_rating.is_none());
        assert!(stats.performance_rating.is_none());
        assert!(stats.by_perf.is_empty());
        assert!(stats.activity.is_empty());
        assert_eq!(stats.activity_bucket, "day");
    }

    #[test]
    fn buckets_opponents_into_200_point_bands() {
        let mut low = record("a", "win", 1_000);
        low.opponent_rating = Some(1850);
        let mut mid = record("b", "loss", 2_000);
        mid.opponent_rating = Some(1999);
        let mut high = record("c", "draw", 3_000);
        high.opponent_rating = Some(2205);
        let mut unrated = record("d", "win", 4_000);
        unrated.opponent_rating = None;
        let stats = compute_stats(
            &[low, mid, high, unrated],
            &ScorebookFilter::default(),
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(stats.by_opponent_band.len(), 2);
        assert_eq!(stats.by_opponent_band[0].label, "1800–1999");
        assert_eq!(stats.by_opponent_band[0].min_rating, 1800);
        assert_eq!(stats.by_opponent_band[0].games, 2);
        assert_eq!(stats.by_opponent_band[1].label, "2200–2399");
        assert_eq!(stats.by_opponent_band[1].games, 1);
    }

    #[test]
    fn filters_compose_over_account_engine_and_perf() {
        let mut other_account = record("a", "win", 1_000);
        other_account.account_id = "otherbot".into();
        let mut bullet = record("b", "win", 2_000);
        bullet.perf = "bullet".into();
        let matching = record("c", "loss", 3_000);
        let stats = compute_stats(
            &[other_account, bullet, matching],
            &ScorebookFilter {
                account_id: Some("queenbot".into()),
                engine_id: Some("engine-1".into()),
                perf: Some("blitz".into()),
                ..ScorebookFilter::default()
            },
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(stats.total_games, 1);
        assert_eq!(stats.losses, 1);
    }

    #[tokio::test]
    async fn appends_with_dedup_and_reloads_past_corrupt_lines() {
        let directory =
            std::env::temp_dir().join(format!("queenui-history-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).expect("create temp dir");
        let path: PathBuf = directory.join("history.jsonl");

        let store = HistoryStore::load(path.clone());
        assert!(store
            .append(record("one", "win", 1_000))
            .await
            .expect("append"));
        assert!(store
            .append(record("two", "loss", 2_000))
            .await
            .expect("append"));
        // Same id again: deduplicated, nothing written.
        assert!(!store
            .append(record("one", "draw", 3_000))
            .await
            .expect("append"));
        assert_eq!(store.records().await.len(), 2);

        // Corrupt one line on disk; the reload must keep every readable record.
        let mut content = std::fs::read_to_string(&path).expect("read history");
        content.push_str("{ this is not json\n");
        std::fs::write(&path, content).expect("write history");
        let reloaded = HistoryStore::load(path.clone());
        let records = reloaded.records().await;
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].id, "one");
        assert_eq!(records[0].result, "win");
        assert_eq!(records[1].id, "two");

        // Appending after a reload still dedups against persisted ids.
        assert!(!reloaded
            .append(record("two", "win", 4_000))
            .await
            .expect("append"));
        assert!(reloaded
            .append(record("three", "win", 5_000))
            .await
            .expect("append"));
        assert_eq!(HistoryStore::load(path).records().await.len(), 3);

        let _ = std::fs::remove_dir_all(directory);
    }

    fn stats(records: &[GameRecord]) -> super::ScorebookStats {
        compute_stats(records, &ScorebookFilter::default(), Vec::new(), Vec::new())
    }

    #[test]
    fn telemetry_roundtrips_and_legacy_lines_still_deserialize() {
        let mut with = record("a", "win", 1_000);
        with.telemetry = Some(GameTelemetry {
            eval_series_cp: vec![20, 150, -80],
            avg_depth: Some(21.5),
            min_depth: Some(18),
            avg_move_time_ms: Some(742.0),
            max_move_time_ms: Some(1_950),
            end_clock_ms: Some(41_000),
            book_plies: 2,
            engine_restarts: 1,
            submission_retries: 3,
            stream_reconnects: 2,
            flag_safety_stops: 4,
            failure_resign: false,
            max_eval_cp: Some(150),
            min_eval_cp: Some(-80),
            blunders: 1,
            config_fingerprint: Some("00af31c2d9e4".into()),
        });
        let line = serde_json::to_string(&with).expect("serialize");
        assert!(line.contains("\"telemetry\""));
        assert!(line.contains("\"evalSeriesCp\""));
        assert!(line.contains("\"configFingerprint\""));
        let back: GameRecord = serde_json::from_str(&line).expect("deserialize");
        let telemetry = back.telemetry.expect("telemetry survives");
        assert_eq!(telemetry.eval_series_cp, vec![20, 150, -80]);
        assert_eq!(telemetry.min_depth, Some(18));
        assert_eq!(telemetry.book_plies, 2);
        assert_eq!(telemetry.submission_retries, 3);
        assert_eq!(telemetry.flag_safety_stops, 4);
        assert_eq!(
            telemetry.config_fingerprint.as_deref(),
            Some("00af31c2d9e4")
        );

        // Records without telemetry serialize without the key at all.
        let without = record("b", "loss", 2_000);
        let line = serde_json::to_string(&without).expect("serialize");
        assert!(!line.contains("telemetry"));

        // A phase-1 line (no telemetry key) must still deserialize.
        let mut legacy = serde_json::to_value(&with).expect("to value");
        legacy.as_object_mut().unwrap().remove("telemetry");
        let back: GameRecord = serde_json::from_value(legacy).expect("legacy deserialize");
        assert!(back.telemetry.is_none());

        // Telemetry written before the flag-safety counter was introduced is
        // still valid and means no recorded safety stops.
        let mut legacy = serde_json::to_value(&with).expect("to value");
        legacy["telemetry"]
            .as_object_mut()
            .unwrap()
            .remove("flagSafetyStops");
        let back: GameRecord = serde_json::from_value(legacy).expect("legacy telemetry");
        assert_eq!(back.telemetry.unwrap().flag_safety_stops, 0);
    }

    #[test]
    fn counts_blunders_as_200cp_drops_between_snapshots() {
        assert_eq!(count_blunders(&[]), 0);
        assert_eq!(count_blunders(&[100]), 0);
        // 50 -> -160 (drop 210), -50 -> -400 (drop 350); rises never count.
        assert_eq!(count_blunders(&[50, -160, -50, -400, 100]), 2);
        // Exactly 200 counts; 199 does not.
        assert_eq!(count_blunders(&[200, 0]), 1);
        assert_eq!(count_blunders(&[199, 0]), 0);
    }

    #[test]
    fn lab_is_none_without_telemetry_games() {
        let stats = stats(&[record("a", "win", 1_000)]);
        assert!(stats.lab.is_none());
    }

    #[test]
    fn classifies_thrown_wins_and_steals_most_recent_first() {
        let mut thrown_old = record("t1", "draw", 1_000);
        thrown_old.telemetry = Some(telemetry(450, 10, vec![450, 10]));
        let mut thrown_new = record("t2", "loss", 3_000);
        thrown_new.telemetry = Some(telemetry(300, -50, vec![300, -50]));
        let mut converted = record("t3", "win", 2_000);
        converted.telemetry = Some(telemetry(600, 0, vec![600]));
        let mut steal = record("s1", "draw", 4_000);
        steal.telemetry = Some(telemetry(20, -300, vec![-300, 20]));
        let mut lost_hole = record("s2", "loss", 5_000);
        lost_hole.telemetry = Some(telemetry(0, -700, vec![-700]));

        let stats = stats(&[thrown_old, thrown_new, converted, steal, lost_hole]);
        let lab = stats.lab.expect("lab");
        assert_eq!(lab.telemetry_games, 5);
        assert_eq!(lab.thrown_wins.len(), 2);
        assert_eq!(lab.thrown_wins[0].id, "t2");
        assert_eq!(lab.thrown_wins[0].peak_eval_cp, 300);
        assert_eq!(lab.thrown_wins[1].id, "t1");
        assert_eq!(lab.thrown_wins[1].peak_eval_cp, 450);
        assert_eq!(lab.steals.len(), 1);
        assert_eq!(lab.steals[0].id, "s1");
        // For steals peak_eval_cp carries the minimum eval.
        assert_eq!(lab.steals[0].peak_eval_cp, -300);
    }

    #[test]
    fn computes_conversion_and_defense_rates() {
        let mut converted = record("a", "win", 1_000);
        converted.telemetry = Some(telemetry(500, 50, vec![500]));
        let mut thrown = record("b", "draw", 2_000);
        thrown.telemetry = Some(telemetry(400, -350, vec![400, -350]));
        let mut collapsed = record("c", "loss", 3_000);
        collapsed.telemetry = Some(telemetry(100, -500, vec![100, -500]));
        let mut quiet = record("d", "win", 4_000);
        quiet.telemetry = Some(telemetry(120, -90, vec![120, -90]));

        let stats = stats(&[converted, thrown, collapsed, quiet]);
        let lab = stats.lab.expect("lab");
        // 2 winning games (a, b); only a converted.
        assert!((lab.conversion_rate.expect("conversion") - 50.0).abs() < f64::EPSILON);
        // 2 losing games (b, c); only b was held.
        assert!((lab.defense_rate.expect("defense") - 50.0).abs() < f64::EPSILON);
    }

    #[test]
    fn splits_book_and_bookless_games_with_exit_eval() {
        let mut booked_win = record("a", "win", 1_000);
        booked_win.telemetry = Some(GameTelemetry {
            eval_series_cp: vec![0, 0, 40, 90],
            book_plies: 2,
            ..GameTelemetry::default()
        });
        let mut booked_loss = record("b", "loss", 2_000);
        booked_loss.telemetry = Some(GameTelemetry {
            eval_series_cp: vec![0, -120],
            book_plies: 1,
            ..GameTelemetry::default()
        });
        let mut bookless = record("c", "win", 3_000);
        bookless.telemetry = Some(GameTelemetry {
            eval_series_cp: vec![30, 60],
            ..GameTelemetry::default()
        });

        let stats = stats(&[booked_win, booked_loss, bookless]);
        let book = stats.lab.expect("lab").book.expect("book lab");
        assert_eq!(book.games_with_book, 2);
        assert_eq!(book.games_without, 1);
        assert!((book.score_with - 50.0).abs() < f64::EPSILON);
        assert!((book.score_without.expect("score without") - 100.0).abs() < f64::EPSILON);
        assert!((book.avg_book_plies.expect("plies") - 1.5).abs() < f64::EPSILON);
        // Exit evals: series[2] = 40 and series[1] = -120 -> mean -40.
        assert!((book.avg_exit_eval_cp.expect("exit eval") - (-40.0)).abs() < f64::EPSILON);

        // No with-book games at all: the whole book section is None.
        let mut none = record("d", "win", 4_000);
        none.telemetry = Some(GameTelemetry::default());
        assert!(stats_helper_book_is_none(&[none]));
    }

    fn stats_helper_book_is_none(records: &[GameRecord]) -> bool {
        stats(records).lab.expect("lab").book.is_none()
    }

    #[test]
    fn groups_config_cohorts_by_fingerprint() {
        let mut alpha_one = record("a", "win", 1_000);
        alpha_one.telemetry = Some(GameTelemetry {
            config_fingerprint: Some("aaaaaaaaaaaa".into()),
            ..GameTelemetry::default()
        });
        let mut alpha_two = record("b", "loss", 3_000);
        alpha_two.telemetry = Some(GameTelemetry {
            config_fingerprint: Some("aaaaaaaaaaaa".into()),
            ..GameTelemetry::default()
        });
        let mut beta = record("c", "draw", 2_000);
        beta.engine_name = Some("Other".into());
        beta.telemetry = Some(GameTelemetry {
            config_fingerprint: Some("bbbbbbbbbbbb".into()),
            ..GameTelemetry::default()
        });
        let mut unfingerprinted = record("d", "win", 4_000);
        unfingerprinted.telemetry = Some(GameTelemetry::default());

        let stats = stats(&[alpha_one, alpha_two, beta, unfingerprinted]);
        let by_config = stats.lab.expect("lab").by_config;
        assert_eq!(by_config.len(), 2);
        assert_eq!(by_config[0].fingerprint, "aaaaaaaaaaaa");
        assert_eq!(by_config[0].games, 2);
        assert_eq!(by_config[0].engine_name, "Stockfish");
        assert!((by_config[0].score_percent - 50.0).abs() < f64::EPSILON);
        assert_eq!(by_config[0].first_seen_ms, 1_000);
        assert_eq!(by_config[0].last_seen_ms, 3_000);
        assert_eq!(by_config[1].fingerprint, "bbbbbbbbbbbb");
        assert_eq!(by_config[1].games, 1);
        assert_eq!(by_config[1].engine_name, "Other");
    }

    const DAY: i64 = super::DAY_MS;
    /// 2024-01-01 00:00 UTC, a Monday (epoch day 19723).
    const MONDAY_2024_01_01: i64 = 19_723 * DAY;

    #[test]
    fn time_window_composes_with_filters_but_activity_keeps_the_full_span() {
        let mut early = record("a", "win", MONDAY_2024_01_01);
        early.telemetry = Some(GameTelemetry::default());
        let inside = record("b", "loss", MONDAY_2024_01_01 + 5 * DAY);
        let mut edge = record("c", "draw", MONDAY_2024_01_01 + 9 * DAY);
        edge.telemetry = Some(GameTelemetry::default());
        let mut bullet = record("d", "win", MONDAY_2024_01_01 + 5 * DAY);
        bullet.perf = "bullet".into();

        let stats = compute_stats(
            &[early, inside, edge, bullet],
            &ScorebookFilter {
                perf: Some("blitz".into()),
                from_ms: Some(MONDAY_2024_01_01 + 4 * DAY),
                to_ms: Some(MONDAY_2024_01_01 + 9 * DAY),
                ..ScorebookFilter::default()
            },
            Vec::new(),
            Vec::new(),
        );
        // The window excludes "a" from every aggregation…
        assert_eq!(stats.total_games, 2);
        assert_eq!(stats.losses, 1);
        assert_eq!(stats.draws, 1);
        assert_eq!(stats.recorded, 2);
        assert_eq!(stats.lab.expect("lab").telemetry_games, 1);
        // …but activity spans the whole perf-filtered history ("a" included,
        // the bullet game excluded).
        assert_eq!(stats.activity_bucket, "day");
        assert_eq!(stats.activity.len(), 10);
        assert_eq!(stats.activity[0].day_start_ms, MONDAY_2024_01_01);
        assert_eq!(stats.activity[0].games, 1);
        assert_eq!(stats.activity[5].games, 1);
        assert_eq!(stats.activity[9].games, 1);
        assert_eq!(stats.activity.iter().map(|line| line.games).sum::<i64>(), 3);
        // Empty buckets between records are present and zeroed.
        assert_eq!(stats.activity[1].games, 0);
        assert_eq!(stats.activity[8].games, 0);
    }

    #[test]
    fn window_edges_are_inclusive() {
        let records = vec![
            record("a", "win", 1_000),
            record("b", "draw", 2_000),
            record("c", "loss", 3_000),
        ];
        let exact = compute_stats(
            &records,
            &ScorebookFilter {
                from_ms: Some(2_000),
                to_ms: Some(2_000),
                ..ScorebookFilter::default()
            },
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(exact.total_games, 1);
        assert_eq!(exact.draws, 1);
        let full = compute_stats(
            &records,
            &ScorebookFilter {
                from_ms: Some(1_000),
                to_ms: Some(3_000),
                ..ScorebookFilter::default()
            },
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(full.total_games, 3);
    }

    #[test]
    fn activity_coarsens_from_day_to_week_to_month() {
        let ten_days = stats(&[
            record("a", "win", MONDAY_2024_01_01),
            record("b", "loss", MONDAY_2024_01_01 + 9 * DAY),
        ]);
        assert_eq!(ten_days.activity_bucket, "day");
        assert_eq!(ten_days.activity.len(), 10);

        // 365 daily buckets is too many; 53 Monday-start weeks fit.
        let one_year = stats(&[
            record("a", "win", MONDAY_2024_01_01),
            record("b", "loss", MONDAY_2024_01_01 + 364 * DAY),
        ]);
        assert_eq!(one_year.activity_bucket, "week");
        assert_eq!(one_year.activity.len(), 53);

        // ~261 weeks is too many; 60 calendar months fit.
        let five_years = stats(&[
            record("a", "win", MONDAY_2024_01_01),
            record("b", "loss", MONDAY_2024_01_01 + 5 * 365 * DAY),
        ]);
        assert_eq!(five_years.activity_bucket, "month");
        assert_eq!(five_years.activity.len(), 60);
    }

    #[test]
    fn weekly_buckets_start_on_utc_mondays() {
        let stats = stats(&[
            record("a", "win", MONDAY_2024_01_01),
            // Sunday of the same week: lands in the Monday bucket.
            record("b", "draw", MONDAY_2024_01_01 + 6 * DAY + 12 * 3_600_000),
            record("c", "loss", MONDAY_2024_01_01 + 364 * DAY),
        ]);
        assert_eq!(stats.activity_bucket, "week");
        for line in &stats.activity {
            assert_eq!(line.day_start_ms.rem_euclid(DAY), 0);
            // Monday = 0 with the epoch (a Thursday) offset by 3.
            assert_eq!((line.day_start_ms / DAY + 3).rem_euclid(7), 0);
        }
        assert_eq!(stats.activity[0].day_start_ms, MONDAY_2024_01_01);
        assert_eq!(stats.activity[0].games, 2);
        assert!((stats.activity[0].score_points - 1.5).abs() < f64::EPSILON);
        assert_eq!(stats.activity[1].games, 0);
        assert_eq!(stats.activity[52].games, 1);
    }

    #[test]
    fn monthly_buckets_follow_utc_calendar_months() {
        // 2024-02-01 00:00 UTC is epoch day 19723 + 31.
        let february_start = (19_723 + 31) * DAY;
        let stats = stats(&[
            // Last millisecond of January 2024.
            record("a", "win", february_start - 1),
            // First millisecond of February 2024.
            record("b", "draw", february_start),
            record("c", "loss", MONDAY_2024_01_01 + 5 * 365 * DAY),
        ]);
        assert_eq!(stats.activity_bucket, "month");
        assert_eq!(stats.activity.len(), 60);
        assert_eq!(stats.activity[0].day_start_ms, MONDAY_2024_01_01);
        assert_eq!(stats.activity[0].games, 1);
        assert_eq!(stats.activity[1].day_start_ms, february_start);
        assert_eq!(stats.activity[1].games, 1);
        // March 2024 starts 29 days after February (2024 is a leap year).
        assert_eq!(stats.activity[2].day_start_ms, february_start + 29 * DAY);
        assert_eq!(stats.activity[2].games, 0);
        // Last bucket is December 2028, holding the span-ending record.
        assert_eq!(stats.activity[59].games, 1);
    }

    #[test]
    fn week_and_month_starts_handle_epoch_and_pre_epoch_dates() {
        use super::{month_start_ms, week_start_ms};
        // Monday stays put; any later moment of the week maps back to it.
        assert_eq!(week_start_ms(MONDAY_2024_01_01), MONDAY_2024_01_01);
        assert_eq!(
            week_start_ms(MONDAY_2024_01_01 + 6 * DAY + 5),
            MONDAY_2024_01_01
        );
        // Sunday 1970-01-04 belongs to the pre-epoch Monday 1969-12-29.
        assert_eq!(week_start_ms(3 * DAY + 1), -3 * DAY);
        // Leap-day February 2024 and end-of-year December 2023.
        assert_eq!(month_start_ms((19_723 + 59) * DAY), (19_723 + 31) * DAY);
        assert_eq!(month_start_ms(19_722 * DAY + 7), (19_722 - 30) * DAY);
    }

    #[test]
    fn flags_time_losses_that_were_still_winning() {
        let mut flagged = record("a", "loss", 1_000);
        flagged.status = "outoftime".into();
        flagged.telemetry = Some(telemetry(400, 100, vec![400, 200]));
        let mut deserved = record("b", "loss", 2_000);
        deserved.status = "timeout".into();
        deserved.telemetry = Some(telemetry(50, -300, vec![50, -300]));
        let mut normal_loss = record("c", "loss", 3_000);
        normal_loss.telemetry = Some(telemetry(300, 150, vec![300, 150]));

        let stats = stats(&[flagged, deserved, normal_loss]);
        let lab = stats.lab.expect("lab");
        assert_eq!(lab.flagged_winning, 1);
    }

    #[test]
    fn sums_reliability_totals_and_averages() {
        let mut first = record("a", "win", 1_000);
        first.telemetry = Some(GameTelemetry {
            eval_series_cp: vec![250, 40, 300],
            engine_restarts: 1,
            submission_retries: 2,
            stream_reconnects: 3,
            flag_safety_stops: 4,
            failure_resign: true,
            end_clock_ms: Some(10_000),
            blunders: 1,
            ..GameTelemetry::default()
        });
        let mut second = record("b", "loss", 2_000);
        second.telemetry = Some(GameTelemetry {
            eval_series_cp: vec![0],
            submission_retries: 1,
            end_clock_ms: Some(20_000),
            blunders: 3,
            ..GameTelemetry::default()
        });

        let stats = stats(&[first, second]);
        let lab = stats.lab.expect("lab");
        assert_eq!(lab.reliability.engine_restarts, 1);
        assert_eq!(lab.reliability.submission_retries, 3);
        assert_eq!(lab.reliability.stream_reconnects, 3);
        assert_eq!(lab.reliability.flag_safety_stops, 4);
        assert_eq!(lab.reliability.failure_resigns, 1);
        assert!((lab.avg_end_clock_ms.expect("end clock") - 15_000.0).abs() < f64::EPSILON);
        assert!((lab.avg_blunders_per_game.expect("blunders") - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn reliability_totals_accept_an_older_runner_without_flag_safety_stops() {
        let totals: ReliabilityTotals = serde_json::from_value(serde_json::json!({
            "engineRestarts": 1,
            "submissionRetries": 2,
            "streamReconnects": 3,
            "failureResigns": 4
        }))
        .expect("legacy reliability totals");
        assert_eq!(totals.flag_safety_stops, 0);
    }
}
