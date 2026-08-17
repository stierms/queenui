//! Application diagnostics: the operational events QueenUI used to print to a
//! console that a bundled Windows build does not have.
//!
//! Every `eprintln!` that reports something an operator would want to know
//! about — a dropped Lichess stream, a challenge we could not decline, a game
//! we failed to record — records a [`DiagnosticEntry`] here instead. Entries
//! live in a bounded in-memory ring (so the Logs tab can render them without
//! touching disk) and are appended to `logs/diagnostics.jsonl` (so they survive
//! a restart). Volume is a handful of entries an hour, occasionally a burst
//! during a reconnect storm; the engine's UCI firehose is a different module
//! with different tradeoffs.

use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    fs::{self, File, OpenOptions},
    io::Write,
    panic::{self, AssertUnwindSafe},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

const LOG_DIR: &str = "logs";
const DIAGNOSTICS_FILE: &str = "diagnostics.jsonl";
/// Entries the ring serves to the Logs tab. Roughly a day of a busy session,
/// and small enough that cloning the whole ring for a query stays free.
const MAX_IN_MEMORY: usize = 1_000;
/// Entries kept on disk. Only [`DiagnosticsLog::prune`] enforces this; between
/// prunes the file may run over, which costs nothing but a little disk.
const MAX_PERSISTED: usize = 20_000;
const DAY_MS: u64 = 86_400_000;

/// Milliseconds since the Unix epoch. A clock behind the epoch is impossible in
/// practice and not worth failing a log write over, so it maps to 0.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// Locks without ever propagating a panic. No code path panics while holding
/// one of these locks, and a diagnostics sink that panics would take down the
/// very paths it exists to report on.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Ordering behind the minimum-level filter. Unknown levels rank as `info` so
/// that neither a typo in a filter nor an unrecognized stored level can hide
/// entries an operator is looking for.
fn level_rank(level: &str) -> u8 {
    match level {
        "error" => 2,
        "warn" => 1,
        _ => 0,
    }
}

/// One operational event. `level` is "info" | "warn" | "error" and `source` is
/// "engine" | "lichess" | "campaign" | "storage" | "app"; both stay plain
/// strings so adding a source never invalidates already-persisted lines.
#[derive(Clone, Debug, Deserialize, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticEntry {
    pub id: String,
    pub at_ms: u64,
    pub level: String,
    pub source: String,
    /// The Lichess account the event concerns, when it concerns one.
    #[serde(default)]
    pub account_id: Option<String>,
    #[serde(default)]
    pub game_id: Option<String>,
    /// One line, already phrased for an operator.
    pub message: String,
    /// The underlying error text, stack of causes, or offending payload.
    #[serde(default)]
    pub detail: Option<String>,
}

