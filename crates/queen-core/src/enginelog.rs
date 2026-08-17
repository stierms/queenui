//! Engine flight recorder: the complete UCI conversation of every game,
//! streamed to a gzip file with a searchable metadata index.
//!
//! Layout under the app data directory:
//!
//! ```text
//! logs/index.jsonl                   one LogSessionSummary per line
//! logs/sessions/<session-id>.uci.gz  header block + timestamped event lines
//! ```
//!
//! A session file starts with a header block of `# key: value` lines and then
//! carries one event per line:
//!
//! ```text
//! <ms-since-session-start>\t<dir>\t<text>
//! ```
//!
//! `<dir>` is `>` (sent to the engine), `<` (engine stdout), `!` (engine
//! stderr) or `#` (a QueenUI note). Only the first two tabs separate fields, so
//! engine text containing tabs survives untouched. Notes are the spine the
//! outline is built from and use stable shapes:
//!
//! ```text
//! search ply=<n> move=<n> color=<w|b> wtime=<ms> btime=<ms> winc=<ms> binc=<ms>
//! bestmove uci=<move> elapsed=<ms>
//! book move=<uci>
//! engine-restart reason=<text>
//! meta <key>: <value>          metadata that only became known mid-game
//! session-end status=<status> result=<result>
//! ```
//!
//! These are real rated games, so nothing here may delay a search: the writer
//! entry points only stamp a timestamp and push onto a channel, and a dedicated
//! OS thread does every gzip and file operation. The stream is flushed once per
//! completed move, which bounds what a crash can cost to the move in progress
//! and makes a live session readable while it is still being written.

use crate::diagnostics::{self, DiagnosticEntry};
use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{ErrorKind, Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
        Arc, Mutex, MutexGuard,
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::oneshot;

const LOGS_DIR: &str = "logs";
const SESSIONS_DIR: &str = "sessions";
const INDEX_FILE: &str = "index.jsonl";
const SESSION_SUFFIX: &str = ".uci.gz";
/// Queued events past this point are dropped and counted rather than letting a
/// stalled disk grow the channel until the app runs out of memory.
const MAX_QUEUED_EVENTS: u64 = 50_000;
const DAY_MS: u64 = 86_400_000;
const BYTES_PER_MB: u64 = 1024 * 1024;
/// Guards the collision-suffix loop against a wedged sessions directory.
const MAX_ID_ATTEMPTS: u32 = 500;
const DECODE_CHUNK: usize = 64 * 1024;
/// How recently a session file must have been touched to count as owned by a
/// process that is still writing it. Nothing on disk distinguishes a crashed
/// session from one a second QueenUI window has open right now, so age is the
/// only signal: the writer flushes on every completed move, and a file that has
/// not moved for this long belongs to a process that is gone. The known cost is
/// a correspondence game thinking longer than the window — its file can be
/// adopted, and then pruned, by another window.
const LIVE_FILE_WINDOW_MS: u64 = 5 * 60 * 1_000;

const SENT: char = '>';
const RECEIVED: char = '<';
const STDERR: char = '!';
const NOTE: char = '#';

const KEY_FORMAT: &str = "queenui-log";
const KEY_SESSION: &str = "session";
const KEY_STARTED: &str = "started";
const KEY_STARTED_MS: &str = "started-ms";
const KEY_KIND: &str = "kind";
const KEY_GAME: &str = "game";
const KEY_ACCOUNT: &str = "account";
const KEY_BOT: &str = "bot";
const KEY_OPPONENT: &str = "opponent";
const KEY_OPPONENT_RATING: &str = "opponent-rating";
const KEY_ENGINE_ID: &str = "engine-id";
const KEY_ENGINE: &str = "engine";
const KEY_ENGINE_PATH: &str = "engine-path";
const KEY_COLOR: &str = "color";
const KEY_CLOCK: &str = "clock";
const KEY_INITIAL_FEN: &str = "initial-fen";
const KEY_BOOK: &str = "book";
const KEY_APP_VERSION: &str = "app-version";
const KEY_OPTION: &str = "option";

/// How much recording is allowed to keep. Either cap set to zero disables it.
#[derive(Clone, Debug, Deserialize, Serialize, ts_rs::TS)]
#[serde(default, rename_all = "camelCase")]
pub struct LogRetention {
    pub capture_enabled: bool,
    pub max_total_mb: u64,
    pub max_age_days: u32,
}

impl Default for LogRetention {
    fn default() -> Self {
        Self {
            capture_enabled: true,
            max_total_mb: 2048,
            max_age_days: 90,
        }
    }
}

/// Everything known about a session when it opens. The engine subprocess starts
/// before Lichess has described the game, so the game-side fields are optional
/// here and filled in later through `LogWriter::describe`.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMeta {
    /// "game" | "probe"
    pub kind: String,
    pub game_id: Option<String>,
    pub account_id: String,
    pub bot_username: String,
    pub opponent: Option<String>,
    pub opponent_rating: Option<i64>,
    pub engine_id: String,
    pub engine_name: String,
    pub engine_path: String,
    pub color: Option<String>,
    /// "180+2"
    pub clock: Option<String>,
    pub initial_fen: Option<String>,
    /// The UCI options actually applied to this process.
    pub options: Vec<(String, String)>,
    pub book: Option<String>,
    pub app_version: String,
}

/// Metadata that only becomes known once the game stream opens. `None` fields
/// leave the current value alone.
#[derive(Clone, Debug, Default)]
pub struct SessionDescription {
    pub opponent: Option<String>,
    pub opponent_rating: Option<i64>,
    pub color: Option<String>,
    pub clock: Option<String>,
    pub initial_fen: Option<String>,
}

/// The index record for one session. Written to `index.jsonl` when the session
/// finishes; live sessions exist only in memory until then.
#[derive(Clone, Debug, Default, Deserialize, Serialize, ts_rs::TS)]
#[serde(default, rename_all = "camelCase")]
pub struct LogSessionSummary {
    pub id: String,
    pub kind: String,
    pub game_id: Option<String>,
    pub account_id: String,
    pub bot_username: String,
    pub opponent: Option<String>,
    pub engine_id: String,
    pub engine_name: String,
    pub color: Option<String>,
    pub clock: Option<String>,
    pub started_at_ms: u64,
    pub finished_at_ms: Option<u64>,
    pub status: Option<String>,
    pub result: Option<String>,
    pub line_count: u64,
    pub search_count: u32,
    pub compressed_bytes: u64,
    pub raw_bytes: u64,
    pub dropped_lines: u64,
    pub live: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, ts_rs::TS)]
#[serde(default, rename_all = "camelCase")]
pub struct LogFilter {
    pub account_id: Option<String>,
    pub engine_id: Option<String>,
    pub from_ms: Option<u64>,
    pub to_ms: Option<u64>,
    /// Case-insensitive substring over opponent, game id, engine name and bot
    /// username.
    pub query: Option<String>,
    pub limit: Option<usize>,
}

/// A line search. `limit` of zero means uncapped, as it does everywhere in this
/// module: a UI that has not chosen a page size wants everything.
#[derive(Clone, Debug, Default, Deserialize, Serialize, ts_rs::TS)]
#[serde(default, rename_all = "camelCase")]
pub struct LogQuery {
    pub text: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub limit: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    pub index: u64,
    pub at_ms: u64,
    pub direction: String,
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct LogHeaderField {
    pub key: String,
    pub value: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct LogPage {
    pub session_id: String,
    pub total_lines: u64,
    pub offset: u64,
    pub lines: Vec<LogLine>,
    pub header: Vec<LogHeaderField>,
    pub live: bool,
}

/// One search, from its `search` note to its `bestmove` note (or to the last
/// line before the next search, when the game ended mid-think).
#[derive(Clone, Debug, Deserialize, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct LogSearchBlock {
    pub move_number: u32,
    pub ply: u32,
    pub color: String,
    pub start_line: u64,
    pub end_line: u64,
    pub best_move: Option<String>,
    pub elapsed_ms: Option<u64>,
    pub depth: Option<u32>,
    pub score_cp: Option<i32>,
    pub mate_in: Option<i32>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct LogMatch {
    pub line_index: u64,
    pub direction: String,
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct LogSessionMatches {
    pub session: LogSessionSummary,
    pub match_count: u64,
    pub first: Option<LogMatch>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct LogsOverview {
    pub session_count: u64,
    pub compressed_bytes: u64,
    pub raw_bytes: u64,
    pub oldest_started_at_ms: Option<u64>,
    pub live_count: u64,
    pub retention: LogRetention,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, ts_rs::TS)]
#[serde(rename_all = "lowercase")]
pub enum ExportMode {
    /// The decompressed file verbatim: header plus timestamped event lines.
    Annotated,
    /// Only the engine conversation, unprefixed, so it can be replayed.
    Plain,
    /// The `.gz` byte for byte.
    Archive,
}

impl ExportMode {
    /// Parses the string the command layer receives from the frontend.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "annotated" => Ok(Self::Annotated),
            "plain" => Ok(Self::Plain),
            "archive" => Ok(Self::Archive),
            other => Err(format!("{other} is not a log export mode.")),
        }
    }
}

impl std::str::FromStr for ExportMode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

/// Counters a live session publishes so the UI can describe it before it ends.
/// Only the writer thread stores into them, so relaxed ordering is enough: a
/// reader that sees a slightly stale line count loses nothing.
#[derive(Default)]
struct LiveCounters {
    queued: AtomicU64,
    dropped: AtomicU64,
    lines: AtomicU64,
    searches: AtomicU64,
    raw_bytes: AtomicU64,
    compressed_bytes: AtomicU64,
}

impl LiveCounters {
    fn apply(&self, summary: &mut LogSessionSummary) {
        summary.line_count = self.lines.load(Ordering::Relaxed);
        summary.search_count = clamp_u32(self.searches.load(Ordering::Relaxed));
        summary.raw_bytes = self.raw_bytes.load(Ordering::Relaxed);
        summary.compressed_bytes = self.compressed_bytes.load(Ordering::Relaxed);
        summary.dropped_lines = self.dropped.load(Ordering::Relaxed);
    }
}

/// The store's handle on a session that is still recording. The summary is
/// shared with the writer so `describe` and `finish` agree on one copy.
struct LiveSession {
    summary: Arc<Mutex<LogSessionSummary>>,
    counters: Arc<LiveCounters>,
}

#[derive(Default)]
struct IndexState {
    /// Finished sessions, in the order `index.jsonl` holds them.
    entries: Vec<LogSessionSummary>,
    live: HashMap<String, LiveSession>,
}

/// A decompressed session, cached whole so paging, outline and search share one
/// decode.
struct SessionData {
    header: Vec<LogHeaderField>,
    lines: Vec<LogLine>,
    /// Compressed length this was decoded from. A live session grows, so the
    /// cache is only valid while the file it came from has not changed.
    file_len: u64,
    /// Decompressed length. The writer fed the encoder exactly these bytes, so
    /// this reproduces its `raw_bytes` for a session it never got to record.
    decoded_bytes: u64,
}

/// The disk half of an index mutation, computed under `state` and performed
/// afterwards so no file work happens while a game start waits for the lock.
enum IndexWrite {
    /// The mutation changed nothing on disk.
    Nothing,
    /// One new record; appending beats rewriting the whole index for it.
    Append(Box<LogSessionSummary>),
    /// Session files to remove and the index to rewrite from scratch. The
    /// entries were snapshotted after the mutation, so a record another thread
    /// added first is inside them rather than overwritten by them.
    Rewrite {
        doomed: Vec<String>,
        entries: Vec<LogSessionSummary>,
    },
}

struct StoreInner {
    sessions_dir: PathBuf,
    index_path: PathBuf,
    state: Mutex<IndexState>,
    /// Serialises index writes and session-file deletions. Always taken before
    /// `state` and held across the whole mutation, never the other way round:
    /// that ordering is what keeps an append from being lost by a rewrite, and
    /// what keeps a bulk delete out of the lock `open_session` needs.
    io: Mutex<()>,
    retention: Mutex<LogRetention>,
    cache: Mutex<Option<(String, Arc<SessionData>)>>,
    /// Sessions rebuilt from a file this process never recorded. Another
    /// QueenUI window may still own them, so they are the only records the
    /// freshness guard applies to.
    adopted: HashSet<String>,
}

/// The engine flight recorder. Cloning shares one store.
#[derive(Clone)]
pub struct EngineLogStore {
    inner: Arc<StoreInner>,
}

enum LogEvent {
    Line {
        at_ms: u64,
        direction: char,
        text: String,
    },
    Finish {
        at_ms: u64,
        status: String,
        result: Option<String>,
        reply: oneshot::Sender<Option<LogSessionSummary>>,
    },
}

struct WriterShared {
    sender: Sender<LogEvent>,
    started: Instant,
    counters: Arc<LiveCounters>,
    summary: Arc<Mutex<LogSessionSummary>>,
    /// What the file already says about the late metadata fields, seeded from
    /// the header block. `describe` writes a note only for what this does not
    /// already hold.
    described: Mutex<SessionDescription>,
    finished: AtomicBool,
}

/// A cheap, clonable handle on a recording session. Every method is
/// synchronous, allocation-light and non-blocking; the work happens on the
/// session's own thread.
#[derive(Clone)]
pub struct LogWriter {
    shared: Arc<WriterShared>,
}

impl EngineLogStore {
    /// Creates `logs/` and `logs/sessions/` if absent and loads the index. An
    /// unreadable index yields an empty one: a broken log must never stop
    /// QueenUI from starting, let alone from playing.
    pub fn load(app_data_dir: &Path, retention: LogRetention) -> Self {
        let root = app_data_dir.join(LOGS_DIR);
        let sessions_dir = root.join(SESSIONS_DIR);
        if let Err(error) = fs::create_dir_all(&sessions_dir) {
            diagnostics::record(
                DiagnosticEntry::error("storage", "Could not create the engine-log directory")
                    .with_detail(format!("{}: {error}", sessions_dir.display())),
            );
        }
        let index_path = root.join(INDEX_FILE);

        // Failures below go to the Diagnostics log, where an operator will look
        // for them; these startup counts are console bookkeeping and would only
        // crowd it out.
        let mut entries: Vec<LogSessionSummary> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut corrupt = 0usize;
        // Set by anything that makes the file disagree with what we ended up
        // with, so the reconciliation is paid for once instead of every launch.
        let mut stale_index = false;
        if let Ok(content) = fs::read_to_string(&index_path) {
            for line in content.lines() {
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<LogSessionSummary>(line) {
                    Ok(mut summary) if !summary.id.is_empty() => {
                        // Only sessions opened by this process are live; a live
                        // flag on disk is a leftover from a crash.
                        stale_index |= summary.live;
                        summary.live = false;
                        if seen.insert(summary.id.clone()) {
                            entries.push(summary);
                        } else {
                            stale_index = true;
                        }
                    }
                    _ => corrupt += 1,
                }
            }
        }
        if corrupt > 0 {
            stale_index = true;
            crate::diagnostics::record(
                crate::diagnostics::DiagnosticEntry::warn(
                    "storage",
                    format!("Skipped {corrupt} corrupt engine-log index line(s)"),
                )
                .with_detail(index_path.display().to_string()),
            );
        }
        // A record whose file was deleted outside QueenUI would only offer the
        // operator a session that cannot be opened.
        let before = entries.len();
        entries.retain(|entry| {
            sessions_dir
                .join(format!("{}{SESSION_SUFFIX}", entry.id))
                .exists()
        });
        if entries.len() < before {
            stale_index = true;
            crate::diagnostics::record(crate::diagnostics::DiagnosticEntry::warn(
                "storage",
                format!(
                    "Dropped {} engine-log record(s) whose file is gone",
                    before - entries.len()
                ),
            ));
        }
        let recovered = adopt_orphans(&sessions_dir, &entries);
        let adopted: HashSet<String> = recovered.iter().map(|entry| entry.id.clone()).collect();
        if !recovered.is_empty() {
            stale_index = true;
            entries.extend(recovered);
        }
        if stale_index {
            // Writing the reconciled view back is what makes this a one-time
            // cost: leaving the dangling and corrupt lines in place would drop
            // them again, and warn about them again, on every launch.
            write_index(&index_path, &entries);
        }
        crate::diagnostics::record(crate::diagnostics::DiagnosticEntry::info(
            "app",
            format!("Loaded {} engine-log session(s)", entries.len()),
        ));

        Self {
            inner: Arc::new(StoreInner {
                sessions_dir,
                index_path,
                state: Mutex::new(IndexState {
                    entries,
                    live: HashMap::new(),
                }),
                io: Mutex::new(()),
                retention: Mutex::new(retention),
                cache: Mutex::new(None),
                adopted,
            }),
        }
    }

