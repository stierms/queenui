use crate::enginelog::LogWriter;
use crate::models::{EngineProfile, EngineTelemetry, UciOption};
use crate::position::LivePosition;
use std::{
    ffi::OsString,
    path::Path,
    process::Stdio,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::{OwnedSemaphorePermit, Semaphore},
    task::JoinHandle,
    time::timeout,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(8);
const STOP_GRACE: Duration = Duration::from_millis(1500);
const ENGINE_STDOUT_LINE_CAP: usize = 1024 * 1024;
const ENGINE_STDERR_LINE_CAP: usize = 64 * 1024;
/// Extra virtual address space for file-backed maps (Syzygy WDL/DTZ).
///
/// `total_memory_mb` is the Hash/RSS budget. Linux `RLIMIT_AS` and Windows Job
/// `ProcessMemoryLimit` count address space: a 6-piece `.rtbw` is 0.3–2.0 GiB
/// of VAS even when almost none of it is resident. Using the Hash slice as
/// `RLIMIT_AS` made `mmap` return ENOMEM while the machine still had free RAM.
const TABLEBASE_ADDRESS_SPACE_BYTES: u64 = 32 * 1024 * 1024 * 1024;
const ENGINE_ENV_ALLOWLIST: &[&str] = &[
    "PATH",
    "PATHEXT",
    "SystemRoot",
    "WINDIR",
    "TEMP",
    "TMP",
    "TMPDIR",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TZ",
];

pub(crate) fn unix_time_ms() -> u64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    u64::try_from(millis).unwrap_or(u64::MAX)
}

#[derive(Clone, Debug)]
pub struct EngineLimits {
    pub simultaneous_engines: usize,
    /// Aggregate Hash/RSS budget. Process address space is this slice plus
    /// [`TABLEBASE_ADDRESS_SPACE_BYTES`].
    pub total_memory_mb: u64,
    pub total_cpu_threads: usize,
    pub total_tasks: usize,
    pub output_bytes_per_engine_per_second: u64,
    pub total_output_bytes_per_second: u64,
    pub output_bytes_per_engine: u64,
}

impl Default for EngineLimits {
    fn default() -> Self {
        let cpus = std::thread::available_parallelism().map_or(1, usize::from);
        Self {
            simultaneous_engines: cpus.clamp(1, 16),
            total_memory_mb: 16 * 1024,
            total_cpu_threads: cpus,
            total_tasks: 256,
            output_bytes_per_engine_per_second: 1024 * 1024,
            total_output_bytes_per_second: 4 * 1024 * 1024,
            output_bytes_per_engine: 64 * 1024 * 1024,
        }
    }
}