impl DiagnosticEntry {
    fn new(level: &str, source: &str, message: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            at_ms: now_ms(),
            level: level.into(),
            source: source.into(),
            account_id: None,
            game_id: None,
            message: message.into(),
            detail: None,
        }
    }

    pub fn info(source: &str, message: impl Into<String>) -> Self {
        Self::new("info", source, message)
    }

    pub fn warn(source: &str, message: impl Into<String>) -> Self {
        Self::new("warn", source, message)
    }

    pub fn error(source: &str, message: impl Into<String>) -> Self {
        Self::new("error", source, message)
    }

    pub fn with_account(mut self, account_id: &str) -> Self {
        self.account_id = Some(account_id.to_string());
        self
    }

    pub fn with_game(mut self, game_id: &str) -> Self {
        self.game_id = Some(game_id.to_string());
        self
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticFilter {
    /// MINIMUM level: "info" shows everything, "warn" hides info, "error"
    /// shows only errors.
    #[serde(default)]
    pub level: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub account_id: Option<String>,
    /// Case-insensitive substring over message and detail.
    #[serde(default)]
    pub query: Option<String>,
    /// 0 or absent means "everything in memory".
    #[serde(default)]
    pub limit: Option<usize>,
}

fn matches(
    entry: &DiagnosticEntry,
    filter: &DiagnosticFilter,
    minimum: u8,
    query: Option<&str>,
) -> bool {
    level_rank(&entry.level) >= minimum
        && filter
            .source
            .as_ref()
            .is_none_or(|source| &entry.source == source)
        && filter
            .account_id
            .as_ref()
            .is_none_or(|id| entry.account_id.as_ref() == Some(id))
        && query.is_none_or(|needle| {
            entry.message.to_lowercase().contains(needle)
                || entry
                    .detail
                    .as_deref()
                    .is_some_and(|detail| detail.to_lowercase().contains(needle))
        })
}

/// Keeps only what a prune preserves: entries at or after `cutoff_ms`, then the
/// newest `max_records`. `records` must be in append (oldest first) order.
/// Split out as a pure function so the retention rule is testable without
/// writing MAX_PERSISTED lines to disk.
fn retain_after_prune(
    records: Vec<DiagnosticEntry>,
    cutoff_ms: u64,
    max_records: usize,
) -> Vec<DiagnosticEntry> {
    let mut kept: Vec<DiagnosticEntry> = records
        .into_iter()
        .filter(|record| record.at_ms >= cutoff_ms)
        .collect();
    if kept.len() > max_records {
        kept.drain(..kept.len() - max_records);
    }
    kept
}

/// Atomic rewrite (temp file + rename), the same shape as `storage::save`, so a
/// crash mid-prune can never leave a half-written diagnostics file behind.
fn rewrite(path: &Path, records: &[DiagnosticEntry]) -> Result<(), String> {
    let mut content = String::new();
    for record in records {
        if let Ok(line) = serde_json::to_string(record) {
            content.push_str(&line);
            content.push('\n');
        }
    }
    let temporary = path.with_extension("jsonl.tmp");
    let mut file = File::create(&temporary)
        .map_err(|error| format!("Could not write the diagnostics log: {error}"))?;
    file.write_all(content.as_bytes())
        .map_err(|error| format!("Could not write the diagnostics log: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("Could not flush the diagnostics log to disk: {error}"))?;
    drop(file);
    fs::rename(&temporary, path)
        .map_err(|error| format!("Could not replace the diagnostics log: {error}"))
}

/// Notified for every recorded entry. Held as an `Arc` rather than a `Box` so
/// `record` can clone it out and drop the lock before calling: an observer that
/// itself logs (directly, or by way of a Tauri emit that fails) would otherwise
/// deadlock on a non-reentrant mutex.
type Observer = Arc<dyn Fn(&DiagnosticEntry) + Send + Sync>;

/// Bounded in-memory ring over an append-only JSONL file.
///
/// Three independent locks. No code path holds more than one at a time, which
/// is what makes them deadlock-free: `record` fills the ring, releases it, then
/// takes the writer, releases that, and only then reads the observer slot;
/// `prune` and `clear` finish with the writer before touching the ring.
pub struct DiagnosticsLog {
    path: PathBuf,
    entries: Mutex<VecDeque<DiagnosticEntry>>,
    /// The append handle, opened lazily and dropped again whenever a write
    /// fails or the file is replaced. Kept open rather than reopened per entry
    /// so a `record` costs one write syscall, and deliberately unbuffered: at
    /// a handful of entries an hour a BufWriter would buy nothing and would
    /// lose the most interesting entries — the ones right before a crash.
    ///
    /// A dedicated writer thread was the alternative; it would only pay off
    /// under sustained volume this log will never see, and it would need its
    /// own shutdown handshake to avoid dropping the tail on exit.
    file: Mutex<Option<File>>,
    observer: Mutex<Option<Observer>>,
}

impl DiagnosticsLog {
    pub fn path_in(app_data_dir: &Path) -> PathBuf {
        app_data_dir.join(LOG_DIR).join(DIAGNOSTICS_FILE)
    }

    /// Loads the tail of the log. Never fails hard: an unreadable directory or
    /// file leaves an empty ring, because a diagnostics sink that refuses to
    /// start would suppress exactly the reports explaining why.
    pub fn load(app_data_dir: &Path) -> Self {
        let path = Self::path_in(app_data_dir);
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let mut entries: VecDeque<DiagnosticEntry> = VecDeque::new();
        let mut corrupt = 0usize;
        if let Ok(content) = fs::read_to_string(&path) {
            for line in content.lines() {
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<DiagnosticEntry>(line) {
                    Ok(entry) => {
                        entries.push_back(entry);
                        if entries.len() > MAX_IN_MEMORY {
                            entries.pop_front();
                        }
                    }
                    Err(_) => corrupt += 1,
                }
            }
        }

        let log = Self {
            path,
            entries: Mutex::new(entries),
            file: Mutex::new(None),
            observer: Mutex::new(None),
        };
        if corrupt > 0 {
            // One entry for the whole file: a truncated write usually damages a
            // run of lines at once, and a per-line report would bury the rest.
            log.record(
                DiagnosticEntry::warn(
                    "storage",
                    format!("Skipped {corrupt} unreadable diagnostics line(s) while loading"),
                )
                .with_detail(log.path.display().to_string()),
            );
        }
        log
    }

    /// Records an entry into the ring and appends it to disk. A failed write
    /// degrades to in-memory only — it never panics and never propagates,
    /// since callers are error paths already.
    pub fn record(&self, entry: DiagnosticEntry) -> DiagnosticEntry {
        let line = serde_json::to_string(&entry).map(|line| line + "\n").ok();

        {
            let mut entries = lock(&self.entries);
            entries.push_back(entry.clone());
            while entries.len() > MAX_IN_MEMORY {
                entries.pop_front();
            }
        }

        if let Some(line) = line {
            let mut writer = lock(&self.file);
            if writer.is_none() {
                if let Some(parent) = self.path.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                *writer = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.path)
                    .ok();
            }
            if let Some(file) = writer.as_mut() {
                if file.write_all(line.as_bytes()).is_err() {
                    // Drop the handle so the next record reopens; the usual
                    // cause is the file being moved or deleted underneath us.
                    *writer = None;
                }
            }
        }

        // Cloned out and called with every lock released, so an observer is
        // free to log or to re-enter this method. A panicking observer is
        // swallowed for the same reason a failed write is: callers are already
        // on an error path and must not be taken down by their own reporting.
        let observer = lock(&self.observer).clone();
        if let Some(observer) = observer {
            let _ = panic::catch_unwind(AssertUnwindSafe(|| observer(&entry)));
        }

        entry
    }

    /// Installs the hook the app layer uses to forward entries to the UI,
    /// replacing any previous observer. Kept as a plain closure so this module
    /// never depends on tauri.
    pub fn set_observer(&self, observer: Box<dyn Fn(&DiagnosticEntry) + Send + Sync>) {
        *lock(&self.observer) = Some(Arc::from(observer));
    }

    /// Newest first, capped at `filter.limit` (0 or absent means "all in
    /// memory"). Ring order is insertion order, so entries sharing a
    /// millisecond keep the order they were recorded in.
    pub fn recent(&self, filter: &DiagnosticFilter) -> Vec<DiagnosticEntry> {
        let minimum = filter.level.as_deref().map_or(0, level_rank);
        let query = filter
            .query
            .as_ref()
            .map(|query| query.trim().to_lowercase())
            .filter(|query| !query.is_empty());
        let limit = filter.limit.unwrap_or(0);

        let entries = lock(&self.entries);
        let mut selected = Vec::new();
        for entry in entries.iter().rev() {
            if !matches(entry, filter, minimum, query.as_deref()) {
                continue;
            }
            selected.push(entry.clone());
            if limit > 0 && selected.len() >= limit {
                break;
            }
        }
        selected
    }

    /// Empties the ring and deletes the file. The ring is cleared even when the
    /// file could not be removed: the operator asked for a clean slate, and the
    /// failure is reported back rather than silently half-applied.
    pub fn clear(&self) -> Result<(), String> {
        let result = {
            let mut writer = lock(&self.file);
            *writer = None;
            match fs::remove_file(&self.path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(format!("Could not clear the diagnostics log: {error}")),
            }
        };
        lock(&self.entries).clear();
        result
    }

    /// Drops entries older than `max_age_days` and trims the file to the newest
    /// MAX_PERSISTED records, rewriting it atomically. `max_age_days` of 0 means
    /// "no age limit", so a caller can enforce the size cap alone. Returns how
    /// many persisted records were removed (corrupt lines included: a prune is
    /// the one moment we can garbage-collect them).
    pub fn prune(&self, max_age_days: u32) -> u64 {
        let cutoff_ms = if max_age_days == 0 {
            0
        } else {
            now_ms().saturating_sub(u64::from(max_age_days) * DAY_MS)
        };

        let removed = {
            // The writer lock is held across the whole read-rewrite-rename: a
            // concurrent record then blocks and lands in the new file instead of
            // being lost with the old one. Dropping the handle first is what
            // lets the rename succeed on Windows, where an open file cannot be
            // replaced.
            let mut writer = lock(&self.file);
            *writer = None;

            match fs::read_to_string(&self.path) {
                Ok(content) => {
                    let mut seen = 0u64;
                    let mut records = Vec::new();
                    for line in content.lines() {
                        if line.trim().is_empty() {
                            continue;
                        }
                        seen += 1;
                        if let Ok(entry) = serde_json::from_str::<DiagnosticEntry>(line) {
                            records.push(entry);
                        }
                    }
                    let kept = retain_after_prune(records, cutoff_ms, MAX_PERSISTED);
                    let removed = seen.saturating_sub(kept.len() as u64);
                    // Report 0 when the rewrite failed: the old file survives,
                    // so nothing was actually removed.
                    if removed > 0 && rewrite(&self.path, &kept).is_ok() {
                        removed
                    } else {
                        0
                    }
                }
                Err(_) => 0,
            }
        };

        // The ring must not keep serving entries the prune just deleted.
        lock(&self.entries).retain(|entry| entry.at_ms >= cutoff_ms);
        removed
    }

    /// How many entries the ring holds. The UI reads entries rather than
    /// counts, so this exists for the tests that assert retention behaviour.
    #[cfg(test)]
    pub fn count(&self) -> usize {
        lock(&self.entries).len()
    }
}

/// The process-wide sink. It exists because several of the reports this module
/// replaces come from pure helpers with no access to app state — Lichess
/// payload parsing, and the corrupt-config recovery in `storage`, which runs
/// before app state exists at all. Threading a handle through those would cost
/// more than it buys. Everything else should take a `&DiagnosticsLog`.
static GLOBAL: OnceLock<DiagnosticsLog> = OnceLock::new();

/// Installs the process-wide sink during Tauri setup. A second call is ignored
/// (the sink already handed out `&'static` references) and returns the sink
/// that is actually installed.
pub fn install(log: DiagnosticsLog) -> &'static DiagnosticsLog {
    GLOBAL.get_or_init(|| log)
}

/// The installed sink, or None before setup.
pub fn global() -> Option<&'static DiagnosticsLog> {
    GLOBAL.get()
}