    /// Opens a session and starts its writer thread. `None` when capture is
    /// disabled or the file could not be created — a game must go on without
    /// its recording, never the other way round.
    pub fn open_session(&self, meta: SessionMeta) -> Option<LogWriter> {
        if !lock(&self.inner.retention).capture_enabled {
            return None;
        }
        if let Err(error) = fs::create_dir_all(&self.inner.sessions_dir) {
            diagnostics::record(
                DiagnosticEntry::error("storage", "Could not create the engine-log directory")
                    .with_detail(error.to_string()),
            );
            return None;
        }

        let started_at_ms = now_ms();
        let base = sanitize_id(&format!(
            "{}_{}_{}",
            compact_timestamp(started_at_ms),
            meta.bot_username,
            meta.game_id.as_deref().unwrap_or(&meta.kind)
        ));

        // Only ids this process has open can collide in memory, and there are at
        // most a handful of them; every id already on disk is fenced off by the
        // `create_new` in `reserve_session_file`. Cloning the whole index here
        // would put twenty thousand allocations on the path of a game start.
        let live_ids: HashSet<String> = lock(&self.inner.state).live.keys().cloned().collect();
        let (session_id, file) = reserve_session_file(&self.inner.sessions_dir, &base, &live_ids)?;

        let start = {
            let mut state = lock(&self.inner.state);
            let summary = Arc::new(Mutex::new(LogSessionSummary {
                id: session_id.clone(),
                kind: meta.kind.clone(),
                game_id: meta.game_id.clone(),
                account_id: meta.account_id.clone(),
                bot_username: meta.bot_username.clone(),
                opponent: meta.opponent.clone(),
                engine_id: meta.engine_id.clone(),
                engine_name: meta.engine_name.clone(),
                color: meta.color.clone(),
                clock: meta.clock.clone(),
                started_at_ms,
                live: true,
                ..LogSessionSummary::default()
            }));
            let counters = Arc::new(LiveCounters::default());
            state.live.insert(
                session_id.clone(),
                LiveSession {
                    summary: summary.clone(),
                    counters: counters.clone(),
                },
            );
            SessionStart {
                session_id,
                file,
                started_at_ms,
                started: Instant::now(),
                summary,
                counters,
            }
        };

        let session_id = start.session_id.clone();
        match self.spawn_writer(start, &meta) {
            Some(writer) => Some(writer),
            None => {
                lock(&self.inner.state).live.remove(&session_id);
                let _ = fs::remove_file(self.inner.session_path(&session_id));
                None
            }
        }
    }

    pub fn list(&self, filter: &LogFilter) -> Vec<LogSessionSummary> {
        let mut sessions: Vec<LogSessionSummary> = self
            .inner
            .snapshot()
            .into_iter()
            .filter(|summary| matches_filter(summary, filter))
            .collect();
        sessions.sort_by(|left, right| {
            right
                .started_at_ms
                .cmp(&left.started_at_ms)
                .then_with(|| right.id.cmp(&left.id))
        });
        if let Some(limit) = filter.limit {
            sessions.truncate(limit);
        }
        sessions
    }

    /// A window of lines starting at `offset`. `limit` of zero is uncapped, as
    /// it is everywhere in this module: every line from `offset` to the end.
    pub fn page(&self, session_id: &str, offset: u64, limit: u64) -> Result<LogPage, String> {
        let data = self.inner.read_session(session_id)?;
        let total_lines = data.lines.len() as u64;
        let start = offset.min(total_lines) as usize;
        let end = if limit == 0 {
            data.lines.len()
        } else {
            data.lines.len().min(start.saturating_add(limit as usize))
        };
        Ok(LogPage {
            session_id: session_id.to_string(),
            total_lines,
            offset,
            lines: data.lines[start..end].to_vec(),
            header: data.header.clone(),
            live: self.inner.is_live(session_id),
        })
    }

    pub fn outline(&self, session_id: &str) -> Result<Vec<LogSearchBlock>, String> {
        let data = self.inner.read_session(session_id)?;
        Ok(build_outline(&data.lines))
    }

    pub fn search(&self, session_id: &str, query: &LogQuery) -> Result<Vec<LogMatch>, String> {
        if query.text.is_empty() {
            return Ok(Vec::new());
        }
        let matcher = Matcher::build(query)?;
        let data = self.inner.read_session(session_id)?;
        let mut matches = Vec::new();
        for line in &data.lines {
            if !matcher.is_match(&line.text) {
                continue;
            }
            matches.push(LogMatch {
                line_index: line.index,
                direction: line.direction.clone(),
                text: line.text.clone(),
            });
            if query.limit > 0 && matches.len() as u64 >= query.limit {
                break;
            }
        }
        Ok(matches)
    }

    /// Sweeps the filtered sessions newest first. `limit` caps how many
    /// sessions with hits are returned, not how many lines each contributes.
    pub fn search_all(
        &self,
        filter: &LogFilter,
        query: &LogQuery,
    ) -> Result<Vec<LogSessionMatches>, String> {
        if query.text.is_empty() {
            return Ok(Vec::new());
        }
        let matcher = Matcher::build(query)?;
        let mut hits = Vec::new();
        for session in self.list(filter) {
            if query.limit > 0 && hits.len() as u64 >= query.limit {
                break;
            }
            // One unreadable session must not fail the whole sweep, and no
            // session it touches may displace what the operator has open.
            let Ok(data) = self.inner.scan_session(&session.id) else {
                continue;
            };
            let mut match_count = 0u64;
            let mut first = None;
            for line in &data.lines {
                if !matcher.is_match(&line.text) {
                    continue;
                }
                match_count += 1;
                if first.is_none() {
                    first = Some(LogMatch {
                        line_index: line.index,
                        direction: line.direction.clone(),
                        text: line.text.clone(),
                    });
                }
            }
            if match_count > 0 {
                hits.push(LogSessionMatches {
                    session,
                    match_count,
                    first,
                });
            }
        }
        Ok(hits)
    }

    pub fn export(
        &self,
        session_id: &str,
        destination: &Path,
        mode: ExportMode,
    ) -> Result<(), String> {
        if let Some(parent) = destination.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)
                    .map_err(|error| format!("Could not create the export folder: {error}"))?;
            }
        }
        let source = self.inner.session_path(session_id);
        match mode {
            ExportMode::Archive => fs::copy(&source, destination)
                .map(|_| ())
                .map_err(|error| format!("Could not export the log archive: {error}")),
            ExportMode::Annotated => {
                let bytes = decode_lossy(&source)?;
                fs::write(destination, bytes)
                    .map_err(|error| format!("Could not write the exported log: {error}"))
            }
            ExportMode::Plain => {
                let data = self.inner.read_session(session_id)?;
                let mut transcript = String::new();
                for line in &data.lines {
                    if direction_is(line, SENT) || direction_is(line, RECEIVED) {
                        transcript.push_str(&line.text);
                        transcript.push('\n');
                    }
                }
                fs::write(destination, transcript)
                    .map_err(|error| format!("Could not write the exported log: {error}"))
            }
        }
    }

    pub fn export_bytes(&self, session_id: &str, mode: ExportMode) -> Result<Vec<u8>, String> {
        self.export_bytes_bounded(session_id, mode, usize::MAX)
    }

    pub fn export_bytes_bounded(
        &self,
        session_id: &str,
        mode: ExportMode,
        max_bytes: usize,
    ) -> Result<Vec<u8>, String> {
        let source = self.inner.session_path(session_id);
        match mode {
            ExportMode::Archive => {
                if fs::metadata(&source)
                    .map(|metadata| metadata.len() > max_bytes as u64)
                    .unwrap_or(true)
                {
                    return Err("The log export exceeds the server byte cap".into());
                }
                fs::read(&source)
                    .map_err(|error| format!("Could not read the log archive: {error}"))
            }
            ExportMode::Annotated => decode_lossy_bounded(&source, max_bytes),
            ExportMode::Plain => {
                let data = self.inner.read_session(session_id)?;
                if data.decoded_bytes > max_bytes as u64 {
                    return Err("The log export exceeds the server byte cap".into());
                }
                let mut transcript = String::new();
                for line in &data.lines {
                    if direction_is(line, SENT) || direction_is(line, RECEIVED) {
                        if transcript
                            .len()
                            .saturating_add(line.text.len())
                            .saturating_add(1)
                            > max_bytes
                        {
                            return Err("The log export exceeds the server byte cap".into());
                        }
                        transcript.push_str(&line.text);
                        transcript.push('\n');
                    }
                }
                Ok(transcript.into_bytes())
            }
        }
    }

    pub fn delete(&self, session_id: &str) -> Result<(), String> {
        let guarded = self.inner.guarded_ids();
        self.inner.with_index(|state| {
            if state.live.contains_key(session_id) {
                return (
                    Err("That session is still recording.".into()),
                    IndexWrite::Nothing,
                );
            }
            if !state.entries.iter().any(|entry| entry.id == session_id) {
                return (
                    Err("That log session no longer exists.".into()),
                    IndexWrite::Nothing,
                );
            }
            if guarded.contains(session_id) {
                return (
                    Err("Another QueenUI window still seems to be recording that session.".into()),
                    IndexWrite::Nothing,
                );
            }
            state.entries.retain(|entry| entry.id != session_id);
            (
                Ok(()),
                IndexWrite::Rewrite {
                    doomed: vec![session_id.to_string()],
                    entries: state.entries.clone(),
                },
            )
        })
    }

    /// Removes every non-live session; returns how many were removed. A session
    /// another QueenUI window still seems to be writing is left alone.
    pub fn clear(&self) -> Result<u64, String> {
        let guarded = self.inner.guarded_ids();
        Ok(self.inner.with_index(|state| {
            let doomed: Vec<String> = state
                .entries
                .iter()
                .map(|entry| entry.id.clone())
                .filter(|id| !guarded.contains(id))
                .collect();
            (doomed.len() as u64, remove_doomed(state, doomed))
        }))
    }

    pub fn overview(&self) -> LogsOverview {
        let sessions = self.inner.snapshot();
        LogsOverview {
            session_count: sessions.len() as u64,
            compressed_bytes: sessions.iter().map(|s| s.compressed_bytes).sum(),
            raw_bytes: sessions.iter().map(|s| s.raw_bytes).sum(),
            oldest_started_at_ms: sessions.iter().map(|s| s.started_at_ms).min(),
            live_count: sessions.iter().filter(|s| s.live).count() as u64,
            retention: lock(&self.inner.retention).clone(),
        }
    }

    /// Stores the new policy and immediately prunes to it.
    pub fn set_retention(&self, retention: LogRetention) -> u64 {
        *lock(&self.inner.retention) = retention;
        self.prune()
    }

    /// Applies the age cap, then the size cap; returns how many sessions were
    /// removed. Live sessions are never touched.
    pub fn prune(&self) -> u64 {
        let retention = lock(&self.inner.retention).clone();
        self.inner.prune(&retention)
    }

    fn spawn_writer(&self, start: SessionStart, meta: &SessionMeta) -> Option<LogWriter> {
        let mut encoder = GzEncoder::new(start.file, Compression::default());
        let header = header_block(&start.session_id, start.started_at_ms, meta);
        if let Err(error) = encoder.write_all(header.as_bytes()) {
            diagnostics::record(
                DiagnosticEntry::error("storage", "Could not start the engine log")
                    .with_detail(error.to_string()),
            );
            return None;
        }
        // Flushing the header immediately means even a session that dies in its
        // first second still identifies itself on disk.
        let _ = encoder.flush();
        let raw_bytes = header.len() as u64;
        start.counters.raw_bytes.store(raw_bytes, Ordering::Relaxed);
        start
            .counters
            .compressed_bytes
            .store(file_len(encoder.get_ref()), Ordering::Relaxed);

        let (sender, receiver) = mpsc::channel();
        let context = WriterContext {
            store: self.inner.clone(),
            path: self.inner.session_path(&start.session_id),
            started: start.started,
            summary: start.summary.clone(),
            counters: start.counters.clone(),
        };
        // A plain OS thread rather than a runtime blocking task: a session lives
        // as long as its game, and parking a pooled worker for an hour would
        // starve the pool the rest of the app shares.
        std::thread::Builder::new()
            .name(format!("queenui-log-{}", start.session_id))
            .spawn(move || run_writer(encoder, receiver, context, raw_bytes))
            .map_err(|error| {
                diagnostics::record(
                    DiagnosticEntry::error("storage", "Could not start the engine-log writer")
                        .with_detail(error.to_string()),
                );
            })
            .ok()?;

        Some(LogWriter {
            shared: Arc::new(WriterShared {
                sender,
                started: start.started,
                counters: start.counters,
                summary: start.summary,
                described: Mutex::new(SessionDescription {
                    opponent: meta.opponent.clone(),
                    opponent_rating: meta.opponent_rating,
                    color: meta.color.clone(),
                    clock: meta.clock.clone(),
                    initial_fen: meta.initial_fen.clone(),
                }),
                finished: AtomicBool::new(false),
            }),
        })
    }
}