impl EngineLimits {
    pub fn validate(&self) -> Result<(), String> {
        if self.simultaneous_engines == 0
            || self.total_memory_mb < 256
            || self.total_cpu_threads == 0
            || self.total_tasks == 0
            || self.output_bytes_per_engine_per_second < 1024
            || self.total_output_bytes_per_second < self.output_bytes_per_engine_per_second
            || self.output_bytes_per_engine < 1024
        {
            return Err("Engine resource limits must all be positive and safely bounded".into());
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct EngineGovernor {
    limits: EngineLimits,
    admission: Arc<Semaphore>,
    output: Arc<Mutex<OutputWindow>>,
}

struct OutputWindow {
    started: Instant,
    bytes: u64,
}

impl EngineGovernor {
    pub fn new(limits: EngineLimits) -> Result<Self, String> {
        limits.validate()?;
        Ok(Self {
            admission: Arc::new(Semaphore::new(limits.simultaneous_engines)),
            output: Arc::new(Mutex::new(OutputWindow {
                started: Instant::now(),
                bytes: 0,
            })),
            limits,
        })
    }

    async fn acquire(&self, configured_options: &[UciOption]) -> Result<EngineLease, String> {
        validate_configured_resources(configured_options, &self.limits)?;
        let permit = self
            .admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| "The runner-wide simultaneous-engine limit is full".to_string())?;
        Ok(EngineLease {
            _permit: permit,
            meter: OutputMeter {
                aggregate: self.output.clone(),
                aggregate_rate: self.limits.total_output_bytes_per_second,
                engine: Arc::new(Mutex::new(OutputWindow {
                    started: Instant::now(),
                    bytes: 0,
                })),
                engine_rate: self.limits.output_bytes_per_engine_per_second,
                per_engine_total: self.limits.output_bytes_per_engine,
                consumed: Arc::new(AtomicU64::new(0)),
            },
            address_space_bytes: per_engine_address_space_bytes(&self.limits),
            task_limit: (self.limits.total_tasks / self.limits.simultaneous_engines).max(1),
        })
    }
}

impl Default for EngineGovernor {
    fn default() -> Self {
        Self::new(EngineLimits::default()).expect("default engine limits are valid")
    }
}

struct EngineLease {
    _permit: OwnedSemaphorePermit,
    meter: OutputMeter,
    address_space_bytes: u64,
    task_limit: usize,
}

fn per_engine_hash_bytes(limits: &EngineLimits) -> u64 {
    limits.total_memory_mb.saturating_mul(1024 * 1024) / limits.simultaneous_engines as u64
}

fn per_engine_hash_mb(limits: &EngineLimits) -> u64 {
    (limits.total_memory_mb / limits.simultaneous_engines as u64).max(1)
}

fn per_engine_address_space_bytes(limits: &EngineLimits) -> u64 {
    per_engine_hash_bytes(limits).saturating_add(TABLEBASE_ADDRESS_SPACE_BYTES)
}

#[derive(Clone)]
struct OutputMeter {
    aggregate: Arc<Mutex<OutputWindow>>,
    aggregate_rate: u64,
    engine: Arc<Mutex<OutputWindow>>,
    engine_rate: u64,
    per_engine_total: u64,
    consumed: Arc<AtomicU64>,
}

impl OutputMeter {
    fn charge(&self, bytes: usize) -> Result<(), String> {
        let bytes = bytes as u64;
        let consumed = self.consumed.fetch_add(bytes, Ordering::Relaxed) + bytes;
        if consumed > self.per_engine_total {
            return Err("Engine output exceeded its total byte ceiling".into());
        }
        let mut engine = self
            .engine
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if engine.started.elapsed() >= Duration::from_secs(1) {
            engine.started = Instant::now();
            engine.bytes = 0;
        }
        engine.bytes = engine.bytes.saturating_add(bytes);
        if engine.bytes > self.engine_rate {
            return Err("Engine output exceeded its per-engine rate ceiling".into());
        }
        drop(engine);
        let mut aggregate = self
            .aggregate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if aggregate.started.elapsed() >= Duration::from_secs(1) {
            aggregate.started = Instant::now();
            aggregate.bytes = 0;
        }
        aggregate.bytes = aggregate.bytes.saturating_add(bytes);
        if aggregate.bytes > self.aggregate_rate {
            return Err("Aggregate engine output exceeded its rate ceiling".into());
        }
        Ok(())
    }
}

fn validate_configured_resources(
    options: &[UciOption],
    limits: &EngineLimits,
) -> Result<(), String> {
    let per_engine_threads = (limits.total_cpu_threads / limits.simultaneous_engines).max(1) as u64;
    let per_engine_hash = per_engine_hash_mb(limits);
    for option in options {
        let Some(value) = option.value.as_deref() else {
            continue;
        };
        let Ok(value) = value.parse::<u64>() else {
            continue;
        };
        if option.name.eq_ignore_ascii_case("Threads") && value > per_engine_threads {
            return Err(
                "The configured engine Threads value exceeds the server-owned CPU ceiling".into(),
            );
        }
        if option.name.eq_ignore_ascii_case("Hash") && value > per_engine_hash {
            return Err(
                "The configured engine Hash value exceeds the server-owned memory ceiling".into(),
            );
        }
    }
    Ok(())
}

pub async fn probe(path: &str) -> Result<EngineProfile, String> {
    probe_with_governor(path, &EngineGovernor::default()).await
}

pub async fn probe_with_governor(
    path: &str,
    governor: &EngineGovernor,
) -> Result<EngineProfile, String> {
    if !Path::new(path).is_file() {
        return Err("The selected engine executable does not exist.".into());
    }

    let mut engine = UciEngine::start_governed(path, &[], None, governor).await?;
    let profile = EngineProfile {
        id: Uuid::new_v4().to_string(),
        name: engine.name.clone().unwrap_or_else(|| {
            Path::new(path)
                .file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or("UCI engine")
                .to_string()
        }),
        path: path.to_string(),
        author: engine.author.clone(),
        option_count: engine.option_count,
        last_probed_at_ms: Some(unix_time_ms()),
        probe_ok: Some(true),
        options: engine.options.clone(),
        opening_book: None,
    };
    engine.shutdown().await;
    Ok(profile)
}

pub struct SearchResult {
    pub best_move: String,
    pub last_info: Option<String>,
    pub telemetry: Option<EngineTelemetry>,
}

pub struct UciEngine {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    stderr_task: Option<JoinHandle<()>>,
    process_tree: ProcessTree,
    lease: Option<EngineLease>,
    output_violation: CancellationToken,
    name: Option<String>,
    author: Option<String>,
    option_count: usize,
    options: Vec<UciOption>,
    /// Flight recorder for this engine process, when logging is enabled.
    log: Option<LogWriter>,
}

struct SearchProgress {
    last_info: Option<String>,
    telemetry: Option<EngineTelemetry>,
    last_published_raw: Option<String>,
    last_publish: Instant,
}

impl SearchProgress {
    fn new() -> Self {
        Self {
            last_info: None,
            telemetry: None,
            last_published_raw: None,
            last_publish: Instant::now()
                .checked_sub(Duration::from_secs(1))
                .unwrap_or_else(Instant::now),
        }
    }
}

impl UciEngine {
    pub async fn start(
        path: &str,
        configured_options: &[UciOption],
        log: Option<LogWriter>,
    ) -> Result<Self, String> {
        Self::start_governed(path, configured_options, log, &EngineGovernor::default()).await
    }

    pub async fn start_governed(
        path: &str,
        configured_options: &[UciOption],
        log: Option<LogWriter>,
        governor: &EngineGovernor,
    ) -> Result<Self, String> {
        let lease = governor.acquire(configured_options).await?;
        let output_violation = CancellationToken::new();
        let mut command = Command::new(path);
        command
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Piping stderr obliges us to drain it, or a chatty engine blocks
            // once the pipe buffer fills. It is only worth piping when there
            // is a recorder to drain it into.
            .stderr(if log.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .kill_on_drop(true);
        for (name, value) in allowed_engine_environment() {
            command.env(name, value);
        }

        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.as_std_mut().process_group(0);
            let address_space_bytes = lease.address_space_bytes;
            let task_limit = lease.task_limit;
            let task_ceiling = match current_user_task_count() {
                Ok(current_tasks) => Some(nproc_ceiling(current_tasks, task_limit)),
                Err(error) => {
                    crate::diagnostics::record(
                        crate::diagnostics::DiagnosticEntry::warn(
                            "engine",
                            "Skipped the engine task ceiling because the user task baseline could not be counted",
                        )
                        .with_detail(error),
                    );
                    None
                }
            };
            // SAFETY: pre_exec runs after fork and only invokes async-signal-
            // safe libc resource-limit operations before exec.
            unsafe {
                command.as_std_mut().pre_exec(move || {
                    let memory = libc::rlimit {
                        rlim_cur: address_space_bytes,
                        rlim_max: address_space_bytes,
                    };
                    if libc::setrlimit(libc::RLIMIT_AS, &memory) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if let Some(task_ceiling) = task_ceiling {
                        let tasks = libc::rlimit {
                            rlim_cur: task_ceiling,
                            rlim_max: task_ceiling,
                        };
                        if libc::setrlimit(libc::RLIMIT_NPROC, &tasks) != 0 {
                            return Err(std::io::Error::last_os_error());
                        }
                    }
                    Ok(())
                });
            }
        }

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.as_std_mut().creation_flags(0x0800_0000);
        }

        let mut child = command
            .spawn()
            .map_err(|error| format!("Could not launch the engine: {error}"))?;
        let process_tree =
            ProcessTree::attach(&child, lease.address_space_bytes, lease.task_limit)?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "Engine stdin is unavailable".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Engine stdout is unavailable".to_string())?;
        let stderr_task = if let (Some(stderr), Some(recorder)) = (child.stderr.take(), log.clone())
        {
            // Engine stderr carries the diagnostics that never reach the UCI
            // protocol — missing network files, option complaints, panics.
            //
            // Read raw bytes rather than lines: a single non-UTF-8 byte (a
            // Windows-1252 path in an error message, an assertion dump) makes
            // `Lines` yield an error, and stopping there would leave a piped
            // stream nobody drains. The engine would then block inside
            // `write` once the pipe buffer filled — mid-search, in a rated
            // game. Lossy decoding keeps the drain running whatever arrives.
            let meter = lease.meter.clone();
            let violation = output_violation.clone();
            Some(tokio::spawn(async move {
                let mut stderr = BufReader::new(stderr);
                loop {
                    match read_capped_line(&mut stderr, ENGINE_STDERR_LINE_CAP).await {
                        Ok(Some(line)) => {
                            if meter.charge(line.len()).is_err() {
                                violation.cancel();
                                return;
                            }
                            recorder
                                .stderr(&sanitize_engine_output(&String::from_utf8_lossy(&line)));
                        }
                        Ok(None) => return,
                        Err(error) => {
                            recorder
                                .stderr(&format!("[QueenUI stopped oversized stderr: {error}]"));
                            return;
                        } // An I/O error means the pipe itself is gone; there is
                          // nothing left to drain.
                    }
                }
            }))
        } else {
            None
        };
        let mut engine = Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            stderr_task,
            process_tree,
            lease: Some(lease),
            output_violation,
            name: None,
            author: None,
            option_count: 0,
            options: Vec::new(),
            log,
        };

        engine.send("uci").await?;
        timeout(HANDSHAKE_TIMEOUT, engine.read_handshake())
            .await
            .map_err(|_| {
                "The engine did not complete its UCI handshake within 8 seconds".to_string()
            })??;
        engine.apply_options(configured_options).await?;
        engine.ready().await?;
        engine.send("ucinewgame").await?;
        engine.ready().await?;
        Ok(engine)
    }