/// Records on the process-wide sink. Before `install` this is a no-op: an early
/// startup diagnostic is worth dropping, never worth panicking over.
pub fn record(entry: DiagnosticEntry) {
    if let Some(log) = global() {
        log.record(entry);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        retain_after_prune, DiagnosticEntry, DiagnosticFilter, DiagnosticsLog, DAY_MS,
        MAX_IN_MEMORY,
    };
    use std::{
        path::PathBuf,
        sync::{Arc, Mutex},
    };

    fn temp_dir() -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("queenui-diagnostics-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).expect("create temp dir");
        directory
    }

    fn messages(entries: &[DiagnosticEntry]) -> Vec<&str> {
        entries.iter().map(|entry| entry.message.as_str()).collect()
    }

    fn all() -> DiagnosticFilter {
        DiagnosticFilter::default()
    }

    #[test]
    fn records_newest_first_and_honours_limit() {
        let directory = temp_dir();
        let log = DiagnosticsLog::load(&directory);

        log.record(DiagnosticEntry::info("app", "first"));
        log.record(DiagnosticEntry::warn("lichess", "second"));
        log.record(DiagnosticEntry::error("engine", "third"));

        assert_eq!(log.count(), 3);
        assert_eq!(messages(&log.recent(&all())), ["third", "second", "first"]);
        assert_eq!(
            messages(&log.recent(&DiagnosticFilter {
                limit: Some(2),
                ..DiagnosticFilter::default()
            })),
            ["third", "second"]
        );
        // 0 means "all in memory", not "none".
        assert_eq!(
            log.recent(&DiagnosticFilter {
                limit: Some(0),
                ..DiagnosticFilter::default()
            })
            .len(),
            3
        );

        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn evicts_oldest_beyond_the_memory_cap_and_reloads_capped() {
        let directory = temp_dir();
        let log = DiagnosticsLog::load(&directory);

        for index in 0..MAX_IN_MEMORY + 10 {
            log.record(DiagnosticEntry::info("app", format!("entry {index}")));
        }

        assert_eq!(log.count(), MAX_IN_MEMORY);
        let recent = log.recent(&all());
        assert_eq!(recent[0].message, format!("entry {}", MAX_IN_MEMORY + 9));
        assert_eq!(recent[MAX_IN_MEMORY - 1].message, "entry 10");

        // Every entry is on disk; only the newest MAX_IN_MEMORY come back.
        let reloaded = DiagnosticsLog::load(&directory);
        assert_eq!(reloaded.count(), MAX_IN_MEMORY);
        assert_eq!(
            reloaded.recent(&all())[0].message,
            format!("entry {}", MAX_IN_MEMORY + 9)
        );

        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn level_filter_is_a_minimum_not_an_equality() {
        let directory = temp_dir();
        let log = DiagnosticsLog::load(&directory);

        log.record(DiagnosticEntry::info("app", "chatter"));
        log.record(DiagnosticEntry::warn("lichess", "reconnecting"));
        log.record(DiagnosticEntry::error("engine", "exited"));

        let at_least = |level: &str| {
            messages(&log.recent(&DiagnosticFilter {
                level: Some(level.into()),
                ..DiagnosticFilter::default()
            }))
            .len()
        };
        assert_eq!(at_least("info"), 3);
        assert_eq!(at_least("warn"), 2);
        assert_eq!(at_least("error"), 1);
        assert_eq!(
            messages(&log.recent(&DiagnosticFilter {
                level: Some("warn".into()),
                ..DiagnosticFilter::default()
            })),
            ["exited", "reconnecting"]
        );

        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn filters_by_source_account_and_case_insensitive_query() {
        let directory = temp_dir();
        let log = DiagnosticsLog::load(&directory);

        log.record(
            DiagnosticEntry::warn("lichess", "Game stream dropped")
                .with_account("acct-1")
                .with_game("game0001")
                .with_detail("HTTP 429 Too Many Requests"),
        );
        log.record(DiagnosticEntry::error("engine", "Engine exited").with_account("acct-2"));
        log.record(DiagnosticEntry::info("lichess", "Challenge declined").with_account("acct-1"));

        let count = |filter: DiagnosticFilter| log.recent(&filter).len();
        assert_eq!(
            count(DiagnosticFilter {
                source: Some("lichess".into()),
                ..DiagnosticFilter::default()
            }),
            2
        );
        assert_eq!(
            count(DiagnosticFilter {
                account_id: Some("acct-2".into()),
                ..DiagnosticFilter::default()
            }),
            1
        );
        // Case-insensitive over the message …
        assert_eq!(
            count(DiagnosticFilter {
                query: Some("STREAM".into()),
                ..DiagnosticFilter::default()
            }),
            1
        );
        // … and over the detail.
        assert_eq!(
            count(DiagnosticFilter {
                query: Some("too many".into()),
                ..DiagnosticFilter::default()
            }),
            1
        );
        assert_eq!(
            count(DiagnosticFilter {
                query: Some("nothing matches this".into()),
                ..DiagnosticFilter::default()
            }),
            0
        );
        // Filters combine.
        assert_eq!(
            count(DiagnosticFilter {
                level: Some("warn".into()),
                source: Some("lichess".into()),
                account_id: Some("acct-1".into()),
                query: Some("dropped".into()),
                limit: Some(0),
            }),
            1
        );

        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn reload_restores_entries_and_skips_a_corrupt_line() {
        let directory = temp_dir();
        let path = DiagnosticsLog::path_in(&directory);
        {
            let log = DiagnosticsLog::load(&directory);
            log.record(DiagnosticEntry::info("app", "started"));
            log.record(DiagnosticEntry::error("storage", "could not record a game"));
        }

        let mut content = std::fs::read_to_string(&path).expect("read diagnostics");
        content.push_str("{ this is not json\n");
        std::fs::write(&path, content).expect("write diagnostics");

        let reloaded = DiagnosticsLog::load(&directory);
        // Both readable entries, plus one warn about the line that was skipped.
        assert_eq!(reloaded.count(), 3);
        let recent = reloaded.recent(&all());
        assert!(recent[0].message.contains("Skipped 1 unreadable"));
        assert_eq!(recent[0].level, "warn");
        assert_eq!(recent[1].message, "could not record a game");
        assert_eq!(recent[2].message, "started");
        // The warn about the corrupt line is itself persisted.
        assert_eq!(DiagnosticsLog::load(&directory).count(), 4);

        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn prune_removes_only_old_entries_and_rewrites_the_file() {
        let directory = temp_dir();
        let log = DiagnosticsLog::load(&directory);

        let now = super::now_ms();
        let mut ancient = DiagnosticEntry::info("app", "ancient");
        ancient.at_ms = now - 10 * DAY_MS;
        log.record(ancient);
        let mut yesterday = DiagnosticEntry::warn("lichess", "yesterday");
        yesterday.at_ms = now - DAY_MS;
        log.record(yesterday);
        log.record(DiagnosticEntry::error("engine", "just now"));

        assert_eq!(log.prune(7), 1);
        assert_eq!(messages(&log.recent(&all())), ["just now", "yesterday"]);

        // The rewritten file reloads cleanly and holds exactly the survivors.
        let reloaded = DiagnosticsLog::load(&directory);
        assert_eq!(
            messages(&reloaded.recent(&all())),
            ["just now", "yesterday"]
        );

        // Nothing left to remove.
        assert_eq!(log.prune(7), 0);
        // Recording after a prune still lands on disk.
        log.record(DiagnosticEntry::info("app", "after prune"));
        assert_eq!(DiagnosticsLog::load(&directory).count(), 3);

        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn prune_retention_keeps_the_newest_records() {
        let record = |at_ms: u64| {
            let mut entry = DiagnosticEntry::info("app", format!("at {at_ms}"));
            entry.at_ms = at_ms;
            entry
        };
        let kept = retain_after_prune(vec![record(10), record(20), record(30), record(40)], 20, 2);
        assert_eq!(messages(&kept), ["at 30", "at 40"]);
        // No age limit: only the size cap applies.
        let kept = retain_after_prune(vec![record(10), record(20), record(30)], 0, 2);
        assert_eq!(messages(&kept), ["at 20", "at 30"]);
    }

    #[test]
    fn observer_sees_every_stored_entry() {
        let directory = temp_dir();
        let log = DiagnosticsLog::load(&directory);

        let seen: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        log.set_observer(Box::new(move |entry| {
            sink.lock()
                .expect("observer lock")
                .push((entry.id.clone(), entry.message.clone()));
        }));

        let first = log.record(DiagnosticEntry::info("app", "first"));
        let second = log.record(DiagnosticEntry::error("engine", "second"));

        let seen = seen.lock().expect("observer lock").clone();
        assert_eq!(
            seen,
            vec![
                (first.id.clone(), "first".to_string()),
                (second.id.clone(), "second".to_string()),
            ]
        );

        // A panicking observer must not take the recorder down with it.
        log.set_observer(Box::new(|_| panic!("observer blew up")));
        log.record(DiagnosticEntry::warn("app", "third"));
        assert_eq!(log.count(), 3);

        let _ = std::fs::remove_dir_all(directory);
    }

    /// The only test that touches the process-wide sink: a `OnceLock` is shared
    /// by every test in the binary, so install ordering has to be asserted in
    /// one place.
    ///
    /// Every other module records into that same sink once it exists, and the
    /// test binary runs them concurrently, so entries are identified by unique
    /// marker text rather than by counting what the sink holds.
    #[test]
    fn global_sink_installs_once_and_ignores_early_records() {
        const EARLY: &str = "global-test marker: before install";
        const AFTER: &str = "global-test marker: after install";
        const SECOND: &str = "global-test marker: after the second install";

        // Another test in this binary may have installed the global sink
        // first; the early-drop property is only observable when this test
        // wins that race, so it is asserted conditionally.
        let sink_already_installed = super::global().is_some();

        // Dropped rather than panicking.
        super::record(DiagnosticEntry::error("storage", EARLY));

        let directory = temp_dir();
        let installed = super::install(DiagnosticsLog::load(&directory));
        assert!(std::ptr::eq(installed, super::global().expect("global")));
        if !sink_already_installed {
            assert!(!messages(&installed.recent(&all())).contains(&EARLY));
        }

        super::record(DiagnosticEntry::warn("lichess", AFTER));
        assert!(messages(&installed.recent(&all())).contains(&AFTER));

        // A second install keeps the first sink, so the second directory never
        // receives anything.
        let other = temp_dir();
        let again = super::install(DiagnosticsLog::load(&other));
        assert!(std::ptr::eq(again, installed));
        super::record(DiagnosticEntry::info("app", SECOND));
        assert!(messages(&installed.recent(&all())).contains(&SECOND));
        assert_eq!(DiagnosticsLog::load(&other).count(), 0);

        let _ = std::fs::remove_dir_all(directory);
        let _ = std::fs::remove_dir_all(other);
    }

    #[test]
    fn clear_empties_memory_and_file() {
        let directory = temp_dir();
        let log = DiagnosticsLog::load(&directory);

        log.record(DiagnosticEntry::info("app", "first"));
        log.record(DiagnosticEntry::warn("lichess", "second"));
        log.clear().expect("clear");

        assert_eq!(log.count(), 0);
        assert!(log.recent(&all()).is_empty());
        assert!(!DiagnosticsLog::path_in(&directory).exists());
        assert_eq!(DiagnosticsLog::load(&directory).count(), 0);

        // The append handle was dropped, so the next record reopens the file.
        log.record(DiagnosticEntry::info("app", "after clear"));
        let reloaded = DiagnosticsLog::load(&directory);
        assert_eq!(messages(&reloaded.recent(&all())), ["after clear"]);

        let _ = std::fs::remove_dir_all(directory);
    }
}