/// Everything `open_session` reserved before the writer thread exists.
struct SessionStart {
    session_id: String,
    file: File,
    started_at_ms: u64,
    started: Instant,
    summary: Arc<Mutex<LogSessionSummary>>,
    counters: Arc<LiveCounters>,
}

impl StoreInner {
    fn session_path(&self, session_id: &str) -> PathBuf {
        let session_id = if valid_session_id(session_id) {
            session_id
        } else {
            "__invalid_session_id__"
        };
        self.sessions_dir
            .join(format!("{session_id}{SESSION_SUFFIX}"))
    }

    fn is_live(&self, session_id: &str) -> bool {
        lock(&self.state).live.contains_key(session_id)
    }

    /// Every session, finished and live, with live counters folded in.
    fn snapshot(&self) -> Vec<LogSessionSummary> {
        let state = lock(&self.state);
        let mut sessions = state.entries.clone();
        for session in state.live.values() {
            let mut summary = lock(&session.summary).clone();
            session.counters.apply(&mut summary);
            sessions.push(summary);
        }
        sessions
    }

    /// Decodes a session on behalf of a reader, and keeps it: the next live-tail
    /// poll of the same session is then free.
    fn read_session(&self, session_id: &str) -> Result<Arc<SessionData>, String> {
        let data = self.scan_session(session_id)?;
        *lock(&self.cache) = Some((session_id.to_string(), data.clone()));
        Ok(data)
    }

    /// Decodes a session without claiming the cache for it. A sweep across every
    /// session would otherwise leave the cache holding whichever one it happened
    /// to visit last, throwing away the decode the operator is actually using.
    fn scan_session(&self, session_id: &str) -> Result<Arc<SessionData>, String> {
        let path = self.session_path(session_id);
        let file_len = fs::metadata(&path)
            .map(|metadata| metadata.len())
            .map_err(|error| format!("Could not open that log session: {error}"))?;
        if let Some((cached_id, data)) = lock(&self.cache).as_ref() {
            if cached_id == session_id && data.file_len == file_len {
                return Ok(data.clone());
            }
        }
        let bytes = decode_lossy(&path)?;
        Ok(Arc::new(parse_session(&bytes, file_len)))
    }

    fn forget_cached(&self, session_id: &str) {
        let mut cache = lock(&self.cache);
        if cache
            .as_ref()
            .is_some_and(|(cached_id, _)| cached_id == session_id)
        {
            *cache = None;
        }
    }

    /// Mutates the in-memory index and then performs whatever disk work the
    /// mutation asked for. The `state` lock is held for the in-memory half only:
    /// deleting a thousand files and fsyncing a rewritten index behind it would
    /// park whichever runtime worker is starting the next game.
    fn with_index<T>(&self, mutate: impl FnOnce(&mut IndexState) -> (T, IndexWrite)) -> T {
        // Taken before `state` and held across both halves. Two writers can then
        // never interleave, so a rewrite always carries the record an append put
        // there first instead of erasing it.
        let _io = lock(&self.io);
        let (value, write) = {
            let mut state = lock(&self.state);
            mutate(&mut state)
        };
        match write {
            IndexWrite::Nothing => {}
            IndexWrite::Append(summary) => match serde_json::to_string(&summary) {
                Ok(line) => append_line(&self.index_path, &line),
                Err(error) => diagnostics::record(
                    DiagnosticEntry::error("storage", "Could not serialize an engine-log record")
                        .with_detail(error.to_string()),
                ),
            },
            IndexWrite::Rewrite { doomed, entries } => {
                for id in &doomed {
                    let _ = fs::remove_file(self.session_path(id));
                    self.forget_cached(id);
                }
                write_index(&self.index_path, &entries);
            }
        }
        value
    }

    /// Adopted sessions whose file has been touched too recently to be anyone's
    /// leftovers. Adoption is rare, so this usually stats nothing at all.
    fn guarded_ids(&self) -> HashSet<String> {
        self.adopted
            .iter()
            .filter(|id| recently_modified(&self.session_path(id)))
            .cloned()
            .collect()
    }

    /// Moves a session from live to finished: records it and prunes to the
    /// current policy.
    fn complete_session(&self, summary: &LogSessionSummary) {
        let retention = lock(&self.retention).clone();
        let guarded = self.guarded_ids();
        self.with_index(|state| {
            state.live.remove(&summary.id);
            if state.entries.iter().any(|entry| entry.id == summary.id) {
                return ((), IndexWrite::Nothing);
            }
            state.entries.push(summary.clone());
            let doomed = doomed_ids(state, &retention, &guarded);
            if doomed.is_empty() {
                // Nothing else moved, so one appended line says everything a
                // full rewrite would have.
                return ((), IndexWrite::Append(Box::new(summary.clone())));
            }
            ((), remove_doomed(state, doomed))
        });
    }

    fn prune(&self, retention: &LogRetention) -> u64 {
        let guarded = self.guarded_ids();
        self.with_index(|state| {
            let doomed = doomed_ids(state, retention, &guarded);
            (doomed.len() as u64, remove_doomed(state, doomed))
        })
    }
}

/// The sessions the policy no longer keeps: the age cap first, then the size
/// cap. Live sessions, and sessions another window may still be writing, are
/// never candidates.
fn doomed_ids(
    state: &IndexState,
    retention: &LogRetention,
    guarded: &HashSet<String>,
) -> Vec<String> {
    let mut doomed: HashSet<String> = HashSet::new();
    if retention.max_age_days > 0 {
        let cutoff = now_ms().saturating_sub(u64::from(retention.max_age_days) * DAY_MS);
        for entry in state.entries.iter().filter(|e| e.started_at_ms < cutoff) {
            if !guarded.contains(&entry.id) {
                doomed.insert(entry.id.clone());
            }
        }
    }
    if retention.max_total_mb > 0 {
        let budget = retention.max_total_mb.saturating_mul(BYTES_PER_MB);
        // Live sessions cannot be removed but their bytes are still on the
        // disk, so they count against the budget.
        let live_bytes: u64 = state
            .live
            .values()
            .map(|session| session.counters.compressed_bytes.load(Ordering::Relaxed))
            .sum();
        let mut total: u64 = live_bytes
            + state
                .entries
                .iter()
                .filter(|entry| !doomed.contains(&entry.id))
                .map(|entry| entry.compressed_bytes)
                .sum::<u64>();
        if total > budget {
            // A guarded session is not a candidate, and its bytes stay in the
            // total: pretending they are about to go would free nothing.
            let mut candidates: Vec<&LogSessionSummary> = state
                .entries
                .iter()
                .filter(|entry| !doomed.contains(&entry.id) && !guarded.contains(&entry.id))
                .collect();
            candidates.sort_by_key(|entry| entry.started_at_ms);
            for entry in candidates {
                if total <= budget {
                    break;
                }
                total = total.saturating_sub(entry.compressed_bytes);
                doomed.insert(entry.id.clone());
            }
        }
    }
    doomed.into_iter().collect()
}

/// Drops the doomed sessions from the in-memory index and hands their files,
/// with a snapshot taken after the drop, to the caller's disk phase.
fn remove_doomed(state: &mut IndexState, doomed: Vec<String>) -> IndexWrite {
    if doomed.is_empty() {
        return IndexWrite::Nothing;
    }
    let removed: HashSet<&String> = doomed.iter().collect();
    state.entries.retain(|entry| !removed.contains(&entry.id));
    let entries = state.entries.clone();
    IndexWrite::Rewrite { doomed, entries }
}

/// Rewrites `index.jsonl` through a temp file so a crash mid-rewrite cannot
/// leave a half-written index behind.
fn write_index(index_path: &Path, entries: &[LogSessionSummary]) {
    let mut content = String::new();
    for entry in entries {
        match serde_json::to_string(entry) {
            Ok(line) => {
                content.push_str(&line);
                content.push('\n');
            }
            Err(error) => diagnostics::record(
                DiagnosticEntry::error("storage", "Could not serialize an engine-log record")
                    .with_detail(error.to_string()),
            ),
        }
    }
    let temporary = index_path.with_extension("jsonl.tmp");
    let written = File::create(&temporary).and_then(|mut file| {
        file.write_all(content.as_bytes())?;
        file.sync_all()
    });
    if let Err(error) = written.and_then(|()| fs::rename(&temporary, index_path)) {
        diagnostics::record(
            DiagnosticEntry::error("storage", "Could not rewrite the engine-log index")
                .with_detail(error.to_string()),
        );
    }
}

/// Whether a session file has been written to recently enough that a process
/// other than this one is probably still recording into it.
fn recently_modified(path: &Path) -> bool {
    let Ok(modified) = fs::metadata(path).and_then(|metadata| metadata.modified()) else {
        return false;
    };
    is_recent(modified)
}

fn is_recent(modified: SystemTime) -> bool {
    match SystemTime::now().duration_since(modified) {
        Ok(age) => (age.as_millis() as u64) < LIVE_FILE_WINDOW_MS,
        // A timestamp in the future is a clock correction, not an old file.
        Err(_) => true,
    }
}

impl LogWriter {
    pub fn sent(&self, text: &str) {
        self.push(SENT, text);
    }

    pub fn received(&self, text: &str) {
        self.push(RECEIVED, text);
    }

    pub fn stderr(&self, text: &str) {
        self.push(STDERR, text);
    }

    pub fn note(&self, text: &str) {
        self.push(NOTE, text);
    }