    async fn read_handshake(&mut self) -> Result<(), String> {
        loop {
            let line = self.next_line().await?;
            if let Some(value) = line.strip_prefix("id name ") {
                self.name = Some(value.trim().to_string());
            } else if let Some(value) = line.strip_prefix("id author ") {
                self.author = Some(value.trim().to_string());
            } else if let Some(option) = parse_uci_option(&line) {
                self.option_count += 1;
                self.options.push(option);
            } else if line == "uciok" {
                return Ok(());
            }
        }
    }

    async fn apply_options(&mut self, options: &[UciOption]) -> Result<(), String> {
        for option in options {
            if option.option_type == "button" {
                continue;
            }
            let Some(value) = option.value.as_deref() else {
                continue;
            };
            validate_option_value(option, Some(value))?;
            self.send(&format!("setoption name {} value {value}", option.name))
                .await?;
        }
        Ok(())
    }

    async fn ready(&mut self) -> Result<(), String> {
        self.send("isready").await?;
        timeout(HANDSHAKE_TIMEOUT, async {
            loop {
                if self.next_line().await? == "readyok" {
                    return Ok(());
                }
            }
        })
        .await
        .map_err(|_| "The engine did not become ready within 8 seconds".to_string())?
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn best_move<F>(
        &mut self,
        initial_fen: &str,
        moves: &str,
        white_time: i64,
        black_time: i64,
        white_increment: i64,
        black_increment: i64,
        mut on_info: F,
    ) -> Result<SearchResult, String>
    where
        F: FnMut(EngineTelemetry),
    {
        let live_position = LivePosition::parse(initial_fen, moves)?;
        self.ready().await?;
        self.send(&live_position.uci_position_command()).await?;
        self.send(&format!(
            "go wtime {} btime {} winc {} binc {}",
            white_time.max(0),
            black_time.max(0),
            white_increment.max(0),
            black_increment.max(0),
        ))
        .await?;

        let watchdog = search_watchdog(&live_position, white_time, black_time);
        let mut progress = SearchProgress::new();
        match timeout(
            watchdog,
            self.read_search_result(&live_position, &mut progress, &mut on_info),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => {
                self.send("stop").await?;
                timeout(
                    STOP_GRACE,
                    self.read_search_result(&live_position, &mut progress, &mut on_info),
                )
                .await
                .map_err(|_| {
                    format!(
                        "The engine ignored stop after the {} ms safety watchdog",
                        watchdog.as_millis()
                    )
                })?
            }
        }
    }

    async fn read_search_result<F>(
        &mut self,
        position: &LivePosition,
        progress: &mut SearchProgress,
        on_info: &mut F,
    ) -> Result<SearchResult, String>
    where
        F: FnMut(EngineTelemetry),
    {
        loop {
            let line = self.next_line().await?;
            if line.starts_with("info ") {
                if let Some(parsed) = parse_uci_info(&line) {
                    if progress.last_publish.elapsed() >= Duration::from_millis(120) {
                        progress.last_published_raw = Some(parsed.raw.clone());
                        on_info(parsed.clone());
                        progress.last_publish = Instant::now();
                    }
                    progress.telemetry = Some(parsed);
                }
                progress.last_info = Some(line);
                continue;
            }
            if let Some(value) = line.strip_prefix("bestmove ") {
                let best_move = value.split_whitespace().next().unwrap_or_default();
                if best_move.is_empty() || best_move == "(none)" || best_move == "0000" {
                    return Err("The engine reported no legal move".to_string());
                }
                let best_move = position.canonical_legal_move(best_move)?;
                if let Some(final_telemetry) = progress.telemetry.as_ref() {
                    if progress.last_published_raw.as_deref() != Some(&final_telemetry.raw) {
                        on_info(final_telemetry.clone());
                    }
                }
                return Ok(SearchResult {
                    best_move,
                    last_info: progress.last_info.clone(),
                    telemetry: progress.telemetry.clone(),
                });
            }
        }
    }

    async fn send(&mut self, command: &str) -> Result<(), String> {
        if let Some(log) = &self.log {
            log.sent(command);
        }
        self.stdin
            .write_all(format!("{command}\n").as_bytes())
            .await
            .map_err(|error| format!("Could not write to the engine: {error}"))?;
        self.stdin
            .flush()
            .await
            .map_err(|error| format!("Could not flush the engine command: {error}"))
    }

    async fn next_line(&mut self) -> Result<String, String> {
        let result = tokio::select! {
            biased;
            _ = self.output_violation.cancelled() => {
                Err("Engine output exceeded a server-owned rate or byte ceiling".into())
            }
            result = read_capped_line(&mut self.stdout, ENGINE_STDOUT_LINE_CAP) => result,
        };
        let bytes = match result {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return Err("The engine process exited unexpectedly".into()),
            Err(error) => {
                self.process_tree.terminate();
                return Err(error);
            }
        };
        if let Some(lease) = &self.lease {
            if let Err(error) = lease.meter.charge(bytes.len()) {
                self.process_tree.terminate();
                return Err(error);
            }
        }
        let line = match String::from_utf8(bytes) {
            Ok(line) => line,
            Err(_) => {
                self.process_tree.terminate();
                return Err("The engine emitted non-UTF-8 UCI output".into());
            }
        };
        let line = sanitize_engine_output(&line);
        if let Some(log) = &self.log {
            log.received(&line);
        }
        Ok(line)
    }

    pub async fn shutdown(&mut self) {
        let _ = self.send("quit").await;
        let exited = timeout(Duration::from_secs(2), self.child.wait())
            .await
            .is_ok();
        // Closing/killing the tree also catches helpers that outlived a
        // cooperative direct-child exit.
        self.process_tree.terminate();
        if !exited {
            let _ = self.child.kill().await;
            let _ = timeout(Duration::from_secs(1), self.child.wait()).await;
        }
        if let Some(mut task) = self.stderr_task.take() {
            if timeout(Duration::from_secs(1), &mut task).await.is_err() {
                task.abort();
                let _ = task.await;
            }
        }
        self.lease.take();
    }
}

fn sanitize_engine_output(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character == '\t' || !character.is_control() {
                character
            } else {
                '\u{fffd}'
            }
        })
        .collect()
}