    /// Fills in metadata that only becomes known once the game stream opens.
    /// Fields left `None` keep their current value; safe to call repeatedly.
    pub fn describe(&self, patch: SessionDescription) {
        {
            let mut summary = lock(&self.shared.summary);
            if let Some(opponent) = &patch.opponent {
                summary.opponent = Some(opponent.clone());
            }
            if let Some(color) = &patch.color {
                summary.color = Some(color.clone());
            }
            if let Some(clock) = &patch.clock {
                summary.clock = Some(clock.clone());
            }
        }
        // The file must stay self-contained: whoever receives the .gz should not
        // need our index to know who was playing. Lichess repeats `gameFull` on
        // every stream reconnect though, so only a value that actually moved
        // earns a note — repeating them all would duplicate the header fields
        // the reader folds these into and inflate the session's line count.
        let fields = {
            let mut described = lock(&self.shared.described);
            [
                (
                    KEY_OPPONENT,
                    changed(&mut described.opponent, patch.opponent),
                ),
                (
                    KEY_OPPONENT_RATING,
                    changed(&mut described.opponent_rating, patch.opponent_rating)
                        .map(|rating| rating.to_string()),
                ),
                (KEY_COLOR, changed(&mut described.color, patch.color)),
                (KEY_CLOCK, changed(&mut described.clock, patch.clock)),
                (
                    KEY_INITIAL_FEN,
                    changed(&mut described.initial_fen, patch.initial_fen),
                ),
            ]
        };
        for (key, value) in fields {
            if let Some(value) = value {
                self.note(&format!("meta {key}: {}", single_line(&value)));
            }
        }
    }

    /// Flushes, closes the gzip stream, appends the index record, prunes and
    /// returns the final summary. A second call is a no-op returning `None`,
    /// and every other method goes quiet once this has run.
    pub async fn finish(&self, status: &str, result: Option<&str>) -> Option<LogSessionSummary> {
        if self.shared.finished.swap(true, Ordering::SeqCst) {
            return None;
        }
        let (reply, response) = oneshot::channel();
        self.shared.counters.queued.fetch_add(1, Ordering::Relaxed);
        let sent = self
            .shared
            .sender
            .send(LogEvent::Finish {
                at_ms: self.elapsed_ms(),
                status: status.to_string(),
                result: result.map(str::to_string),
                reply,
            })
            .is_ok();
        if !sent {
            self.shared.counters.queued.fetch_sub(1, Ordering::Relaxed);
            return None;
        }
        response.await.ok().flatten()
    }

    fn elapsed_ms(&self) -> u64 {
        // Monotonic: a wall-clock correction mid-game must not rewind the log.
        self.shared.started.elapsed().as_millis() as u64
    }

    fn push(&self, direction: char, text: &str) {
        if self.shared.finished.load(Ordering::Relaxed) {
            return;
        }
        if self.shared.counters.queued.load(Ordering::Relaxed) >= MAX_QUEUED_EVENTS {
            // Better to lose lines than to let a wedged disk grow the queue
            // without bound in the middle of a rated game.
            self.shared.counters.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        self.shared.counters.queued.fetch_add(1, Ordering::Relaxed);
        let event = LogEvent::Line {
            at_ms: self.elapsed_ms(),
            direction,
            text: text.to_string(),
        };
        if self.shared.sender.send(event).is_err() {
            self.shared.counters.queued.fetch_sub(1, Ordering::Relaxed);
            self.shared.counters.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

struct WriterContext {
    store: Arc<StoreInner>,
    path: PathBuf,
    started: Instant,
    summary: Arc<Mutex<LogSessionSummary>>,
    counters: Arc<LiveCounters>,
}

struct SessionWriter {
    encoder: GzEncoder<File>,
    context: WriterContext,
    lines: u64,
    searches: u64,
    raw_bytes: u64,
    /// Set once a write fails: we keep draining so `finish` still gets an
    /// answer, but stop touching the broken file.
    broken: bool,
}

impl SessionWriter {
    fn write_event(&mut self, at_ms: u64, direction: char, text: &str) {
        if self.broken {
            self.context
                .counters
                .dropped
                .fetch_add(1, Ordering::Relaxed);
            return;
        }
        let rendered = format!("{at_ms}\t{direction}\t{}\n", single_line(text));
        if let Err(error) = self.encoder.write_all(rendered.as_bytes()) {
            diagnostics::record(
                DiagnosticEntry::error("storage", "The engine log could not be written")
                    .with_detail(error.to_string()),
            );
            self.broken = true;
            self.context
                .counters
                .dropped
                .fetch_add(1, Ordering::Relaxed);
            return;
        }
        self.raw_bytes += rendered.len() as u64;
        self.lines += 1;
        if direction == NOTE {
            if text.starts_with("search ") {
                self.searches += 1;
            }
            // One flush per completed move: a crash costs at most the move in
            // progress, and the file stays readable while the game runs.
            if text.starts_with("bestmove ") || text.starts_with("session-end ") {
                let _ = self.encoder.flush();
                self.context
                    .counters
                    .compressed_bytes
                    .store(file_len(self.encoder.get_ref()), Ordering::Relaxed);
            }
        }
        let counters = &self.context.counters;
        counters.lines.store(self.lines, Ordering::Relaxed);
        counters.searches.store(self.searches, Ordering::Relaxed);
        counters.raw_bytes.store(self.raw_bytes, Ordering::Relaxed);
    }

    fn elapsed_ms(&self) -> u64 {
        self.context.started.elapsed().as_millis() as u64
    }

    fn finalize(
        mut self,
        at_ms: u64,
        status: &str,
        result: Option<&str>,
    ) -> Option<LogSessionSummary> {
        let note = match result {
            Some(result) => format!("session-end status={status} result={result}"),
            None => format!("session-end status={status}"),
        };
        self.write_event(at_ms, NOTE, &note);
        let compressed_bytes = match self.encoder.finish() {
            Ok(file) => {
                let _ = file.sync_all();
                file_len(&file)
            }
            Err(error) => {
                diagnostics::record(
                    DiagnosticEntry::error("storage", "Could not close the engine log")
                        .with_detail(error.to_string()),
                );
                fs::metadata(&self.context.path)
                    .map(|metadata| metadata.len())
                    .unwrap_or(0)
            }
        };
        let summary = {
            let mut summary = lock(&self.context.summary);
            summary.live = false;
            summary.finished_at_ms = Some(now_ms());
            summary.status = Some(status.to_string());
            summary.result = result.map(str::to_string);
            summary.line_count = self.lines;
            summary.search_count = clamp_u32(self.searches);
            summary.raw_bytes = self.raw_bytes;
            summary.compressed_bytes = compressed_bytes;
            summary.dropped_lines = self.context.counters.dropped.load(Ordering::Relaxed);
            summary.clone()
        };
        self.context.store.complete_session(&summary);
        Some(summary)
    }
}

fn run_writer(
    encoder: GzEncoder<File>,
    receiver: Receiver<LogEvent>,
    context: WriterContext,
    raw_bytes: u64,
) {
    let mut writer = SessionWriter {
        encoder,
        context,
        lines: 0,
        searches: 0,
        raw_bytes,
        broken: false,
    };
    loop {
        let Ok(event) = receiver.recv() else {
            // Every handle went away without a finish: the game task died or the
            // app is closing. Record the session anyway, or it would stay live
            // forever and never become prunable.
            let at_ms = writer.elapsed_ms();
            writer.finalize(at_ms, "interrupted", None);
            return;
        };
        writer
            .context
            .counters
            .queued
            .fetch_sub(1, Ordering::Relaxed);
        match event {
            LogEvent::Line {
                at_ms,
                direction,
                text,
            } => writer.write_event(at_ms, direction, &text),
            LogEvent::Finish {
                at_ms,
                status,
                result,
                reply,
            } => {
                let summary = writer.finalize(at_ms, &status, result.as_deref());
                let _ = reply.send(summary);
                return;
            }
        }
    }
}

/// Picks the first free `<base>`, `<base>-2`, `<base>-3`, … and creates the
/// file, so the id is reserved on disk the moment it is chosen.
fn reserve_session_file(
    sessions_dir: &Path,
    base: &str,
    taken: &HashSet<String>,
) -> Option<(String, File)> {
    for attempt in 1..=MAX_ID_ATTEMPTS {
        let id = if attempt == 1 {
            base.to_string()
        } else {
            format!("{base}-{attempt}")
        };
        if taken.contains(&id) {
            continue;
        }
        let path = sessions_dir.join(format!("{id}{SESSION_SUFFIX}"));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Some((id, file)),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => {
                diagnostics::record(
                    DiagnosticEntry::error("storage", "Could not create the engine-log file")
                        .with_detail(error.to_string()),
                );
                return None;
            }
        }
    }
    diagnostics::record(
        DiagnosticEntry::error("storage", "Could not find a free engine-log file name")
            .with_detail(base.to_string()),
    );
    None
}

/// Session ids double as file names, so everything outside the portable set
/// becomes '-'.
fn sanitize_id(raw: &str) -> String {
    raw.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                character
            } else {
                '-'
            }
        })
        .collect()
}

fn valid_session_id(value: &str) -> bool {
    !value.is_empty()
        && !matches!(value, "." | "..")
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        })
}

fn header_block(session_id: &str, started_at_ms: u64, meta: &SessionMeta) -> String {
    let mut fields: Vec<(&str, String)> = vec![
        (KEY_FORMAT, "1".to_string()),
        (KEY_SESSION, session_id.to_string()),
        (KEY_STARTED, iso_timestamp(started_at_ms)),
        (KEY_STARTED_MS, started_at_ms.to_string()),
        (KEY_KIND, meta.kind.clone()),
    ];
    let optional: [(&str, Option<String>); 11] = [
        (KEY_GAME, meta.game_id.clone()),
        (KEY_ACCOUNT, Some(meta.account_id.clone())),
        (KEY_BOT, Some(meta.bot_username.clone())),
        (KEY_OPPONENT, meta.opponent.clone()),
        (
            KEY_OPPONENT_RATING,
            meta.opponent_rating.map(|rating| rating.to_string()),
        ),
        (KEY_ENGINE_ID, Some(meta.engine_id.clone())),
        (KEY_ENGINE, Some(meta.engine_name.clone())),
        (KEY_ENGINE_PATH, Some(meta.engine_path.clone())),
        (KEY_COLOR, meta.color.clone()),
        (KEY_CLOCK, meta.clock.clone()),
        (KEY_INITIAL_FEN, meta.initial_fen.clone()),
    ];
    for (key, value) in optional {
        if let Some(value) = value {
            fields.push((key, value));
        }
    }
    if let Some(book) = &meta.book {
        fields.push((KEY_BOOK, book.clone()));
    }
    fields.push((KEY_APP_VERSION, meta.app_version.clone()));
    for (name, value) in &meta.options {
        fields.push((KEY_OPTION, format!("{name} = {value}")));
    }

    let mut block = String::new();
    for (key, value) in fields {
        block.push_str(&format!("# {key}: {}\n", single_line(&value)));
    }
    block
}

/// Records `incoming` and hands it back when it says something `current` does
/// not already say. `None` means there is nothing new to write down.
fn changed<T: Clone + PartialEq>(current: &mut Option<T>, incoming: Option<T>) -> Option<T> {
    let value = incoming?;
    if current.as_ref() == Some(&value) {
        return None;
    }
    *current = Some(value.clone());
    Some(value)
}

/// One event is one line, so a stray newline would break the file's shape;
/// engines do emit them on stderr.
fn single_line(text: &str) -> String {
    let trimmed = text.trim_end_matches(['\n', '\r']);
    if trimmed.contains(['\n', '\r']) {
        trimmed.replace(['\n', '\r'], " ")
    } else {
        trimmed.to_string()
    }
}

/// Decompresses as far as the stream allows. A session killed mid-write (or one
/// still recording) ends in a partial deflate block, and the lines before it are
/// exactly what the operator wants to see.
fn decode_lossy(path: &Path) -> Result<Vec<u8>, String> {
    decode_lossy_bounded(path, usize::MAX)
}

fn decode_lossy_bounded(path: &Path, max_bytes: usize) -> Result<Vec<u8>, String> {
    let file =
        File::open(path).map_err(|error| format!("Could not open that log session: {error}"))?;
    let mut decoder = GzDecoder::new(file);
    let mut decoded = Vec::new();
    let mut buffer = vec![0u8; DECODE_CHUNK];
    loop {
        match decoder.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                if decoded.len().saturating_add(read) > max_bytes {
                    return Err("The log export exceeds the server byte cap".into());
                }
                decoded.extend_from_slice(&buffer[..read]);
            }
            Err(_) => break,
        }
    }
    Ok(decoded)
}

fn parse_session(bytes: &[u8], file_len: u64) -> SessionData {
    let content = String::from_utf8_lossy(bytes);
    let mut header = Vec::new();
    let mut lines: Vec<LogLine> = Vec::new();
    for raw in content.lines() {
        if let Some(line) = parse_event(raw, lines.len() as u64) {
            // `meta` notes are late header fields; they belong in both places.
            if direction_is(&line, NOTE) {
                if let Some(field) = meta_field(&line.text) {
                    header.push(field);
                }
            }
            lines.push(line);
            continue;
        }
        // The header block always precedes the first event line.
        if lines.is_empty() {
            if let Some((key, value)) = raw.strip_prefix("# ").and_then(|f| f.split_once(": ")) {
                header.push(LogHeaderField {
                    key: key.to_string(),
                    value: value.to_string(),
                });
            }
        }
    }
    SessionData {
        header,
        lines,
        file_len,
        decoded_bytes: bytes.len() as u64,
    }
}