impl Drop for UciEngine {
    fn drop(&mut self) {
        self.process_tree.terminate();
        if let Some(task) = self.stderr_task.take() {
            task.abort();
        }
    }
}

fn allowed_engine_environment() -> Vec<(&'static str, OsString)> {
    ENGINE_ENV_ALLOWLIST
        .iter()
        .filter_map(|name| std::env::var_os(name).map(|value| (*name, value)))
        .collect()
}

#[cfg(unix)]
fn nproc_ceiling(current_task_count: usize, task_limit: usize) -> libc::rlim_t {
    current_task_count
        .saturating_add(task_limit)
        .try_into()
        .unwrap_or(libc::RLIM_INFINITY)
}

#[cfg(unix)]
fn current_user_task_count() -> Result<usize, String> {
    // WSL's RLIMIT_NPROC accounting can exceed the per-UID tasks visible in
    // procfs. An understated baseline would recreate the failure this guard
    // is meant to prevent, so use the diagnostic fail-open path instead.
    if std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .is_ok_and(|release| release.to_ascii_lowercase().contains("microsoft"))
    {
        return Err(
            "WSL procfs does not expose a per-UID task count that reliably matches RLIMIT_NPROC accounting"
                .into(),
        );
    }

    // RLIMIT_NPROC is charged to the real UID and counts threads, so sum the
    // Threads fields for every process leader owned by this user. Reading
    // procfs here, in the parent, keeps pre_exec limited to setrlimit calls.
    // SAFETY: getuid has no preconditions and runs in the parent process.
    let real_uid = unsafe { libc::getuid() };
    let processes = std::fs::read_dir("/proc")
        .map_err(|error| format!("Could not enumerate /proc: {error}"))?;
    let mut task_count = 0usize;

    for process in processes {
        let process = process.map_err(|error| format!("Could not enumerate /proc: {error}"))?;
        let process_id = process.file_name();
        let Some(process_id) = process_id.to_str() else {
            continue;
        };
        if process_id.is_empty()
            || !process_id
                .as_bytes()
                .iter()
                .all(|byte| byte.is_ascii_digit())
        {
            continue;
        }

        let status_path = process.path().join("status");
        let status = match std::fs::read_to_string(&status_path) {
            Ok(status) => status,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "Could not read {} while counting user tasks: {error}",
                    status_path.display()
                ));
            }
        };
        let owner = status
            .lines()
            .find_map(|line| {
                line.strip_prefix("Uid:")?
                    .split_whitespace()
                    .next()?
                    .parse::<libc::uid_t>()
                    .ok()
            })
            .ok_or_else(|| format!("{} has no readable real UID", status_path.display()))?;
        if owner != real_uid {
            continue;
        }
        let process_tasks = status
            .lines()
            .find_map(|line| {
                line.strip_prefix("Threads:")?
                    .split_whitespace()
                    .next()?
                    .parse::<usize>()
                    .ok()
            })
            .ok_or_else(|| format!("{} has no readable thread count", status_path.display()))?;
        task_count = task_count
            .checked_add(process_tasks)
            .ok_or_else(|| "The current user task count overflowed usize".to_string())?;
    }

    if task_count == 0 {
        return Err("No tasks for the spawning user were visible in /proc".into());
    }
    Ok(task_count)
}

async fn read_capped_line<R>(reader: &mut R, cap: usize) -> Result<Option<Vec<u8>>, String>
where
    R: AsyncBufRead + Unpin,
{
    let mut line = Vec::new();
    loop {
        let available = reader
            .fill_buf()
            .await
            .map_err(|error| format!("Could not read engine output: {error}"))?;
        if available.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                Ok(Some(line))
            };
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let take = newline.unwrap_or(available.len());
        if line.len().saturating_add(take) > cap {
            let consumed = newline.map_or(available.len(), |index| index + 1);
            reader.consume(consumed);
            if newline.is_none() {
                // Drain the remainder without ever extending the allocation.
                loop {
                    let rest = reader
                        .fill_buf()
                        .await
                        .map_err(|error| format!("Could not drain engine output: {error}"))?;
                    if rest.is_empty() {
                        break;
                    }
                    let end = rest.iter().position(|byte| *byte == b'\n');
                    let consumed = end.map_or(rest.len(), |index| index + 1);
                    reader.consume(consumed);
                    if end.is_some() {
                        break;
                    }
                }
            }
            return Err(format!("Engine output line exceeded {cap} bytes"));
        }
        line.extend_from_slice(&available[..take]);
        let consumed = newline.map_or(take, |index| index + 1);
        reader.consume(consumed);
        if newline.is_some() {
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            return Ok(Some(line));
        }
    }
}

#[cfg(unix)]
struct ProcessTree {
    process_group: i32,
}

#[cfg(unix)]
impl ProcessTree {
    fn attach(
        child: &Child,
        _address_space_bytes: u64,
        _task_limit: usize,
    ) -> Result<Self, String> {
        let process_group = child
            .id()
            .ok_or_else(|| "The engine process has no process id".to_string())?
            .try_into()
            .map_err(|_| "The engine process id is out of range".to_string())?;
        Ok(Self { process_group })
    }

    fn terminate(&mut self) {
        if self.process_group > 0 {
            // SAFETY: negative PID targets only the process group created for
            // this engine; SIGKILL has no borrowed-memory preconditions.
            unsafe {
                libc::kill(-self.process_group, libc::SIGKILL);
            }
            self.process_group = 0;
        }
    }
}

#[cfg(windows)]
struct ProcessTree {
    job: windows_sys::Win32::Foundation::HANDLE,
}

// SAFETY: a Win32 Job Object HANDLE is a process-wide kernel handle with no
// thread affinity. ProcessTree owns it exclusively, and terminate() closes it
// and replaces it with null so ownership cannot be exercised twice.
#[cfg(windows)]
unsafe impl Send for ProcessTree {}

#[cfg(windows)]
impl ProcessTree {
    fn attach(child: &Child, address_space_bytes: u64, task_limit: usize) -> Result<Self, String> {
        use std::{mem::size_of, ptr};
        use windows_sys::Win32::{
            Foundation::{CloseHandle, HANDLE},
            System::JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
                SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOB_OBJECT_LIMIT_PROCESS_MEMORY,
            },
        };
        // SAFETY: all pointers are either null (optional names/security) or
        // point to initialized structures for the documented call duration.
        unsafe {
            let job = CreateJobObjectW(ptr::null(), ptr::null());
            if job.is_null() {
                return Err("Could not create a Windows engine Job Object".into());
            }
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                | JOB_OBJECT_LIMIT_PROCESS_MEMORY
                | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
            limits.BasicLimitInformation.ActiveProcessLimit =
                task_limit.min(u32::MAX as usize) as u32;
            limits.ProcessMemoryLimit = address_space_bytes.min(usize::MAX as u64) as usize;
            if SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) == 0
            {
                CloseHandle(job);
                return Err("Could not contain the engine in a Windows Job Object".into());
            }
            let Some(process) = child.raw_handle() else {
                CloseHandle(job);
                return Err(
                    "Could not contain an engine process without a Windows process handle".into(),
                );
            };
            if AssignProcessToJobObject(job, process as HANDLE) == 0 {
                CloseHandle(job);
                return Err("Could not contain the engine in a Windows Job Object".into());
            }
            Ok(Self { job })
        }
    }

    fn terminate(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;
        if !self.job.is_null() {
            // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE terminates the whole tree.
            unsafe { CloseHandle(self.job) };
            self.job = std::ptr::null_mut();
        }
    }
}

#[cfg(not(any(unix, windows)))]
struct ProcessTree;

#[cfg(not(any(unix, windows)))]
impl ProcessTree {
    fn attach(
        _child: &Child,
        _address_space_bytes: u64,
        _task_limit: usize,
    ) -> Result<Self, String> {
        Ok(Self)
    }
    fn terminate(&mut self) {}
}

pub fn validate_option_value(option: &UciOption, value: Option<&str>) -> Result<(), String> {
    if option.name.contains(['\r', '\n']) || value.is_some_and(|value| value.contains(['\r', '\n']))
    {
        return Err("UCI option names and values cannot contain newlines.".into());
    }
    match option.option_type.as_str() {
        "button" => {
            if value.is_some() {
                return Err(format!(
                    "{} is a button and does not store a value.",
                    option.name
                ));
            }
        }
        "check" => {
            if !matches!(value, Some("true" | "false")) {
                return Err(format!("{} must be true or false.", option.name));
            }
        }
        "spin" => {
            let parsed = value
                .and_then(|value| value.parse::<i64>().ok())
                .ok_or_else(|| format!("{} must be a whole number.", option.name))?;
            if option.min.is_some_and(|min| parsed < min)
                || option.max.is_some_and(|max| parsed > max)
            {
                return Err(format!(
                    "{} is outside the engine's accepted range.",
                    option.name
                ));
            }
        }
        "combo" => {
            let value = value.ok_or_else(|| format!("Choose a value for {}.", option.name))?;
            if !option.choices.iter().any(|choice| choice == value) {
                return Err(format!("{value} is not accepted by {}.", option.name));
            }
        }
        _ => {
            // "string" options (and unknown types) accept any value, including
            // the empty string, which is the UCI convention for clearing them.
        }
    }
    Ok(())
}

pub fn parse_uci_option(line: &str) -> Option<UciOption> {
    let definition = line.strip_prefix("option name ")?;
    let (name, definition) = definition.split_once(" type ")?;
    let tokens: Vec<_> = definition.split_whitespace().collect();
    let option_type = tokens.first()?.to_string();
    let mut default_value = None;
    let mut min = None;
    let mut max = None;
    let mut choices = Vec::new();
    let mut index = 1;
    while index < tokens.len() {
        let key = tokens[index];
        index += 1;
        let start = index;
        while index < tokens.len() && !matches!(tokens[index], "default" | "min" | "max" | "var") {
            index += 1;
        }
        let value = tokens[start..index].join(" ");
        match key {
            "default" => default_value = Some(value),
            "min" => min = value.parse().ok(),
            "max" => max = value.parse().ok(),
            "var" => choices.push(value),
            _ => {}
        }
    }
    Some(UciOption {
        name: name.trim().to_string(),
        option_type,
        value: default_value.clone(),
        default_value,
        min,
        max,
        choices,
    })
}

fn search_watchdog(position: &LivePosition, white_time: i64, black_time: i64) -> Duration {
    let remaining = if position.is_white_to_move() {
        white_time
    } else {
        black_time
    }
    .max(0) as u64;
    let reserve = (remaining / 10).clamp(500, 5_000);
    let before_flag = remaining.saturating_sub(reserve).max(250);
    let budget = (remaining / 3).clamp(750, 30_000).min(before_flag);
    Duration::from_millis(budget)
}