fn parse_event(raw: &str, index: u64) -> Option<LogLine> {
    let (stamp, rest) = raw.split_once('\t')?;
    let at_ms = stamp.parse::<u64>().ok()?;
    let (direction, text) = rest.split_once('\t')?;
    let mut characters = direction.chars();
    let direction = characters.next()?;
    if characters.next().is_some() || !matches!(direction, SENT | RECEIVED | STDERR | NOTE) {
        return None;
    }
    Some(LogLine {
        index,
        at_ms,
        direction: direction.to_string(),
        text: text.to_string(),
    })
}

fn direction_is(line: &LogLine, direction: char) -> bool {
    line.direction.starts_with(direction)
}

fn meta_field(text: &str) -> Option<LogHeaderField> {
    let (key, value) = text.strip_prefix("meta ")?.split_once(": ")?;
    Some(LogHeaderField {
        key: key.to_string(),
        value: value.to_string(),
    })
}

/// `key=value` tokens out of a note's payload.
fn note_fields(text: &str) -> HashMap<&str, &str> {
    text.split_whitespace()
        .filter_map(|token| token.split_once('='))
        .collect()
}

fn build_outline(lines: &[LogLine]) -> Vec<LogSearchBlock> {
    let mut blocks: Vec<LogSearchBlock> = Vec::new();
    let mut open: Option<usize> = None;
    for line in lines {
        if direction_is(line, NOTE) {
            if let Some(payload) = line.text.strip_prefix("search ") {
                let fields = note_fields(payload);
                blocks.push(LogSearchBlock {
                    move_number: fields
                        .get("move")
                        .and_then(|value| value.parse().ok())
                        .unwrap_or(0),
                    ply: fields
                        .get("ply")
                        .and_then(|value| value.parse().ok())
                        .unwrap_or(0),
                    color: fields
                        .get("color")
                        .map_or_else(String::new, |value| (*value).to_string()),
                    start_line: line.index,
                    end_line: line.index,
                    best_move: None,
                    elapsed_ms: None,
                    depth: None,
                    score_cp: None,
                    mate_in: None,
                });
                open = Some(blocks.len() - 1);
                continue;
            }
            if let Some(payload) = line.text.strip_prefix("bestmove ") {
                if let Some(index) = open.take() {
                    let fields = note_fields(payload);
                    let block = &mut blocks[index];
                    block.best_move = fields.get("uci").map(|value| (*value).to_string());
                    block.elapsed_ms = fields.get("elapsed").and_then(|value| value.parse().ok());
                    block.end_line = line.index;
                }
                continue;
            }
        }
        let Some(index) = open else {
            continue;
        };
        blocks[index].end_line = line.index;
        if direction_is(line, RECEIVED) && line.text.starts_with("info ") {
            apply_info(&mut blocks[index], &line.text);
        }
    }
    blocks
}

/// Folds one `info` line into its block. Depth and score are taken from the
/// last line that carries each, and a score always replaces the other kind so
/// a block never claims both a centipawn and a mate evaluation.
fn apply_info(block: &mut LogSearchBlock, text: &str) {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "depth" => {
                if let Some(depth) = tokens.get(index + 1).and_then(|v| v.parse().ok()) {
                    block.depth = Some(depth);
                }
            }
            "score" if index + 2 < tokens.len() => {
                let value = tokens[index + 2].parse::<i32>().ok();
                match tokens[index + 1] {
                    "cp" if value.is_some() => {
                        block.score_cp = value;
                        block.mate_in = None;
                    }
                    "mate" if value.is_some() => {
                        block.mate_in = value;
                        block.score_cp = None;
                    }
                    _ => {}
                }
            }
            // These four all run to the end of the line and can contain
            // anything, so stop looking for keywords here: `info string depth
            // 99 …` is an engine talking, not a search that reached depth 99.
            "pv" | "string" | "currline" | "refutation" => break,
            _ => {}
        }
        index += 1;
    }
}

enum Matcher {
    Regex(Regex),
    Substring { needle: String, fold_case: bool },
}

impl Matcher {
    fn build(query: &LogQuery) -> Result<Self, String> {
        if query.regex {
            RegexBuilder::new(&query.text)
                .case_insensitive(!query.case_sensitive)
                .build()
                .map(Matcher::Regex)
                .map_err(|error| format!("That search pattern is not valid: {error}"))
        } else if query.case_sensitive {
            Ok(Matcher::Substring {
                needle: query.text.clone(),
                fold_case: false,
            })
        } else {
            Ok(Matcher::Substring {
                needle: query.text.to_lowercase(),
                fold_case: true,
            })
        }
    }

    fn is_match(&self, text: &str) -> bool {
        match self {
            Matcher::Regex(regex) => regex.is_match(text),
            Matcher::Substring { needle, fold_case } => {
                if *fold_case {
                    text.to_lowercase().contains(needle)
                } else {
                    text.contains(needle)
                }
            }
        }
    }
}

fn matches_filter(summary: &LogSessionSummary, filter: &LogFilter) -> bool {
    if filter
        .account_id
        .as_ref()
        .is_some_and(|id| &summary.account_id != id)
    {
        return false;
    }
    if filter
        .engine_id
        .as_ref()
        .is_some_and(|id| &summary.engine_id != id)
    {
        return false;
    }
    if filter
        .from_ms
        .is_some_and(|from| summary.started_at_ms < from)
    {
        return false;
    }
    if filter.to_ms.is_some_and(|to| summary.started_at_ms > to) {
        return false;
    }
    match filter.query.as_deref() {
        Some(query) if !query.trim().is_empty() => {
            let needle = query.to_lowercase();
            [
                summary.opponent.as_deref(),
                summary.game_id.as_deref(),
                Some(summary.engine_name.as_str()),
                Some(summary.bot_username.as_str()),
            ]
            .into_iter()
            .flatten()
            .any(|field| field.to_lowercase().contains(&needle))
        }
        _ => true,
    }
}

/// Rebuilds index records for session files the index does not know about. A
/// crash leaves the record unwritten, and an untracked file would otherwise be
/// invisible to the UI and, worse, immune to pruning.
fn adopt_orphans(sessions_dir: &Path, entries: &[LogSessionSummary]) -> Vec<LogSessionSummary> {
    let mut recovered = Vec::new();
    let Ok(directory) = fs::read_dir(sessions_dir) else {
        return recovered;
    };
    let known: HashSet<&str> = entries.iter().map(|entry| entry.id.as_str()).collect();
    for file in directory.flatten() {
        let path = file.path();
        let Some(id) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix(SESSION_SUFFIX))
        else {
            continue;
        };
        if known.contains(id) {
            continue;
        }
        let metadata = fs::metadata(&path).ok();
        let file_len = metadata.as_ref().map_or(0, fs::Metadata::len);
        let modified = metadata
            .as_ref()
            .and_then(|metadata| metadata.modified().ok());
        // A session only lacks its index record while it is still recording, so
        // a file that was written moments ago belongs to a QueenUI window that
        // is playing a game right now. Adopting it would hand its file to our
        // pruner and delete a live rated game's recording out from under it.
        if modified.is_some_and(is_recent) {
            continue;
        }
        let modified_ms = modified
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |since| since.as_millis() as u64);
        let Ok(bytes) = decode_lossy(&path) else {
            continue;
        };
        let data = parse_session(&bytes, file_len);
        recovered.push(summary_from_session(id, &data, modified_ms));
    }
    if !recovered.is_empty() {
        crate::diagnostics::record(crate::diagnostics::DiagnosticEntry::warn(
            "app",
            format!(
                "Recovered {} engine-log session(s) that were interrupted",
                recovered.len()
            ),
        ));
    }
    recovered
}

/// Rebuilds an index record from the file itself. `modified_ms` is the file's
/// mtime, the only start time available when the header did not survive.
fn summary_from_session(id: &str, data: &SessionData, modified_ms: u64) -> LogSessionSummary {
    let field = |key: &str| {
        // Late `meta` notes override the opening header block.
        data.header
            .iter()
            .rev()
            .find(|field| field.key == key)
            .map(|field| field.value.clone())
    };
    let started_at_ms: u64 = field(KEY_STARTED_MS)
        .and_then(|value| value.parse().ok())
        .unwrap_or(modified_ms);
    // The last event says when the recording actually stopped. A session-end
    // note on top means only the index write was lost, so the rebuilt record
    // can keep the real outcome.
    let finished_at_ms = data
        .lines
        .last()
        .map(|line| started_at_ms.saturating_add(line.at_ms));
    let ending = data.lines.iter().rev().find_map(|line| {
        line.text
            .strip_prefix("session-end ")
            .map(|payload| note_fields(payload))
    });
    let (status, result) = match ending {
        Some(fields) => (
            fields.get("status").map(|value| (*value).to_string()),
            fields.get("result").map(|value| (*value).to_string()),
        ),
        None => (Some("interrupted".to_string()), None),
    };
    LogSessionSummary {
        id: id.to_string(),
        kind: field(KEY_KIND).unwrap_or_else(|| "game".to_string()),
        game_id: field(KEY_GAME),
        account_id: field(KEY_ACCOUNT).unwrap_or_default(),
        bot_username: field(KEY_BOT).unwrap_or_default(),
        opponent: field(KEY_OPPONENT),
        engine_id: field(KEY_ENGINE_ID).unwrap_or_default(),
        engine_name: field(KEY_ENGINE).unwrap_or_default(),
        color: field(KEY_COLOR),
        clock: field(KEY_CLOCK),
        started_at_ms,
        finished_at_ms,
        status,
        result,
        line_count: data.lines.len() as u64,
        search_count: clamp_u32(
            data.lines
                .iter()
                .filter(|line| direction_is(line, NOTE) && line.text.starts_with("search "))
                .count() as u64,
        ),
        compressed_bytes: data.file_len,
        // What the writer counts is every byte it handed the encoder — the
        // header block plus each rendered line, timestamp and tabs included —
        // which is exactly the decompressed length. Counting the payload text
        // alone would understate it and skew the compression ratio the UI
        // derives from these two numbers.
        raw_bytes: data.decoded_bytes,
        dropped_lines: 0,
        live: false,
    }
}

fn append_line(path: &Path, line: &str) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let appended = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| writeln!(file, "{line}"));
    if let Err(error) = appended {
        diagnostics::record(
            DiagnosticEntry::error("storage", "Could not append to the engine-log index")
                .with_detail(error.to_string()),
        );
    }
}

/// A poisoned lock must never take logging down with it: everything behind
/// these locks is plain data that stays consistent through a panic.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn file_len(file: &File) -> u64 {
    file.metadata().map(|metadata| metadata.len()).unwrap_or(0)
}

fn clamp_u32(value: u64) -> u32 {
    value.min(u64::from(u32::MAX)) as u32
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or(0)
}

/// Civil date from days since the Unix epoch (Howard Hinnant's algorithm).
/// Deliberately duplicated rather than shared with the Scorebook: writing a
/// header line must not depend on the statistics module.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month as u32, day as u32)
}

/// (year, month, day, hour, minute, second) in UTC.
fn utc_parts(ms: u64) -> (i64, u32, u32, u32, u32, u32) {
    let seconds = (ms / 1_000) as i64;
    let (year, month, day) = civil_from_days(seconds.div_euclid(86_400));
    let time = seconds.rem_euclid(86_400);
    (
        year,
        month,
        day,
        (time / 3_600) as u32,
        ((time / 60) % 60) as u32,
        (time % 60) as u32,
    )
}

fn compact_timestamp(ms: u64) -> String {
    let (year, month, day, hour, minute, second) = utc_parts(ms);
    format!("{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}")
}