pub fn parse_uci_info(line: &str) -> Option<EngineTelemetry> {
    let tokens: Vec<_> = line.split_whitespace().collect();
    if tokens.first().copied() != Some("info") {
        return None;
    }
    let mut telemetry = EngineTelemetry {
        raw: line.to_string(),
        ..EngineTelemetry::default()
    };
    let mut index = 1;
    while index < tokens.len() {
        match tokens[index] {
            "depth" => telemetry.depth = parse_next(&tokens, index),
            "seldepth" => telemetry.selective_depth = parse_next(&tokens, index),
            "nodes" => telemetry.nodes = parse_next(&tokens, index),
            "nps" => telemetry.nodes_per_second = parse_next(&tokens, index),
            "time" => telemetry.time_ms = parse_next(&tokens, index),
            "hashfull" => telemetry.hash_full = parse_next(&tokens, index),
            "tbhits" => telemetry.tablebase_hits = parse_next(&tokens, index),
            "multipv" => telemetry.multi_pv = parse_next(&tokens, index),
            "score" if index + 2 < tokens.len() => {
                let value = tokens[index + 2].parse::<i32>().ok();
                match tokens[index + 1] {
                    "cp" => telemetry.score_cp = value,
                    "mate" => telemetry.mate_in = value,
                    _ => {}
                }
                if let Some(bound) = tokens.get(index + 3) {
                    if *bound == "lowerbound" || *bound == "upperbound" {
                        telemetry.score_bound = Some((*bound).to_string());
                    }
                }
            }
            "pv" => {
                telemetry.principal_variation = tokens[index + 1..]
                    .iter()
                    .map(|value| (*value).to_string())
                    .collect();
                break;
            }
            _ => {}
        }
        index += 1;
    }
    Some(telemetry)
}

fn parse_next<T: std::str::FromStr>(tokens: &[&str], index: usize) -> Option<T> {
    tokens.get(index + 1)?.parse().ok()
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::nproc_ceiling;
    use super::{
        allowed_engine_environment, parse_uci_info, parse_uci_option,
        per_engine_address_space_bytes, per_engine_hash_bytes, per_engine_hash_mb,
        read_capped_line, sanitize_engine_output, search_watchdog, validate_option_value,
        EngineGovernor, EngineLimits, UciEngine, ENGINE_ENV_ALLOWLIST,
        TABLEBASE_ADDRESS_SPACE_BYTES,
    };
    use crate::position::LivePosition;
    use std::time::Duration;
    use tokio::io::BufReader;

    #[test]
    fn search_watchdog_preserves_recovery_time() {
        let white = LivePosition::parse("startpos", "e2e4 e7e5").unwrap();
        let black = LivePosition::parse("startpos", "e2e4").unwrap();
        assert_eq!(
            search_watchdog(&white, 110_000, 180_000),
            Duration::from_secs(30)
        );
        assert_eq!(
            search_watchdog(&black, 180_000, 9_000),
            Duration::from_secs(3)
        );
        assert_eq!(
            search_watchdog(&white, 1_000, 180_000),
            Duration::from_millis(500)
        );
    }

    #[test]
    fn search_watchdog_uses_fen_side_to_move() {
        // Black to move at ply 0: black's clock (9s) drives the budget, not white's.
        let fen = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR b KQkq - 0 1";
        let position = LivePosition::parse(fen, "").unwrap();
        assert_eq!(
            search_watchdog(&position, 180_000, 9_000),
            Duration::from_secs(3)
        );
    }

    #[tokio::test]
    async fn rejects_oversized_engine_lines_without_unbounded_growth() {
        let input = format!("{}\nok\n", "x".repeat(33));
        let mut reader = BufReader::new(input.as_bytes());
        assert!(read_capped_line(&mut reader, 32).await.is_err());
        assert_eq!(
            String::from_utf8(read_capped_line(&mut reader, 32).await.unwrap().unwrap()).unwrap(),
            "ok"
        );
    }

    #[test]
    fn engine_output_controls_are_sanitized_before_reaching_logs_or_telemetry() {
        assert_eq!(
            sanitize_engine_output("info\tdepth 1\u{1b}[31m\r"),
            "info\tdepth 1�[31m�"
        );
    }

    #[tokio::test]
    async fn per_engine_and_aggregate_output_rate_ceilings_fail_closed_independently() {
        let error = match EngineGovernor::new(EngineLimits {
            simultaneous_engines: 1,
            total_memory_mb: 256,
            total_cpu_threads: 1,
            total_tasks: 8,
            output_bytes_per_engine_per_second: 16,
            total_output_bytes_per_second: 16,
            output_bytes_per_engine: 24,
        }) {
            Ok(_) => panic!("unsafe output rate unexpectedly accepted"),
            Err(error) => error,
        };
        assert!(error.contains("safely bounded"));

        let governor = EngineGovernor::new(EngineLimits {
            simultaneous_engines: 1,
            total_memory_mb: 256,
            total_cpu_threads: 1,
            total_tasks: 8,
            output_bytes_per_engine_per_second: 1024,
            total_output_bytes_per_second: 2048,
            output_bytes_per_engine: 4096,
        })
        .unwrap();
        let lease = governor.acquire(&[]).await.unwrap();
        assert!(lease.meter.charge(800).is_ok());
        assert!(lease.meter.charge(300).is_err());

        let aggregate_governor = EngineGovernor::new(EngineLimits {
            simultaneous_engines: 2,
            total_memory_mb: 512,
            total_cpu_threads: 2,
            total_tasks: 16,
            output_bytes_per_engine_per_second: 2048,
            total_output_bytes_per_second: 2500,
            output_bytes_per_engine: 4096,
        })
        .unwrap();
        let first = aggregate_governor.acquire(&[]).await.unwrap();
        let second = aggregate_governor.acquire(&[]).await.unwrap();
        assert!(first.meter.charge(1500).is_ok());
        assert!(second.meter.charge(1100).is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn output_rate_overflow_kills_the_engine_process_tree_immediately() {
        use std::os::unix::fs::PermissionsExt;

        let executable = std::env::temp_dir().join(format!(
            "queenui-output-overflow-{}.sh",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(
            &executable,
            r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    uci) echo "id name Overflow"; echo "uciok" ;;
    isready) echo "readyok" ;;
    go*) while :; do echo "info string xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"; done ;;
    quit) exit 0 ;;
  esac
done
"#,
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let governor = EngineGovernor::new(EngineLimits {
            simultaneous_engines: 1,
            total_memory_mb: 512,
            total_cpu_threads: 1,
            total_tasks: 64,
            output_bytes_per_engine_per_second: 1024,
            total_output_bytes_per_second: 2048,
            output_bytes_per_engine: 4096,
        })
        .unwrap();
        let mut engine =
            UciEngine::start_governed(executable.to_str().unwrap(), &[], None, &governor)
                .await
                .unwrap();
        assert!(engine
            .best_move("startpos", "", 10_000, 10_000, 0, 0, |_| {})
            .await
            .is_err());
        tokio::time::timeout(Duration::from_secs(1), engine.child.wait())
            .await
            .expect("overflow kill deadline")
            .unwrap();
        let _ = std::fs::remove_file(executable);
    }

    #[tokio::test]
    async fn engine_threads_and_hash_cannot_exceed_server_owned_aggregate_budgets() {
        let governor = EngineGovernor::new(EngineLimits {
            simultaneous_engines: 2,
            total_memory_mb: 512,
            total_cpu_threads: 4,
            total_tasks: 16,
            output_bytes_per_engine_per_second: 1024,
            total_output_bytes_per_second: 2048,
            output_bytes_per_engine: 1024,
        })
        .unwrap();
        let threads =
            parse_uci_option("option name Threads type spin default 8 min 1 max 128").unwrap();
        let hash =
            parse_uci_option("option name Hash type spin default 512 min 1 max 65536").unwrap();
        assert!(governor.acquire(&[threads]).await.is_err());
        assert!(governor.acquire(&[hash]).await.is_err());
        let allowed =
            parse_uci_option("option name Hash type spin default 256 min 1 max 65536").unwrap();
        assert!(governor.acquire(&[allowed]).await.is_ok());
    }

    #[test]
    fn address_space_ceiling_includes_tablebase_headroom_beyond_the_hash_budget() {
        let limits = EngineLimits {
            simultaneous_engines: 8,
            total_memory_mb: 16 * 1024,
            total_cpu_threads: 16,
            total_tasks: 256,
            output_bytes_per_engine_per_second: 1024,
            total_output_bytes_per_second: 2048,
            output_bytes_per_engine: 4096,
        };
        assert_eq!(per_engine_hash_mb(&limits), 2048);
        assert_eq!(per_engine_hash_bytes(&limits), 2048 * 1024 * 1024);
        assert_eq!(
            per_engine_address_space_bytes(&limits),
            2048 * 1024 * 1024 + TABLEBASE_ADDRESS_SPACE_BYTES
        );
        assert!(per_engine_address_space_bytes(&limits) > per_engine_hash_bytes(&limits));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn tablebase_sized_file_maps_succeed_under_a_tight_hash_budget() {
        use std::os::unix::fs::PermissionsExt;

        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/fake-uci-mmap.rs");
        let fixture =
            std::env::temp_dir().join(format!("queenui-fake-uci-mmap-{}", uuid::Uuid::new_v4()));
        let compilation = std::process::Command::new("rustc")
            .args(["--edition=2021", "-o"])
            .arg(&fixture)
            .arg(&source)
            .status()
            .expect("compile mmap probe engine");
        assert!(compilation.success(), "rustc {source:?} -> {fixture:?}");
        std::fs::set_permissions(&fixture, std::fs::Permissions::from_mode(0o755)).unwrap();

        // 256 MiB / 8 engines = 32 MiB Hash. The fixture maps 80 MiB.
        let governor = EngineGovernor::new(EngineLimits {
            simultaneous_engines: 8,
            total_memory_mb: 256,
            total_cpu_threads: 8,
            total_tasks: 64,
            output_bytes_per_engine_per_second: 1024 * 1024,
            total_output_bytes_per_second: 4 * 1024 * 1024,
            output_bytes_per_engine: 64 * 1024 * 1024,
        })
        .unwrap();
        let mut engine = UciEngine::start_governed(
            fixture.to_str().expect("UTF-8 fixture path"),
            &[],
            None,
            &governor,
        )
        .await
        .expect("start mmap probe engine");
        let search = engine
            .best_move("startpos", "", 10_000, 10_000, 0, 0, |_| {})
            .await
            .expect("search after mapping an 80 MiB file under a 32 MiB Hash budget");
        assert_eq!(search.best_move, "e2e4");
        engine.shutdown().await;
        let _ = std::fs::remove_file(fixture);
    }

    #[test]
    fn engine_environment_allowlist_excludes_credentials_and_home_paths() {
        assert!(!ENGINE_ENV_ALLOWLIST.iter().any(|name| {
            name.contains("TOKEN")
                || name.contains("SECRET")
                || *name == "HOME"
                || *name == "USERPROFILE"
        }));
        assert!(allowed_engine_environment()
            .iter()
            .all(|(name, _)| ENGINE_ENV_ALLOWLIST.contains(name)));
    }

    #[cfg(unix)]
    #[test]
    fn nproc_ceiling_adds_the_user_task_baseline_to_the_engine_allowance() {
        assert_eq!(nproc_ceiling(91, 32), 123);
        assert_eq!(
            nproc_ceiling(usize::MAX, 32),
            usize::MAX.try_into().unwrap_or(libc::RLIM_INFINITY)
        );
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn governed_spawn_allows_a_threaded_engine_above_the_user_task_baseline() {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Barrier,
        };

        struct SleepingThreads {
            stop: Arc<AtomicBool>,
            threads: Vec<std::thread::JoinHandle<()>>,
        }

        impl SleepingThreads {
            fn spawn(count: usize) -> Self {
                let stop = Arc::new(AtomicBool::new(false));
                let ready = Arc::new(Barrier::new(count + 1));
                let threads = (0..count)
                    .map(|_| {
                        let stop = stop.clone();
                        let ready = ready.clone();
                        std::thread::spawn(move || {
                            ready.wait();
                            while !stop.load(Ordering::Acquire) {
                                std::thread::park();
                            }
                        })
                    })
                    .collect();
                ready.wait();
                Self { stop, threads }
            }
        }

        impl Drop for SleepingThreads {
            fn drop(&mut self) {
                self.stop.store(true, Ordering::Release);
                for thread in self.threads.drain(..) {
                    thread.thread().unpark();
                    thread.join().expect("join sleeping test thread");
                }
            }
        }

        let _sleeping_threads = SleepingThreads::spawn(40);

        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/fake-uci-env.rs");
        let fixture = std::env::temp_dir().join(format!(
            "queenui-threaded-fake-uci-{}",
            uuid::Uuid::new_v4()
        ));
        let compilation = std::process::Command::new("rustc")
            .args(["--edition=2021", "-o"])
            .arg(&fixture)
            .arg(source)
            .status()
            .expect("compile threaded fake UCI engine");
        assert!(compilation.success());

        let governor = EngineGovernor::new(EngineLimits {
            simultaneous_engines: 8,
            ..EngineLimits::default()
        })
        .expect("construct the documented default engine limits");
        assert_eq!(
            governor.limits.total_tasks / governor.limits.simultaneous_engines,
            32
        );
        let mut engine = UciEngine::start_governed(
            fixture.to_str().expect("UTF-8 fixture path"),
            &[],
            None,
            &governor,
        )
        .await
        .expect("start a governed engine that creates a real thread");
        engine.shutdown().await;
        let _ = std::fs::remove_file(fixture);
    }

    #[tokio::test]
    async fn spawned_engine_receives_only_the_full_environment_allowlist() {
        struct RestoreEnvironment(Vec<(&'static str, Option<std::ffi::OsString>)>);
        impl Drop for RestoreEnvironment {
            fn drop(&mut self) {
                for (name, value) in self.0.drain(..) {
                    if let Some(value) = value {
                        std::env::set_var(name, value);
                    } else {
                        std::env::remove_var(name);
                    }
                }
            }
        }

        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/fake-uci-env.rs");
        let mut fixture =
            std::env::temp_dir().join(format!("queenui-fake-uci-env-{}", uuid::Uuid::new_v4()));
        if cfg!(windows) {
            fixture.set_extension("exe");
        }
        let compilation = std::process::Command::new("rustc")
            .args(["--edition=2021", "-o"])
            .arg(&fixture)
            .arg(source)
            .status()
            .expect("compile fake UCI environment reporter");
        assert!(compilation.success());
        let planted = [
            ("QUEEN_RUNNER_TOKEN", "runner-secret"),
            ("QUEEN_RUNNER_TOKEN_FILE", "/credentials/runner-token"),
            ("QUEENUI_RUNNER_TOKEN", "desktop-secret"),
            ("QUEENUI_RUNNER_TOKEN_FILE", "/credentials/desktop-token"),
            ("PLANTED_SECRET", "should-never-survive"),
            ("HOME", "/sensitive/home"),
            ("NOISY_PARENT_VALUE", "noise"),
        ];
        let restore = RestoreEnvironment(
            planted
                .iter()
                .map(|(name, _)| (*name, std::env::var_os(name)))
                .collect(),
        );
        for (name, value) in planted {
            std::env::set_var(name, value);
        }
        let mut engine = UciEngine::start(fixture.to_str().unwrap(), &[], None)
            .await
            .expect("start environment-reporting fake UCI engine");
        drop(restore);

        let reported = engine
            .name
            .as_deref()
            .expect("fake engine environment report")
            .strip_prefix("ENV:")
            .expect("environment report prefix");
        let entries: Vec<_> = reported
            // The fake engine uses an ASCII file-separator between values;
            // control-byte sanitization deliberately replaces it before any
            // engine output reaches a model or log.
            .split('\u{fffd}')
            .filter(|entry| !entry.is_empty())
            .collect();
        assert!(entries.iter().all(|entry| {
            entry
                .split_once('=')
                .is_some_and(|(name, _)| ENGINE_ENV_ALLOWLIST.contains(&name))
        }));
        for (name, _) in planted {
            assert!(
                entries
                    .iter()
                    .all(|entry| !entry.starts_with(&format!("{name}="))),
                "planted variable {name} leaked into the engine"
            );
        }
        let expected: std::collections::HashMap<_, _> = allowed_engine_environment()
            .into_iter()
            .map(|(name, value)| (name.to_string(), value.to_string_lossy().into_owned()))
            .collect();
        let actual: std::collections::HashMap<_, _> = entries
            .iter()
            .filter_map(|entry| entry.split_once('='))
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect();
        assert_eq!(actual, expected);
        engine.shutdown().await;
        let _ = std::fs::remove_file(fixture);
    }

    #[test]
    fn allows_empty_string_option_values() {
        let option = parse_uci_option("option name SyzygyPath type string default <empty>")
            .expect("parse string option");
        assert!(validate_option_value(&option, Some("")).is_ok());
        assert!(validate_option_value(&option, Some("/tables")).is_ok());
        assert!(validate_option_value(&option, None).is_ok());
    }

    #[test]
    fn parses_structured_search_telemetry() {
        let info = parse_uci_info(
            "info depth 24 seldepth 31 multipv 1 score cp 83 lowerbound nodes 3456789 nps 2100000 hashfull 422 tbhits 7 time 1646 pv e2e4 e7e5 g1f3",
        )
        .expect("parse info");
        assert_eq!(info.depth, Some(24));
        assert_eq!(info.selective_depth, Some(31));
        assert_eq!(info.score_cp, Some(83));
        assert_eq!(info.score_bound.as_deref(), Some("lowerbound"));
        assert_eq!(info.nodes, Some(3_456_789));
        assert_eq!(info.nodes_per_second, Some(2_100_000));
        assert_eq!(info.hash_full, Some(422));
        assert_eq!(info.tablebase_hits, Some(7));
        assert_eq!(info.time_ms, Some(1646));
        assert_eq!(info.principal_variation, ["e2e4", "e7e5", "g1f3"]);
        let json = serde_json::to_value(&info).expect("serialize telemetry");
        assert!(
            json.get("mateIn").is_none(),
            "an absent mate score must not serialize as null"
        );
    }

    #[test]
    fn parses_mate_scores() {
        let info = parse_uci_info("info depth 18 score mate -3 pv h7h8q").expect("parse mate info");
        assert_eq!(info.mate_in, Some(-3));
        assert_eq!(info.score_cp, None);
    }

    #[test]
    fn parses_and_validates_uci_options() {
        let hash = parse_uci_option("option name Hash type spin default 16 min 1 max 4096")
            .expect("parse Hash");
        assert_eq!(hash.name, "Hash");
        assert_eq!(hash.value.as_deref(), Some("16"));
        assert_eq!(hash.min, Some(1));
        assert_eq!(hash.max, Some(4096));
        assert!(validate_option_value(&hash, Some("512")).is_ok());
        assert!(validate_option_value(&hash, Some("8192")).is_err());

        let style = parse_uci_option(
            "option name Style type combo default Normal var Normal var Aggressive var Defensive",
        )
        .expect("parse Style");
        assert_eq!(style.choices, ["Normal", "Aggressive", "Defensive"]);
        assert!(validate_option_value(&style, Some("Aggressive")).is_ok());
    }
}