fn iso_timestamp(ms: u64) -> String {
    let (year, month, day, hour, minute, second) = utc_parts(ms);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

#[cfg(test)]
mod tests {
    use super::{
        append_line, compact_timestamp, iso_timestamp, lock, now_ms, reserve_session_file,
        sanitize_id, EngineLogStore, ExportMode, LogFilter, LogHeaderField, LogQuery, LogRetention,
        LogSessionSummary, SessionDescription, SessionMeta, DAY_MS, INDEX_FILE,
        LIVE_FILE_WINDOW_MS, LOGS_DIR, SESSIONS_DIR, SESSION_SUFFIX,
    };
    use std::{
        collections::HashSet,
        fs,
        path::{Path, PathBuf},
        time::{Duration, SystemTime},
    };

    fn temp_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "queenui-enginelog-{label}-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).expect("create temp dir");
        root
    }

    fn sessions_dir(root: &Path) -> PathBuf {
        root.join(LOGS_DIR).join(SESSIONS_DIR)
    }

    fn index_path(root: &Path) -> PathBuf {
        root.join(LOGS_DIR).join(INDEX_FILE)
    }

    fn session_file(root: &Path, id: &str) -> PathBuf {
        sessions_dir(root).join(format!("{id}{SESSION_SUFFIX}"))
    }

    fn set_age(path: &Path, ago: Duration) {
        let file = fs::OpenOptions::new()
            .write(true)
            .open(path)
            .expect("open session file");
        file.set_times(fs::FileTimes::new().set_modified(SystemTime::now() - ago))
            .expect("set the modification time");
    }

    /// Backdates a session file past the freshness window, so it looks like
    /// what a crash left behind rather than what a running QueenUI is writing.
    fn abandon(path: &Path) {
        set_age(path, Duration::from_millis(LIVE_FILE_WINDOW_MS * 2));
    }

    /// The opposite: another window just flushed a move into this file.
    fn keep_alive(path: &Path) {
        set_age(path, Duration::ZERO);
    }

    fn cached_session(store: &EngineLogStore) -> Option<String> {
        lock(&store.inner.cache).as_ref().map(|(id, _)| id.clone())
    }

    fn index_lines(root: &Path) -> Vec<String> {
        fs::read_to_string(index_path(root))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn meta() -> SessionMeta {
        SessionMeta {
            kind: "game".into(),
            game_id: Some("abc123".into()),
            account_id: "queenbot".into(),
            bot_username: "QueenBot".into(),
            opponent: None,
            opponent_rating: None,
            engine_id: "engine-1".into(),
            engine_name: "Stockfish 17".into(),
            engine_path: "C:/engines/stockfish.exe".into(),
            color: None,
            clock: None,
            initial_fen: None,
            options: vec![
                ("Hash".into(), "256".into()),
                ("Threads".into(), "4".into()),
            ],
            book: Some("Perfect2023 (polyglot, max 12 plies)".into()),
            app_version: "0.1.0".into(),
        }
    }

    fn header_value<'a>(header: &'a [LogHeaderField], key: &str) -> Option<&'a str> {
        header
            .iter()
            .rev()
            .find(|field| field.key == key)
            .map(|field| field.value.as_str())
    }

    /// Writes an index record plus a placeholder session file, standing in for
    /// history this process did not record itself.
    fn fabricate(root: &Path, id: &str, started_at_ms: u64, compressed_bytes: u64) {
        fs::create_dir_all(sessions_dir(root)).expect("create sessions dir");
        fs::write(session_file(root, id), b"placeholder").expect("write session file");
        let summary = LogSessionSummary {
            id: id.into(),
            kind: "game".into(),
            account_id: "queenbot".into(),
            bot_username: "QueenBot".into(),
            engine_id: "engine-1".into(),
            engine_name: "Stockfish 17".into(),
            started_at_ms,
            compressed_bytes,
            ..LogSessionSummary::default()
        };
        append_line(
            &index_path(root),
            &serde_json::to_string(&summary).expect("serialize summary"),
        );
    }

    /// Two searches with enough info lines to exercise the outline's scoring.
    fn play_two_searches(writer: &super::LogWriter) {
        writer.note("search ply=0 move=1 color=w wtime=180000 btime=180000 winc=2000 binc=2000");
        writer.sent("position startpos");
        writer.sent("go wtime 180000 btime 180000 winc 2000 binc 2000");
        writer.received("info depth 5 score cp 10 pv e2e4");
        writer.received("info depth 12 score cp 34 nodes 120000 pv e2e4 e7e5");
        writer.received("info depth 13 currmove e2e4 currmovenumber 1");
        writer.received("bestmove e2e4");
        writer.note("bestmove uci=e2e4 elapsed=1200");
        writer.note("search ply=2 move=2 color=w wtime=178000 btime=179000 winc=2000 binc=2000");
        writer.received("info depth 20 score mate 3 pv h5f7");
        writer.received("bestmove h5f7");
        writer.note("bestmove uci=h5f7 elapsed=800");
    }

    #[tokio::test]
    async fn records_and_pages_a_session_round_trip() {
        let root = temp_root("roundtrip");
        let store = EngineLogStore::load(&root, LogRetention::default());
        let writer = store.open_session(meta()).expect("open session");
        writer.sent("uci");
        writer.received("id name Stockfish 17");
        writer.stderr("warning: NNUE file missing\n");
        writer.note("book move=e2e4");
        let summary = writer
            .finish("mate", Some("win"))
            .await
            .expect("final summary");

        assert!(!summary.live);
        assert_eq!(summary.status.as_deref(), Some("mate"));
        assert_eq!(summary.result.as_deref(), Some("win"));
        assert_eq!(summary.game_id.as_deref(), Some("abc123"));
        // Four events plus the session-end note.
        assert_eq!(summary.line_count, 5);
        assert_eq!(summary.dropped_lines, 0);
        assert!(summary.compressed_bytes > 0);
        assert!(summary.finished_at_ms.is_some());

        let page = store.page(&summary.id, 0, 100).expect("page");
        assert_eq!(page.session_id, summary.id);
        assert_eq!(page.total_lines, 5);
        assert!(!page.live);
        let shape: Vec<(u64, &str, &str)> = page
            .lines
            .iter()
            .map(|line| (line.index, line.direction.as_str(), line.text.as_str()))
            .collect();
        assert_eq!(
            shape,
            [
                (0, ">", "uci"),
                (1, "<", "id name Stockfish 17"),
                // The trailing newline is stripped, not stored.
                (2, "!", "warning: NNUE file missing"),
                (3, "#", "book move=e2e4"),
                (4, "#", "session-end status=mate result=win"),
            ]
        );

        assert_eq!(header_value(&page.header, "queenui-log"), Some("1"));
        assert_eq!(
            header_value(&page.header, "session"),
            Some(summary.id.as_str())
        );
        assert_eq!(header_value(&page.header, "bot"), Some("QueenBot"));
        assert_eq!(header_value(&page.header, "game"), Some("abc123"));
        assert_eq!(header_value(&page.header, "engine"), Some("Stockfish 17"));
        assert_eq!(header_value(&page.header, "option"), Some("Threads = 4"));
        assert_eq!(
            header_value(&page.header, "started-ms"),
            Some(summary.started_at_ms.to_string().as_str())
        );
        // Fields nobody supplied are simply absent.
        assert_eq!(header_value(&page.header, "opponent"), None);

        let window = store.page(&summary.id, 2, 2).expect("window");
        assert_eq!(window.offset, 2);
        assert_eq!(window.total_lines, 5);
        assert_eq!(window.lines.len(), 2);
        assert_eq!(window.lines[0].index, 2);
        // Past the end is empty rather than an error.
        assert!(store
            .page(&summary.id, 99, 10)
            .expect("tail")
            .lines
            .is_empty());

        // The record survives a reload, and filters find it.
        let reloaded = EngineLogStore::load(&root, LogRetention::default());
        assert_eq!(reloaded.list(&LogFilter::default()).len(), 1);
        assert_eq!(
            reloaded
                .list(&LogFilter {
                    query: Some("stockFISH".into()),
                    ..LogFilter::default()
                })
                .len(),
            1
        );
        assert!(reloaded
            .list(&LogFilter {
                account_id: Some("someone-else".into()),
                ..LogFilter::default()
            })
            .is_empty());
        let overview = reloaded.overview();
        assert_eq!(overview.session_count, 1);
        assert_eq!(overview.live_count, 0);
        assert_eq!(overview.oldest_started_at_ms, Some(summary.started_at_ms));

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn builds_an_outline_from_search_notes() {
        let root = temp_root("outline");
        let store = EngineLogStore::load(&root, LogRetention::default());
        let writer = store.open_session(meta()).expect("open session");
        play_two_searches(&writer);
        let summary = writer.finish("mate", Some("win")).await.expect("summary");
        assert_eq!(summary.search_count, 2);

        let blocks = store.outline(&summary.id).expect("outline");
        assert_eq!(blocks.len(), 2);

        assert_eq!(blocks[0].move_number, 1);
        assert_eq!(blocks[0].ply, 0);
        assert_eq!(blocks[0].color, "w");
        assert_eq!(blocks[0].start_line, 0);
        // The block ends on its bestmove note, not on the next search.
        assert_eq!(blocks[0].end_line, 7);
        assert_eq!(blocks[0].best_move.as_deref(), Some("e2e4"));
        assert_eq!(blocks[0].elapsed_ms, Some(1200));
        // Depth from the last info line carrying one, score from the last that
        // carried a score.
        assert_eq!(blocks[0].depth, Some(13));
        assert_eq!(blocks[0].score_cp, Some(34));
        assert_eq!(blocks[0].mate_in, None);

        assert_eq!(blocks[1].move_number, 2);
        assert_eq!(blocks[1].ply, 2);
        assert_eq!(blocks[1].start_line, 8);
        assert_eq!(blocks[1].end_line, 11);
        assert_eq!(blocks[1].best_move.as_deref(), Some("h5f7"));
        assert_eq!(blocks[1].depth, Some(20));
        // A mate score replaces the centipawn score rather than joining it.
        assert_eq!(blocks[1].mate_in, Some(3));
        assert_eq!(blocks[1].score_cp, None);

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn searches_by_substring_case_and_regex() {
        let root = temp_root("search");
        let store = EngineLogStore::load(&root, LogRetention::default());
        let writer = store.open_session(meta()).expect("open session");
        play_two_searches(&writer);
        let summary = writer.finish("mate", Some("win")).await.expect("summary");

        let query = |text: &str, regex: bool, case_sensitive: bool, limit: u64| LogQuery {
            text: text.into(),
            regex,
            case_sensitive,
            limit,
        };

        let matches = store
            .search(&summary.id, &query("info ", false, false, 0))
            .expect("substring search");
        assert_eq!(matches.len(), 4);
        assert_eq!(matches[0].line_index, 3);
        assert_eq!(matches[0].direction, "<");

        // Case folding is the default; asking for exact case honours it.
        assert_eq!(
            store
                .search(&summary.id, &query("INFO ", false, false, 0))
                .expect("folded search")
                .len(),
            4
        );
        assert!(store
            .search(&summary.id, &query("INFO ", false, true, 0))
            .expect("cased search")
            .is_empty());

        let regex_matches = store
            .search(&summary.id, &query(r"depth (1[0-9])\b", true, false, 0))
            .expect("regex search");
        assert_eq!(regex_matches.len(), 2);
        assert!(regex_matches[0].text.contains("depth 12"));

        // The cap applies to lines within a session.
        assert_eq!(
            store
                .search(&summary.id, &query("info ", false, false, 2))
                .expect("capped search")
                .len(),
            2
        );
        // An empty query is never a match-everything.
        assert!(store
            .search(&summary.id, &query("", false, false, 0))
            .expect("empty search")
            .is_empty());
        assert!(store
            .search(&summary.id, &query("(unclosed", true, false, 0))
            .is_err());

        let sweep = store
            .search_all(&LogFilter::default(), &query("bestmove", false, false, 0))
            .expect("sweep");
        assert_eq!(sweep.len(), 1);
        assert_eq!(sweep[0].session.id, summary.id);
        // Two engine bestmove lines and two bestmove notes.
        assert_eq!(sweep[0].match_count, 4);
        assert_eq!(sweep[0].first.as_ref().expect("first match").line_index, 6);
        // The sweep cap counts sessions, not lines.
        assert!(store
            .search_all(
                &LogFilter::default(),
                &query("nothing-here", false, false, 0)
            )
            .expect("empty sweep")
            .is_empty());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn prunes_sessions_older_than_the_age_cap() {
        let root = temp_root("prune-age");
        let now = now_ms();
        fabricate(&root, "ancient", now - 60 * DAY_MS, 1_000);
        fabricate(&root, "recent", now - DAY_MS, 1_000);

        let store = EngineLogStore::load(
            &root,
            LogRetention {
                capture_enabled: true,
                max_total_mb: 0,
                max_age_days: 30,
            },
        );
        assert_eq!(store.prune(), 1);
        let ids: Vec<String> = store
            .list(&LogFilter::default())
            .into_iter()
            .map(|session| session.id)
            .collect();
        assert_eq!(ids, ["recent"]);
        assert!(!session_file(&root, "ancient").exists());
        // The index was rewritten, not just the memory copy.
        let index = fs::read_to_string(index_path(&root)).expect("read index");
        assert_eq!(index.lines().count(), 1);
        assert!(index.contains("\"recent\""));
        // A second pass has nothing left to do.
        assert_eq!(store.prune(), 0);

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn prunes_by_total_size_but_never_a_live_session() {
        let root = temp_root("prune-size");
        let now = now_ms();
        fabricate(&root, "oldest", now - 3 * DAY_MS, 900_000);
        fabricate(&root, "middle", now - 2 * DAY_MS, 900_000);
        fabricate(&root, "newest", now - DAY_MS, 900_000);

        let store = EngineLogStore::load(
            &root,
            LogRetention {
                capture_enabled: true,
                max_total_mb: 1,
                max_age_days: 0,
            },
        );
        let writer = store.open_session(meta()).expect("open session");
        writer.sent("uci");

        // 2.7 MB against a 1 MB budget: the two oldest go, the newest stays.
        assert_eq!(store.prune(), 2);
        let ids: HashSet<String> = store
            .list(&LogFilter::default())
            .into_iter()
            .map(|session| session.id)
            .collect();
        assert!(ids.contains("newest"));
        assert!(!ids.contains("oldest"));
        assert!(!ids.contains("middle"));
        assert_eq!(ids.len(), 2, "the live session survives the sweep");
        assert_eq!(store.overview().live_count, 1);

        let summary = writer.finish("aborted", None).await.expect("summary");
        assert!(session_file(&root, &summary.id).exists());
        assert!(!store
            .list(&LogFilter::default())
            .iter()
            .any(|session| session.live));

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn deletes_and_clears_only_finished_sessions() {
        let root = temp_root("delete");
        let store = EngineLogStore::load(&root, LogRetention::default());
        let finished = {
            let writer = store.open_session(meta()).expect("open session");
            writer.sent("uci");
            writer.finish("mate", Some("win")).await.expect("summary")
        };
        let live = store.open_session(meta()).expect("open live session");
        live.sent("uci");
        let live_id = store
            .list(&LogFilter::default())
            .into_iter()
            .find(|session| session.live)
            .expect("live session")
            .id;

        assert!(store.delete(&live_id).is_err());
        assert!(store.delete("no-such-session").is_err());
        store.delete(&finished.id).expect("delete finished");
        assert!(!session_file(&root, &finished.id).exists());
        assert!(store.page(&finished.id, 0, 10).is_err());

        // Clearing leaves the recording session alone.
        assert_eq!(store.clear().expect("clear"), 0);
        assert!(session_file(&root, &live_id).exists());
        let summary = live.finish("aborted", None).await.expect("summary");
        assert_eq!(store.clear().expect("clear again"), 1);
        assert!(!session_file(&root, &summary.id).exists());
        assert_eq!(
            fs::read_to_string(index_path(&root))
                .expect("read index")
                .trim(),
            ""
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn keeps_readable_index_lines_past_a_corrupt_one() {
        let root = temp_root("index-corrupt");
        let now = now_ms();
        fabricate(&root, "one", now - 2_000, 10);
        append_line(&index_path(&root), "{ this is not json");
        fabricate(&root, "two", now - 1_000, 10);
        append_line(&index_path(&root), "");

        let store = EngineLogStore::load(&root, LogRetention::default());
        let ids: Vec<String> = store
            .list(&LogFilter::default())
            .into_iter()
            .map(|session| session.id)
            .collect();
        // Newest first.
        assert_eq!(ids, ["two", "one"]);

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn decodes_the_prefix_of_a_truncated_session() {
        let root = temp_root("truncated");
        let store = EngineLogStore::load(&root, LogRetention::default());
        let writer = store.open_session(meta()).expect("open session");
        for move_number in 1..=6u32 {
            writer.note(&format!(
                "search ply={} move={move_number} color=w wtime=60000 btime=60000 winc=0 binc=0",
                move_number - 1
            ));
            for depth in 1..=60u32 {
                writer.received(&format!(
                    "info depth {depth} seldepth {} score cp {depth} nodes {} nps 1500000 pv e2e4 e7e5 g1f3 b8c6",
                    depth + 4,
                    u64::from(depth) * 100_000
                ));
            }
            writer.note("bestmove uci=e2e4 elapsed=900");
        }
        let summary = writer.finish("mate", Some("win")).await.expect("summary");

        // Keep only the bytes a power loss would have left behind.
        let bytes = fs::read(session_file(&root, &summary.id)).expect("read session");
        assert!(bytes.len() > 64);
        fs::write(
            session_file(&root, "halfwritten"),
            &bytes[..bytes.len() * 3 / 5],
        )
        .expect("write truncated session");
        abandon(&session_file(&root, "halfwritten"));

        let reloaded = EngineLogStore::load(&root, LogRetention::default());
        let page = reloaded.page("halfwritten", 0, 0).expect("page");
        assert!(
            !page.lines.is_empty(),
            "the flushed prefix must still decode"
        );
        assert!(page.lines.len() < summary.line_count as usize);
        // The header is flushed at open, so it is always there.
        assert_eq!(header_value(&page.header, "bot"), Some("QueenBot"));
        let recovered = reloaded
            .list(&LogFilter::default())
            .into_iter()
            .find(|session| session.id == "halfwritten")
            .expect("recovered session");
        assert_eq!(recovered.status.as_deref(), Some("interrupted"));
        assert_eq!(recovered.line_count, page.lines.len() as u64);

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn reconciles_the_index_with_the_sessions_directory() {
        let root = temp_root("reconcile");
        let store = EngineLogStore::load(&root, LogRetention::default());
        let writer = store.open_session(meta()).expect("open session");
        play_two_searches(&writer);
        let summary = writer.finish("mate", Some("win")).await.expect("summary");

        // Rewrite the index the way a killed process would have left it: no
        // record for the session that was running, one record still flagged
        // live, and one whose file has been deleted behind our back.
        let mut stale = summary.clone();
        stale.id = "stale".into();
        stale.live = true;
        let mut dangling = summary.clone();
        dangling.id = "dangling".into();
        fs::copy(
            session_file(&root, &summary.id),
            session_file(&root, "stale"),
        )
        .expect("copy session file");
        fs::write(
            index_path(&root),
            format!(
                "{}\n{}\n",
                serde_json::to_string(&stale).expect("serialize"),
                serde_json::to_string(&dangling).expect("serialize")
            ),
        )
        .expect("rewrite index");
        // Both files have to look abandoned; a fresh one is assumed to belong
        // to a QueenUI window that is still playing.
        abandon(&session_file(&root, &summary.id));
        abandon(&session_file(&root, "stale"));

        let reloaded = EngineLogStore::load(&root, LogRetention::default());
        let sessions = reloaded.list(&LogFilter::default());
        assert!(
            !sessions.iter().any(|session| session.id == "dangling"),
            "a record without a file must not be offered"
        );
        let stale = sessions
            .iter()
            .find(|session| session.id == "stale")
            .expect("stale record");
        assert!(!stale.live, "only this process can own a live session");

        let adopted = sessions
            .iter()
            .find(|session| session.id == summary.id)
            .expect("adopted orphan");
        assert_eq!(adopted.line_count, summary.line_count);
        assert_eq!(adopted.search_count, summary.search_count);
        assert_eq!(adopted.started_at_ms, summary.started_at_ms);
        assert_eq!(adopted.bot_username, "QueenBot");
        assert_eq!(adopted.game_id.as_deref(), Some("abc123"));
        assert_eq!(adopted.engine_name, "Stockfish 17");
        // The session-end note survived, so the outcome is not guessed.
        assert_eq!(adopted.status.as_deref(), Some("mate"));
        assert_eq!(adopted.result.as_deref(), Some("win"));
        // Rebuilt from the file, the byte counts still match what the writer
        // reported for the very same session.
        assert_eq!(adopted.raw_bytes, summary.raw_bytes);
        assert_eq!(adopted.compressed_bytes, summary.compressed_bytes);

        // Reconciliation is a one-time cost: what the store ended up with is
        // what the file now holds, dangling record and all.
        let index = index_lines(&root);
        assert_eq!(index.len(), 2, "the record without a file is gone too");
        assert!(index
            .iter()
            .any(|line| line.contains(&format!("\"{}\"", summary.id))));
        assert!(!index.iter().any(|line| line.contains("\"dangling\"")));
        // The leftover live flag was corrected on disk, not only in memory.
        assert!(!index.iter().any(|line| line.contains("\"live\":true")));

        // A second launch has nothing left to reconcile and nothing to warn
        // about, so it must not touch the file again.
        let before = fs::metadata(index_path(&root))
            .and_then(|metadata| metadata.modified())
            .expect("index mtime");
        let again = EngineLogStore::load(&root, LogRetention::default());
        assert_eq!(again.list(&LogFilter::default()).len(), 2);
        assert_eq!(index_lines(&root).len(), 2);
        assert_eq!(
            fs::metadata(index_path(&root))
                .and_then(|metadata| metadata.modified())
                .expect("index mtime"),
            before
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn exports_annotated_plain_and_archive_shapes() {
        let root = temp_root("export");
        let store = EngineLogStore::load(&root, LogRetention::default());
        let writer = store.open_session(meta()).expect("open session");
        writer.sent("uci");
        writer.received("uciok");
        writer.stderr("info: using 4 threads");
        writer.note("bestmove uci=e2e4 elapsed=10");
        let summary = writer.finish("mate", Some("win")).await.expect("summary");

        let annotated = root.join("exports").join("game.uci.txt");
        store
            .export(&summary.id, &annotated, ExportMode::Annotated)
            .expect("annotated export");
        let text = fs::read_to_string(&annotated).expect("read annotated");
        assert!(text.starts_with("# queenui-log: 1\n"));
        assert!(text.contains(&format!("# session: {}\n", summary.id)));
        assert!(text.contains("\t>\tuci\n"));
        assert!(text.contains("\t!\tinfo: using 4 threads\n"));

        let plain = root.join("exports").join("game.uci");
        store
            .export(&summary.id, &plain, ExportMode::Plain)
            .expect("plain export");
        let transcript = fs::read_to_string(&plain).expect("read plain");
        // Only the engine conversation, replayable as-is.
        assert_eq!(transcript, "uci\nuciok\n");

        let archive = root.join("exports").join("game.uci.gz");
        store
            .export(&summary.id, &archive, ExportMode::Archive)
            .expect("archive export");
        assert_eq!(
            fs::read(&archive).expect("read archive"),
            fs::read(session_file(&root, &summary.id)).expect("read session"),
        );
        assert!(store
            .export_bytes_bounded(&summary.id, ExportMode::Annotated, 8)
            .unwrap_err()
            .contains("byte cap"));

        assert!(store
            .export("no-such-session", &archive, ExportMode::Archive)
            .is_err());

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn ignores_writes_after_finish_and_finishes_once() {
        let root = temp_root("finish-once");
        let store = EngineLogStore::load(&root, LogRetention::default());
        let writer = store.open_session(meta()).expect("open session");
        let stderr_handle = writer.clone();
        writer.sent("uci");
        let summary = writer.finish("aborted", None).await.expect("summary");
        assert_eq!(summary.line_count, 2);
        assert_eq!(summary.result, None);

        // The stderr pump can outlive the game task; its writes are dropped.
        stderr_handle.stderr("late line");
        stderr_handle.note("late note");
        writer.sent("late command");
        stderr_handle.describe(SessionDescription {
            opponent: Some("TooLate".into()),
            ..SessionDescription::default()
        });

        assert!(stderr_handle.finish("mate", Some("win")).await.is_none());
        assert!(writer.finish("mate", Some("win")).await.is_none());

        let page = store.page(&summary.id, 0, 0).expect("page");
        assert_eq!(page.total_lines, 2);
        assert!(!page
            .lines
            .iter()
            .any(|line| line.text.starts_with("late") || line.text.contains("TooLate")));
        // One session, one index record: finishing twice never doubles it up.
        assert_eq!(store.list(&LogFilter::default()).len(), 1);
        assert_eq!(
            fs::read_to_string(index_path(&root))
                .expect("read index")
                .lines()
                .count(),
            1
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn describes_a_session_after_it_has_started() {
        let root = temp_root("describe");
        let store = EngineLogStore::load(&root, LogRetention::default());
        let writer = store.open_session(meta()).expect("open session");
        writer.sent("uci");
        writer.received("uciok");
        writer.describe(SessionDescription {
            opponent: Some("Rival".into()),
            opponent_rating: Some(2050),
            color: Some("black".into()),
            clock: Some("180+2".into()),
            initial_fen: Some("startpos".into()),
        });
        // A later patch may know only part of the picture.
        writer.describe(SessionDescription {
            opponent_rating: Some(2075),
            ..SessionDescription::default()
        });
        writer.sent("isready");
        let summary = writer
            .finish("resign", Some("loss"))
            .await
            .expect("summary");

        assert_eq!(summary.opponent.as_deref(), Some("Rival"));
        assert_eq!(summary.color.as_deref(), Some("black"));
        assert_eq!(summary.clock.as_deref(), Some("180+2"));

        let page = store.page(&summary.id, 0, 0).expect("page");
        // The file is self-contained: the patch is in the header view…
        assert_eq!(header_value(&page.header, "opponent"), Some("Rival"));
        assert_eq!(header_value(&page.header, "opponent-rating"), Some("2075"));
        assert_eq!(header_value(&page.header, "color"), Some("black"));
        assert_eq!(header_value(&page.header, "clock"), Some("180+2"));
        assert_eq!(header_value(&page.header, "initial-fen"), Some("startpos"));
        // …and in the transcript, timed where it arrived.
        let note = page
            .lines
            .iter()
            .find(|line| line.text == "meta opponent: Rival")
            .expect("meta note");
        assert_eq!(note.direction, "#");
        assert_eq!(note.index, 2);

        // The index record written at finish carries the real game details.
        let reloaded = EngineLogStore::load(&root, LogRetention::default());
        let stored = reloaded
            .list(&LogFilter {
                query: Some("rival".into()),
                ..LogFilter::default()
            })
            .pop()
            .expect("filtered by opponent");
        assert_eq!(stored.id, summary.id);
        assert_eq!(stored.color.as_deref(), Some("black"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn builds_filesystem_safe_session_ids() {
        assert_eq!(compact_timestamp(1_785_000_000_000), "20260725-172000");
        assert_eq!(iso_timestamp(1_785_000_000_000), "2026-07-25T17:20:00Z");
        assert_eq!(iso_timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(
            sanitize_id("20260725-172000_Queen Bot/x_game:1"),
            "20260725-172000_Queen-Bot-x_game-1"
        );
        assert_eq!(sanitize_id("bot.name_ok-9"), "bot.name_ok-9");

        let root = temp_root("ids");
        let directory = sessions_dir(&root);
        fs::create_dir_all(&directory).expect("create sessions dir");
        let free = HashSet::new();
        let (first, _) = reserve_session_file(&directory, "session", &free).expect("first id");
        assert_eq!(first, "session");
        let (second, _) = reserve_session_file(&directory, "session", &free).expect("second id");
        assert_eq!(second, "session-2");
        // An id the index already claims is skipped even without a file.
        let taken: HashSet<String> = ["session-3".to_string()].into_iter().collect();
        let (third, _) = reserve_session_file(&directory, "session", &taken).expect("third id");
        assert_eq!(third, "session-4");
        assert!(directory.join("session-4.uci.gz").exists());

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn opening_a_session_never_waits_on_index_file_work() {
        let root = temp_root("io-lock");
        let store = EngineLogStore::load(&root, LogRetention::default());
        // Standing in for a prune part-way through deleting a thousand files and
        // fsyncing a rewritten index. That work holds `io` and nothing else, so
        // the next game may start straight through it; holding the index state
        // across it instead would park this thread until the disk was done.
        let busy = lock(&store.inner.io);
        let writer = store
            .open_session(meta())
            .expect("a game starts while the index is being written");
        writer.sent("uci");
        assert_eq!(store.overview().live_count, 1);
        drop(busy);

        let summary = writer.finish("aborted", None).await.expect("summary");
        assert_eq!(index_lines(&root).len(), 1);
        assert_eq!(store.list(&LogFilter::default()).len(), 1);
        assert!(session_file(&root, &summary.id).exists());

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_index_change_and_its_write_are_one_step() {
        let root = temp_root("index-step");
        let store = EngineLogStore::load(&root, LogRetention::default());
        let writer = store.open_session(meta()).expect("open session");
        writer.sent("uci");
        let session_id = store
            .list(&LogFilter::default())
            .first()
            .expect("live session")
            .id
            .clone();

        // Whoever is writing the index owns the in-memory index for the whole
        // step, not just for the part before the disk work. A session that has
        // left the live set without its record being on disk yet is precisely
        // what a rewrite by another thread would erase.
        let busy = lock(&store.inner.io);
        let finishing = tokio::spawn({
            let writer = writer.clone();
            async move { writer.finish("aborted", None).await }
        });
        std::thread::sleep(Duration::from_millis(100));
        assert!(
            store.inner.is_live(&session_id),
            "the session may not leave the live set before its record is written"
        );
        assert!(index_lines(&root).is_empty());
        drop(busy);

        let summary = finishing.await.expect("join").expect("summary");
        assert_eq!(summary.id, session_id);
        assert_eq!(index_lines(&root).len(), 1);
        assert!(index_lines(&root)[0].contains(&format!("\"{session_id}\"")));
        assert!(!store.inner.is_live(&session_id));

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_finishes_keep_every_index_record() {
        let root = temp_root("concurrent-finish");
        let now = now_ms();
        fabricate(&root, "ancient", now - 60 * DAY_MS, 1_000);
        let store = EngineLogStore::load(
            &root,
            LogRetention {
                capture_enabled: true,
                max_total_mb: 0,
                max_age_days: 30,
            },
        );
        let writers: Vec<super::LogWriter> = (0..4)
            .map(|index| {
                let writer = store.open_session(meta()).expect("open session");
                writer.sent(&format!("uci {index}"));
                writer
            })
            .collect();

        // One of these prunes the ancient session and rewrites the index while
        // the others are appending records of their own. Whoever rewrites must
        // carry the records that landed first rather than erase them.
        let finished = tokio::join!(
            writers[0].finish("aborted", None),
            writers[1].finish("aborted", None),
            writers[2].finish("aborted", None),
            writers[3].finish("aborted", None),
        );
        let ids = [
            finished.0.expect("first summary").id,
            finished.1.expect("second summary").id,
            finished.2.expect("third summary").id,
            finished.3.expect("fourth summary").id,
        ];

        let index = index_lines(&root);
        assert_eq!(index.len(), 4, "every record survived the rewrite");
        for id in &ids {
            assert!(
                index.iter().any(|line| line.contains(&format!("\"{id}\""))),
                "{id} is missing from the index"
            );
        }
        assert!(!session_file(&root, "ancient").exists());
        let reloaded = EngineLogStore::load(&root, LogRetention::default());
        assert_eq!(reloaded.list(&LogFilter::default()).len(), 4);

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn reserves_free_ids_from_the_live_set_and_the_disk() {
        let root = temp_root("ids-live");
        let store = EngineLogStore::load(&root, LogRetention::default());
        let first = store.open_session(meta()).expect("first session");
        let second = store.open_session(meta()).expect("second session");
        let live: HashSet<String> = store
            .list(&LogFilter::default())
            .into_iter()
            .map(|session| session.id)
            .collect();
        assert_eq!(live.len(), 2, "two sessions opened at once take two names");

        first.finish("aborted", None).await.expect("first summary");
        second
            .finish("aborted", None)
            .await
            .expect("second summary");
        // The finished ids are no longer scanned, but their files still hold
        // the names: `create_new` is what stops a third session clobbering one.
        let third = store.open_session(meta()).expect("third session");
        let summary = third.finish("aborted", None).await.expect("third summary");
        let ids: HashSet<String> = store
            .list(&LogFilter::default())
            .into_iter()
            .map(|session| session.id)
            .collect();
        assert_eq!(ids.len(), 3);
        assert!(ids.contains(&summary.id));
        for id in &ids {
            assert!(session_file(&root, id).exists(), "{id} has no file");
        }

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn leaves_a_recently_written_orphan_to_the_window_writing_it() {
        let root = temp_root("foreign-live");
        let store = EngineLogStore::load(&root, LogRetention::default());
        let writer = store.open_session(meta()).expect("open session");
        play_two_searches(&writer);
        let summary = writer.finish("mate", Some("win")).await.expect("summary");
        // A file with no index record and a fresh timestamp is what a rated game
        // recording in another QueenUI window looks like from the outside.
        fs::write(index_path(&root), "").expect("empty the index");
        keep_alive(&session_file(&root, &summary.id));

        let second = EngineLogStore::load(&root, LogRetention::default());
        assert!(
            second.list(&LogFilter::default()).is_empty(),
            "a file still being written is not ours to adopt"
        );
        assert_eq!(second.prune(), 0);
        assert_eq!(second.clear().expect("clear"), 0);
        assert!(session_file(&root, &summary.id).exists());

        // Once it has gone quiet for a whole window, the process that owned it
        // is gone and the session is ours to recover.
        abandon(&session_file(&root, &summary.id));
        let third = EngineLogStore::load(&root, LogRetention::default());
        assert_eq!(third.list(&LogFilter::default()).len(), 1);

        // If that window comes back to life, the adopted record is still not
        // something we may delete underneath it.
        keep_alive(&session_file(&root, &summary.id));
        assert_eq!(third.prune(), 0);
        assert_eq!(third.clear().expect("clear"), 0);
        assert!(third.delete(&summary.id).is_err());
        assert!(session_file(&root, &summary.id).exists());

        // And when it falls silent again, nothing protects it any more.
        abandon(&session_file(&root, &summary.id));
        assert_eq!(third.clear().expect("clear"), 1);
        assert!(!session_file(&root, &summary.id).exists());

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn describes_only_what_a_reconnect_actually_changed() {
        let root = temp_root("describe-repeat");
        let store = EngineLogStore::load(&root, LogRetention::default());
        let writer = store.open_session(meta()).expect("open session");
        let full = || SessionDescription {
            opponent: Some("Rival".into()),
            opponent_rating: Some(2050),
            color: Some("black".into()),
            clock: Some("180+2".into()),
            initial_fen: Some("startpos".into()),
        };
        writer.describe(full());
        // Lichess re-sends `gameFull` whenever the stream reconnects.
        writer.describe(full());
        writer.describe(full());
        // A rating that really moved still earns its note.
        writer.describe(SessionDescription {
            opponent_rating: Some(2075),
            ..full()
        });
        let summary = writer
            .finish("resign", Some("loss"))
            .await
            .expect("summary");

        let page = store.page(&summary.id, 0, 0).expect("page");
        let notes: Vec<&str> = page
            .lines
            .iter()
            .filter(|line| line.text.starts_with("meta "))
            .map(|line| line.text.as_str())
            .collect();
        assert_eq!(
            notes,
            [
                "meta opponent: Rival",
                "meta opponent-rating: 2050",
                "meta color: black",
                "meta clock: 180+2",
                "meta initial-fen: startpos",
                "meta opponent-rating: 2075",
            ]
        );
        // One header field per key, so nothing downstream sees a duplicate.
        assert_eq!(
            page.header
                .iter()
                .filter(|field| field.key == "opponent")
                .count(),
            1
        );
        assert_eq!(header_value(&page.header, "opponent-rating"), Some("2075"));

        // A session that opened already knowing all of it has it in the header
        // block; repeating it into the transcript would say nothing new.
        let known = store
            .open_session(SessionMeta {
                opponent: Some("Rival".into()),
                opponent_rating: Some(2050),
                color: Some("black".into()),
                clock: Some("180+2".into()),
                initial_fen: Some("startpos".into()),
                ..meta()
            })
            .expect("open described session");
        known.describe(full());
        let summary = known.finish("resign", Some("loss")).await.expect("summary");
        let page = store.page(&summary.id, 0, 0).expect("page");
        assert!(page
            .lines
            .iter()
            .all(|line| !line.text.starts_with("meta ")));
        assert_eq!(header_value(&page.header, "opponent"), Some("Rival"));

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn keeps_free_text_info_payloads_out_of_the_outline() {
        let root = temp_root("info-free-text");
        let store = EngineLogStore::load(&root, LogRetention::default());
        let writer = store.open_session(meta()).expect("open session");
        writer.note("search ply=0 move=1 color=w wtime=60000 btime=60000 winc=0 binc=0");
        writer.received("info depth 12 score cp 34 nodes 5000 pv e2e4 e7e5");
        // The pv runs to the end of the line: what follows it is moves.
        writer.received("info depth 14 seldepth 22 score cp 41 pv e2e4 depth 96 score mate 2");
        // So do these three, and an engine says whatever it likes in them.
        // Depth and score come from the last line carrying one, so a block that
        // reads them here ends up quoting the chatter rather than the search.
        writer.received("info string NNUE evaluation using nn.nnue depth 99 score mate 1");
        writer.received("info currline 1 e2e4 depth 98 score cp 900");
        writer.received("info refutation d1h5 g8f6 depth 97 score cp 800");
        writer.note("bestmove uci=e2e4 elapsed=900");
        let summary = writer.finish("mate", Some("win")).await.expect("summary");

        let blocks = store.outline(&summary.id).expect("outline");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].depth, Some(14));
        assert_eq!(blocks[0].score_cp, Some(41));
        assert_eq!(blocks[0].mate_in, None);

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn a_sweep_leaves_the_readers_session_in_the_cache() {
        let root = temp_root("cache");
        let store = EngineLogStore::load(&root, LogRetention::default());
        for _ in 0..2 {
            let writer = store.open_session(meta()).expect("open session");
            play_two_searches(&writer);
            writer.finish("mate", Some("win")).await.expect("summary");
        }
        // The sweep visits sessions newest first, so opening the newest one is
        // what an eviction by the sweep's last read would destroy.
        let open_id = store
            .list(&LogFilter::default())
            .first()
            .expect("a session to open")
            .id
            .clone();
        store.page(&open_id, 0, 0).expect("page");
        assert_eq!(cached_session(&store), Some(open_id.clone()));

        let hits = store
            .search_all(
                &LogFilter::default(),
                &LogQuery {
                    text: "bestmove".into(),
                    ..LogQuery::default()
                },
            )
            .expect("sweep");
        assert_eq!(hits.len(), 2);
        assert_eq!(
            cached_session(&store),
            Some(open_id),
            "a cross-session sweep must not evict what the reader has open"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn an_uncapped_page_runs_from_the_offset_to_the_end() {
        let root = temp_root("page-uncapped");
        let store = EngineLogStore::load(&root, LogRetention::default());
        let writer = store.open_session(meta()).expect("open session");
        writer.sent("uci");
        writer.received("uciok");
        writer.sent("isready");
        writer.received("readyok");
        let summary = writer.finish("aborted", None).await.expect("summary");

        let whole = store.page(&summary.id, 0, 0).expect("whole session");
        assert_eq!(whole.lines.len(), 5);
        // Zero is uncapped, not "ignore the offset": the tail is what a viewer
        // that has not chosen a page size asks for.
        let tail = store.page(&summary.id, 3, 0).expect("tail");
        assert_eq!(tail.offset, 3);
        assert_eq!(tail.total_lines, 5);
        assert_eq!(tail.lines.len(), 2);
        assert_eq!(tail.lines[0].index, 3);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn honours_disabled_capture() {
        let root = temp_root("disabled");
        let store = EngineLogStore::load(
            &root,
            LogRetention {
                capture_enabled: false,
                ..LogRetention::default()
            },
        );
        assert!(store.open_session(meta()).is_none());
        let overview = store.overview();
        assert_eq!(overview.session_count, 0);
        assert!(!overview.retention.capture_enabled);
        assert!(sessions_dir(&root).exists());

        let _ = fs::remove_dir_all(root);
    }
}
