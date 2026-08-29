use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use queen_core::{AppState, CoreStateRef};
use queen_protocol::{
    EngineBrowseEntry, EngineBrowseEntryKind, EngineBrowseRequest, EngineBrowseResponse,
    EngineRoot, ENGINE_BROWSE_DEFAULT_PAGE_ENTRIES, ENGINE_BROWSE_MAX_PAGE_ENTRIES,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, VecDeque},
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;

const CONFIG_FILE: &str = "runner-config.json";
const MAX_RELATIVE_PATH_BYTES: usize = 4096;
const MAX_PATH_DEPTH: usize = 16;
const MAX_COMPONENT_BYTES: usize = 255;
const MAX_DIRECTORY_WORK: usize = 4096;
const MAX_SERIALIZED_PAGE_BYTES: usize = 256 * 1024;
const BROWSE_TIMEOUT: Duration = Duration::from_millis(750);
const CURSOR_TTL: Duration = Duration::from_secs(60);
const MAX_CURSORS: usize = 128;
const MAX_STORE_ENTRIES: usize = 4096;
const BROWSE_RATE_WINDOW: Duration = Duration::from_secs(60);
const MAX_BROWSES_PER_WINDOW: usize = 120;
const STORE_DIRECTORY: &str = "engine-store";

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
enum EngineAdminMode {
    #[default]
    AdminInstalled,
    SecureInstall,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RootConfig {
    id: String,
    #[serde(default)]
    label: Option<String>,
    path: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct AvailabilityLimits {
    pub normal_commands: usize,
    pub query_concurrency: usize,
    pub blocking_workers: usize,
    pub simultaneous_engines: usize,
    /// Aggregate Hash/RSS budget across simultaneous engines. Not the process
    /// address-space ceiling: file-backed Syzygy maps get a separate VAS headroom
    /// in queen-core so a 6-piece `.rtbw` cannot `ENOMEM` under this number.
    pub total_engine_memory_mb: u64,
    pub total_engine_cpu_threads: usize,
    pub total_engine_tasks: usize,
    pub engine_output_bytes_per_second: u64,
    pub total_engine_output_bytes_per_second: u64,
    pub engine_output_total_bytes: u64,
    pub engine_log_bytes: u64,
    pub engine_store_bytes: u64,
    pub minimum_free_disk_bytes: u64,
}

impl Default for AvailabilityLimits {
    fn default() -> Self {
        let cpus = std::thread::available_parallelism().map_or(1, usize::from);
        Self {
            normal_commands: 32,
            query_concurrency: 4,
            blocking_workers: 4,
            simultaneous_engines: cpus.clamp(1, 16),
            total_engine_memory_mb: 16 * 1024,
            total_engine_cpu_threads: cpus,
            total_engine_tasks: 256,
            engine_output_bytes_per_second: 1024 * 1024,
            total_engine_output_bytes_per_second: 4 * 1024 * 1024,
            engine_output_total_bytes: 64 * 1024 * 1024,
            engine_log_bytes: 2 * 1024 * 1024 * 1024,
            engine_store_bytes: 8 * 1024 * 1024 * 1024,
            minimum_free_disk_bytes: 256 * 1024 * 1024,
        }
    }
}

impl AvailabilityLimits {
    fn validate(&self) -> Result<(), String> {
        if !(1..=256).contains(&self.normal_commands)
            || !(1..=32).contains(&self.query_concurrency)
            || !(1..=32).contains(&self.blocking_workers)
            || !(1..=64).contains(&self.simultaneous_engines)
            || !(256..=1024 * 1024).contains(&self.total_engine_memory_mb)
            || !(1..=1024).contains(&self.total_engine_cpu_threads)
            || !(1..=4096).contains(&self.total_engine_tasks)
            || !(1024..=64 * 1024 * 1024).contains(&self.engine_output_bytes_per_second)
            || !(1024..=256 * 1024 * 1024).contains(&self.total_engine_output_bytes_per_second)
            || self.total_engine_output_bytes_per_second < self.engine_output_bytes_per_second
            || !(1024..=4 * 1024 * 1024 * 1024).contains(&self.engine_output_total_bytes)
            || !(1024 * 1024..=64 * 1024 * 1024 * 1024).contains(&self.engine_log_bytes)
            || !(1024 * 1024..=1024 * 1024 * 1024 * 1024).contains(&self.engine_store_bytes)
            || !(1024 * 1024..=64 * 1024 * 1024 * 1024).contains(&self.minimum_free_disk_bytes)
        {
            return Err(
                "runner-config.json contains an availability limit outside the safe server bounds"
                    .into(),
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct RunnerConfig {
    engine_admin: EngineAdminMode,
    engine_roots: Vec<RootConfig>,
    /// Exact administrator-provided opening-book paths. These remain a
    /// config-file-only compatibility form until the asset browser lands.
    opening_book_allowlist: Vec<PathBuf>,
    limits: AvailabilityLimits,
}

struct TrustedRoot {
    public: EngineRoot,
    authority: File,
    #[cfg(not(unix))]
    path: PathBuf,
    #[cfg(unix)]
    device: libc::dev_t,
}

impl TrustedRoot {
    fn try_clone(&self) -> Result<Self, String> {
        Ok(Self {
            public: self.public.clone(),
            authority: self
                .authority
                .try_clone()
                .map_err(|_| "The configured engine root authority is unavailable".to_string())?,
            #[cfg(not(unix))]
            path: self.path.clone(),
            #[cfg(unix)]
            device: self.device,
        })
    }
}

#[derive(Clone)]
struct CursorState {
    root_id: String,
    relative_path: String,
    offset: usize,
    expires: Instant,
}

pub(crate) struct EngineAdmin {
    roots: HashMap<String, TrustedRoot>,
    root_order: Vec<String>,
    store: PathBuf,
    cursors: Mutex<HashMap<String, CursorState>>,
    browse_rate: Mutex<VecDeque<Instant>>,
    browse_admission: Arc<Semaphore>,
    store_admission: Arc<Semaphore>,
    opening_book_allowlist: Vec<PathBuf>,
    pub(crate) limits: AvailabilityLimits,
}

impl EngineAdmin {
    pub(crate) fn load(data_dir: &Path) -> Result<Self, String> {
        let path = data_dir.join(CONFIG_FILE);
        let config = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<RunnerConfig>(&bytes).map_err(|_| {
                "runner-config.json is not a valid runner configuration".to_string()
            })?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => RunnerConfig::default(),
            Err(_) => return Err("runner-config.json could not be read".into()),
        };
        if config.engine_admin == EngineAdminMode::SecureInstall {
            return Err("engine_admin=secure-install is reserved until the containment preflight is implemented".into());
        }
        config.limits.validate()?;
        let mut roots = HashMap::new();
        let mut root_order = Vec::new();
        for configured in config.engine_roots {
            validate_root_id(&configured.id)?;
            if roots.contains_key(&configured.id) {
                return Err("runner-config.json contains a duplicate engine root id".into());
            }
            let root = open_root(configured)?;
            root_order.push(root.public.id.clone());
            roots.insert(root.public.id.clone(), root);
        }
        let store = data_dir.join(STORE_DIRECTORY);
        fs::create_dir_all(&store).map_err(|_| {
            "The private content-addressed engine store could not be created".to_string()
        })?;
        set_directory_mode(&store, 0o755)?;
        let opening_book_allowlist = config
            .opening_book_allowlist
            .into_iter()
            .map(|path| {
                if !path.is_absolute() {
                    return Err("opening_book_allowlist entries must be absolute".to_string());
                }
                path.canonicalize()
                    .map_err(|_| "An opening_book_allowlist entry is unavailable".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let blocking_workers = config.limits.blocking_workers;
        Ok(Self {
            roots,
            root_order,
            store,
            cursors: Mutex::new(HashMap::new()),
            browse_rate: Mutex::new(VecDeque::new()),
            browse_admission: Arc::new(Semaphore::new(blocking_workers)),
            store_admission: Arc::new(Semaphore::new(1)),
            opening_book_allowlist,
            limits: config.limits,
        })
    }

    pub(crate) fn roots(&self) -> Vec<EngineRoot> {
        self.root_order
            .iter()
            .filter_map(|id| self.roots.get(id).map(|root| root.public.clone()))
            .collect()
    }

    pub(crate) fn blocking_admission(&self) -> Arc<Semaphore> {
        self.browse_admission.clone()
    }

    pub(crate) async fn browse(
        &self,
        request: EngineBrowseRequest,
    ) -> Result<EngineBrowseResponse, String> {
        self.admit_browse()?;
        let page_entries = request
            .page_entries
            .unwrap_or(ENGINE_BROWSE_DEFAULT_PAGE_ENTRIES);
        if page_entries == 0 || page_entries > ENGINE_BROWSE_MAX_PAGE_ENTRIES {
            return Err("The requested engine-browser page size is invalid".into());
        }
        let (root_id, relative_path, offset) = self.resolve_cursor(&request)?;
        let components = parse_relative_path(&relative_path, true)?;
        let root = self
            .roots
            .get(&root_id)
            .ok_or_else(|| "The configured engine root does not exist".to_string())?
            .try_clone()?;
        let permit = self
            .browse_admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| "The engine browser is at its concurrency limit".to_string())?;
        let task = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            browse_directory(&root, &components, offset, usize::from(page_entries))
        });
        let page = tokio::time::timeout(BROWSE_TIMEOUT, task)
            .await
            .map_err(|_| "The bounded engine-browser deadline was exceeded".to_string())?
            .map_err(|_| "The engine-browser worker stopped unexpectedly".to_string())??;
        let next_cursor = page
            .next_offset
            .map(|next| self.mint_cursor(&root_id, &relative_path, next))
            .transpose()?;
        Ok(EngineBrowseResponse {
            root_id,
            relative_path,
            entries: page.entries,
            next_cursor,
        })
    }

    pub(crate) async fn register(
        &self,
        root_id: String,
        relative_path: String,
        core: &AppState,
    ) -> Result<queen_core::models::EngineProfile, String> {
        let components = parse_relative_path(&relative_path, false)?;
        let root = self
            .roots
            .get(&root_id)
            .ok_or_else(|| "The configured engine root does not exist".to_string())?
            .try_clone()?;
        let store = self.store.clone();
        let store_bytes = self.limits.engine_store_bytes;
        let minimum_free_disk_bytes = self.limits.minimum_free_disk_bytes;
        let permit = self
            .store_admission
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| "The engine store is shutting down".to_string())?;
        let stored = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            copy_into_store(
                &root,
                &components,
                &store,
                store_bytes,
                minimum_free_disk_bytes,
            )
        })
        .await
        .map_err(|_| "The engine-store worker stopped unexpectedly".to_string())??;
        match queen_core::add_engine(
            stored.to_string_lossy().to_string(),
            CoreStateRef::new(core),
        )
        .await
        {
            Ok(profile) => Ok(profile),
            Err(error) => {
                self.garbage_collect(core).await;
                Err(error)
            }
        }
    }

    pub(crate) async fn garbage_collect(&self, core: &AppState) {
        let referenced: std::collections::HashSet<_> = core
            .snapshot()
            .await
            .engines
            .into_iter()
            .filter_map(|engine| {
                let path = PathBuf::from(engine.path);
                (path.parent() == Some(self.store.as_path()))
                    .then(|| path.file_name()?.to_str().map(str::to_string))
                    .flatten()
            })
            .collect();
        let Ok(entries) = fs::read_dir(&self.store) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if is_store_temporary(name) || (is_store_digest(name) && !referenced.contains(name)) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }

    pub(crate) async fn validate_registered_engines(&self, core: &AppState) -> Result<(), String> {
        let engines = core.snapshot().await.engines;
        for engine in engines {
            let path = PathBuf::from(&engine.path);
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                return Err("A registered runner engine is not a managed content identity".into());
            };
            if path.parent() != Some(self.store.as_path())
                || !is_store_digest(name)
                || name.bytes().any(|byte| byte.is_ascii_uppercase())
            {
                return Err("A registered runner engine predates trusted-engine storage; remove it from config and re-register through a configured root".into());
            }
            let path_metadata = fs::symlink_metadata(&path)
                .map_err(|_| "A registered content-addressed engine is unavailable".to_string())?;
            if path_metadata.file_type().is_symlink() || is_platform_reparse_point(&path_metadata) {
                return Err(
                    "A registered content-addressed engine cannot be a link or reparse point"
                        .into(),
                );
            }
            let mut file = File::open(&path)
                .map_err(|_| "A registered content-addressed engine is unavailable".to_string())?;
            let metadata = file
                .metadata()
                .map_err(|_| "A registered content-addressed engine is unavailable".to_string())?;
            if !metadata.is_file() {
                return Err("A registered content-addressed engine is not a regular file".into());
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if metadata.permissions().mode() & 0o222 != 0 {
                    return Err("A registered content-addressed engine is writable".into());
                }
            }
            let mut digest = Sha256::new();
            let mut buffer = [0u8; 64 * 1024];
            loop {
                let read = file.read(&mut buffer).map_err(|_| {
                    "A registered content-addressed engine is unreadable".to_string()
                })?;
                if read == 0 {
                    break;
                }
                digest.update(&buffer[..read]);
            }
            if encode_hex(&digest.finalize()) != name {
                return Err(
                    "A registered content-addressed engine no longer matches its identity".into(),
                );
            }
        }
        Ok(())
    }

    pub(crate) fn validate_opening_book(&self, requested: &str) -> Result<String, String> {
        let path = PathBuf::from(requested);
        if !path.is_absolute() {
            return Err("Remote opening books require an administrator-allowlisted asset".into());
        }
        let canonical = path
            .canonicalize()
            .map_err(|_| "The selected administrator asset is unavailable".to_string())?;
        if !self
            .opening_book_allowlist
            .iter()
            .any(|allowed| allowed == &canonical)
        {
            return Err("Remote opening books require an administrator-allowlisted asset".into());
        }
        Ok(canonical.to_string_lossy().to_string())
    }

    fn admit_browse(&self) -> Result<(), String> {
        let now = Instant::now();
        let mut rate = self
            .browse_rate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while rate
            .front()
            .is_some_and(|at| now.duration_since(*at) >= BROWSE_RATE_WINDOW)
        {
            rate.pop_front();
        }
        if rate.len() >= MAX_BROWSES_PER_WINDOW {
            return Err("The engine-browser rate limit was reached".into());
        }
        rate.push_back(now);
        Ok(())
    }

    fn resolve_cursor(
        &self,
        request: &EngineBrowseRequest,
    ) -> Result<(String, String, usize), String> {
        let Some(cursor) = request.cursor.as_deref() else {
            return Ok((request.root_id.clone(), request.relative_path.clone(), 0));
        };
        let now = Instant::now();
        let mut cursors = self
            .cursors
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cursors.retain(|_, state| state.expires > now);
        let state = cursors
            .remove(cursor)
            .filter(|state| {
                state.root_id == request.root_id
                    && state.relative_path == request.relative_path
                    && state.expires > now
            })
            .ok_or_else(|| "The engine-browser cursor is invalid or expired".to_string())?;
        Ok((state.root_id, state.relative_path, state.offset))
    }

    fn mint_cursor(
        &self,
        root_id: &str,
        relative_path: &str,
        offset: usize,
    ) -> Result<String, String> {
        let mut random = [0u8; 24];
        getrandom::fill(&mut random)
            .map_err(|_| "The engine-browser cursor could not be created".to_string())?;
        let token = URL_SAFE_NO_PAD.encode(random);
        let now = Instant::now();
        let mut cursors = self
            .cursors
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cursors.retain(|_, state| state.expires > now);
        if cursors.len() >= MAX_CURSORS {
            return Err("The engine-browser cursor quota is full".into());
        }
        cursors.insert(
            token.clone(),
            CursorState {
                root_id: root_id.to_string(),
                relative_path: relative_path.to_string(),
                offset,
                expires: now + CURSOR_TTL,
            },
        );
        Ok(token)
    }
}

fn validate_root_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err("Engine root ids must use 1-64 ASCII letters, digits, '-' or '_'".into());
    }
    Ok(())
}

fn parse_relative_path(value: &str, allow_empty: bool) -> Result<Vec<String>, String> {
    if value.is_empty() {
        return if allow_empty {
            Ok(Vec::new())
        } else {
            Err("Select an engine file below the configured root".into())
        };
    }
    if value.len() > MAX_RELATIVE_PATH_BYTES
        || value.starts_with('/')
        || value.contains('\\')
        || value.contains('\0')
    {
        return Err("The engine-browser relative path is invalid".into());
    }
    let components: Vec<_> = value.split('/').map(str::to_string).collect();
    if components.len() > MAX_PATH_DEPTH
        || components.iter().any(|component| {
            component.is_empty()
                || matches!(component.as_str(), "." | "..")
                || component.len() > MAX_COMPONENT_BYTES
        })
    {
        return Err("The engine-browser relative path is invalid".into());
    }
    Ok(components)
}

struct BrowsePage {
    entries: Vec<EngineBrowseEntry>,
    next_offset: Option<usize>,
}

fn copy_into_store(
    root: &TrustedRoot,
    components: &[String],
    store: &Path,
    maximum_store_bytes: u64,
    minimum_free_disk_bytes: u64,
) -> Result<PathBuf, String> {
    let mut source = open_relative(root, components, ExpectedKind::File)?;
    #[cfg(unix)]
    let source_size = {
        let metadata = trusted_metadata(&source, Some(root.device))?;
        if metadata.mode & 0o111 == 0 {
            return Err("The selected administrator file is not marked executable".into());
        }
        metadata.size
    };
    #[cfg(not(unix))]
    let source_size = source
        .metadata()
        .map_err(|_| "The selected engine could not be inspected".to_string())?
        .len();
    let current_store_bytes = store_usage(store)?;
    if source_size > maximum_store_bytes.saturating_sub(current_store_bytes) {
        return Err("The engine-store temporary and installed byte quota would be exceeded".into());
    }
    if available_disk_bytes(store)? < source_size.saturating_add(minimum_free_disk_bytes) {
        return Err("The engine store does not have the configured free-space reserve".into());
    }
    source
        .seek(SeekFrom::Start(0))
        .map_err(|_| "The selected engine could not be read".to_string())?;
    let temporary = store.join(format!(".{}.tmp", random_hex()?));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o500);
    }
    let mut output = options.open(&temporary).map_err(|_| {
        "The content-addressed engine temporary file could not be created".to_string()
    })?;
    let result = (|| {
        let mut digest = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = source
                .read(&mut buffer)
                .map_err(|_| "The selected engine could not be read".to_string())?;
            if read == 0 {
                break;
            }
            digest.update(&buffer[..read]);
            output.write_all(&buffer[..read]).map_err(|_| {
                "The content-addressed engine copy could not be written".to_string()
            })?;
        }
        set_file_mode(&output, 0o555)?;
        output.sync_all().map_err(|_| {
            "The content-addressed engine copy could not be synchronized".to_string()
        })?;
        let identity = encode_hex(&digest.finalize());
        let destination = store.join(&identity);
        match fs::symlink_metadata(&destination) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::rename(&temporary, &destination).map_err(|_| {
                    "The content-addressed engine copy could not be installed".to_string()
                })?;
            }
            Ok(_) => {}
            Err(_) => {
                return Err("The content-addressed engine store could not be inspected".into())
            }
        }
        verify_store_file(&destination, &identity)?;
        File::open(store)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| {
                "The content-addressed engine directory could not be synchronized".to_string()
            })?;
        Ok(destination)
    })();
    drop(output);
    let _ = fs::remove_file(&temporary);
    result
}

fn store_usage(store: &Path) -> Result<u64, String> {
    let mut bytes = 0u64;
    let mut entries = 0usize;
    for entry in
        fs::read_dir(store).map_err(|_| "The engine store could not be enumerated".to_string())?
    {
        let entry = entry.map_err(|_| "The engine store could not be enumerated".to_string())?;
        entries = entries.saturating_add(1);
        if entries > MAX_STORE_ENTRIES {
            return Err("The engine store exceeds its bounded entry quota".into());
        }
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|_| "An engine-store entry could not be inspected".to_string())?;
        if metadata.is_file() {
            bytes = bytes.saturating_add(metadata.len());
        }
    }
    Ok(bytes)
}

#[cfg(unix)]
fn available_disk_bytes(path: &Path) -> Result<u64, String> {
    use std::{mem::MaybeUninit, os::unix::ffi::OsStrExt};
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| "The engine-store path is invalid".to_string())?;
    let mut stats = MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return Err("The engine-store free space could not be inspected".into());
    }
    let stats = unsafe { stats.assume_init() };
    Ok(stats.f_bavail.saturating_mul(stats.f_frsize))
}

#[cfg(windows)]
fn available_disk_bytes(path: &Path) -> Result<u64, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut available = 0u64;
    if unsafe {
        GetDiskFreeSpaceExW(
            path.as_ptr(),
            &mut available,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err("The engine-store free space could not be inspected".into());
    }
    Ok(available)
}

#[cfg(not(any(unix, windows)))]
fn available_disk_bytes(_path: &Path) -> Result<u64, String> {
    Err("Engine-store free-space inspection is unsupported on this platform".into())
}

fn random_hex() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|_| "A secure temporary engine name could not be created".to_string())?;
    Ok(encode_hex(&bytes))
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn is_store_digest(name: &str) -> bool {
    name.len() == 64 && name.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_store_temporary(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() == 37
        && bytes[0] == b'.'
        && bytes[33..] == *b".tmp"
        && bytes[1..33].iter().all(|byte| byte.is_ascii_hexdigit())
}

fn verify_store_file(path: &Path, identity: &str) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| "The content-addressed engine identity is unavailable".to_string())?;
    if metadata.file_type().is_symlink()
        || is_platform_reparse_point(&metadata)
        || !metadata.is_file()
    {
        return Err("The content-addressed engine identity is not a regular file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o222 != 0 {
            return Err("The content-addressed engine identity is writable".into());
        }
    }
    let mut file = File::open(path)
        .map_err(|_| "The content-addressed engine identity is unreadable".to_string())?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| "The content-addressed engine identity is unreadable".to_string())?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    if encode_hex(&digest.finalize()) != identity {
        return Err("The content-addressed engine identity does not match its bytes".into());
    }
    Ok(())
}

#[cfg(windows)]
fn is_platform_reparse_point(metadata: &fs::Metadata) -> bool {
    is_reparse_point(metadata)
}

#[cfg(not(windows))]
fn is_platform_reparse_point(_metadata: &fs::Metadata) -> bool {
    false
}

#[cfg(unix)]
fn open_root(config: RootConfig) -> Result<TrustedRoot, String> {
    use std::os::{fd::FromRawFd, unix::ffi::OsStrExt};
    if !config.path.is_absolute() {
        return Err("Configured engine roots must be absolute".into());
    }
    let path = std::ffi::CString::new(config.path.as_os_str().as_bytes())
        .map_err(|_| "A configured engine root path is invalid".to_string())?;
    // SAFETY: `path` is NUL-terminated and the returned descriptor is owned by
    // the File constructed immediately below.
    let fd = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err("A configured engine root could not be opened without following links".into());
    }
    // SAFETY: `fd` is a fresh successful open result.
    let authority = unsafe { File::from_raw_fd(fd) };
    let metadata = trusted_metadata(&authority, None)?;
    Ok(TrustedRoot {
        public: EngineRoot {
            label: config.label.unwrap_or_else(|| config.id.clone()),
            id: config.id,
        },
        authority,
        device: metadata.device,
    })
}

#[cfg(not(unix))]
fn open_root(config: RootConfig) -> Result<TrustedRoot, String> {
    if !config.path.is_absolute() {
        return Err("Configured engine roots must be absolute".into());
    }
    let metadata = fs::symlink_metadata(&config.path)
        .map_err(|_| "A configured engine root could not be opened".to_string())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
        return Err("A configured engine root cannot be a link or reparse point".into());
    }
    let authority = File::open(&config.path)
        .map_err(|_| "A configured engine root could not be held open".to_string())?;
    Ok(TrustedRoot {
        public: EngineRoot {
            label: config.label.unwrap_or_else(|| config.id.clone()),
            id: config.id,
        },
        authority,
        path: config.path,
    })
}

#[derive(Clone, Copy)]
enum ExpectedKind {
    Directory,
    File,
}

#[cfg(unix)]
struct TrustedMetadata {
    device: libc::dev_t,
    mode: libc::mode_t,
    size: u64,
    modified_at_ms: Option<u64>,
}

#[cfg(unix)]
fn trusted_metadata(
    file: &File,
    root_device: Option<libc::dev_t>,
) -> Result<TrustedMetadata, String> {
    use std::os::fd::AsRawFd;
    // SAFETY: `stat` points to writable initialized storage and the descriptor
    // remains open for the duration of fstat.
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(file.as_raw_fd(), &mut stat) } != 0 {
        return Err("An engine-browser entry could not be inspected".into());
    }
    if root_device.is_some_and(|device| device != stat.st_dev) {
        return Err("The engine-browser does not cross mounted filesystems".into());
    }
    let effective_uid = unsafe { libc::geteuid() };
    if !matches!(stat.st_uid, 0) && stat.st_uid != effective_uid || stat.st_mode & 0o022 != 0 {
        return Err(
            "An engine-browser entry is not owned and protected by the runner administrator".into(),
        );
    }
    let modified_at_ms = u64::try_from(stat.st_mtime)
        .ok()
        .map(|seconds| seconds.saturating_mul(1000));
    Ok(TrustedMetadata {
        device: stat.st_dev,
        mode: stat.st_mode,
        size: u64::try_from(stat.st_size).unwrap_or(0),
        modified_at_ms,
    })
}

#[cfg(unix)]
fn open_relative(
    root: &TrustedRoot,
    components: &[String],
    expected: ExpectedKind,
) -> Result<File, String> {
    use std::os::fd::{AsRawFd, FromRawFd};
    // SAFETY: fcntl duplicates the held authority; ownership transfers to File.
    let duplicate = unsafe { libc::fcntl(root.authority.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    if duplicate < 0 {
        return Err("The configured engine root authority is unavailable".into());
    }
    let mut current = unsafe { File::from_raw_fd(duplicate) };
    trusted_metadata(&current, Some(root.device))?;
    for (index, component) in components.iter().enumerate() {
        let final_component = index + 1 == components.len();
        let kind = if final_component {
            expected
        } else {
            ExpectedKind::Directory
        };
        let child = open_component(&current, component, kind)?;
        trusted_metadata(&child, Some(root.device))?;
        current = child;
    }
    Ok(current)
}

#[cfg(unix)]
fn open_component(parent: &File, component: &str, expected: ExpectedKind) -> Result<File, String> {
    use std::os::fd::{AsRawFd, FromRawFd};
    let component = std::ffi::CString::new(component)
        .map_err(|_| "The engine-browser relative path is invalid".to_string())?;
    let mut flags = libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW;
    if matches!(expected, ExpectedKind::Directory) {
        flags |= libc::O_DIRECTORY;
    }
    #[cfg(target_os = "linux")]
    let fd = openat2_component(parent.as_raw_fd(), &component, flags)?;
    #[cfg(not(target_os = "linux"))]
    let fd = unsafe { libc::openat(parent.as_raw_fd(), component.as_ptr(), flags) };
    if fd < 0 {
        return Err(
            "The engine-browser refused a link, mount crossing, or unavailable entry".into(),
        );
    }
    let file = unsafe { File::from_raw_fd(fd) };
    let metadata = file
        .metadata()
        .map_err(|_| "An engine-browser entry could not be inspected".to_string())?;
    let valid = match expected {
        ExpectedKind::Directory => metadata.is_dir(),
        ExpectedKind::File => metadata.is_file(),
    };
    if !valid {
        return Err("The engine-browser entry has the wrong type".into());
    }
    Ok(file)
}

#[cfg(target_os = "linux")]
fn openat2_component(
    parent: std::os::fd::RawFd,
    component: &std::ffi::CStr,
    flags: libc::c_int,
) -> Result<libc::c_int, String> {
    #[repr(C)]
    struct OpenHow {
        flags: u64,
        mode: u64,
        resolve: u64,
    }
    const RESOLVE_NO_XDEV: u64 = 0x01;
    const RESOLVE_NO_MAGICLINKS: u64 = 0x02;
    const RESOLVE_NO_SYMLINKS: u64 = 0x04;
    const RESOLVE_BENEATH: u64 = 0x08;
    let how = OpenHow {
        flags: flags as u64,
        mode: 0,
        resolve: RESOLVE_NO_XDEV | RESOLVE_NO_MAGICLINKS | RESOLVE_NO_SYMLINKS | RESOLVE_BENEATH,
    };
    let fd = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            parent,
            component.as_ptr(),
            &how,
            std::mem::size_of::<OpenHow>(),
        ) as libc::c_int
    };
    if fd >= 0 {
        return Ok(fd);
    }
    let error = std::io::Error::last_os_error();
    if matches!(
        error.raw_os_error(),
        Some(libc::ENOSYS) | Some(libc::EINVAL)
    ) {
        let fallback = unsafe { libc::openat(parent, component.as_ptr(), flags) };
        return Ok(fallback);
    }
    Err("The engine-browser refused a link, mount crossing, or unavailable entry".into())
}

#[cfg(unix)]
fn browse_directory(
    root: &TrustedRoot,
    components: &[String],
    offset: usize,
    limit: usize,
) -> Result<BrowsePage, String> {
    use std::os::fd::AsRawFd;
    let directory = open_relative(root, components, ExpectedKind::Directory)?;
    let duplicate = unsafe { libc::fcntl(directory.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    if duplicate < 0 {
        return Err("The engine-browser directory authority is unavailable".into());
    }
    let stream = unsafe { libc::fdopendir(duplicate) };
    if stream.is_null() {
        unsafe { libc::close(duplicate) };
        return Err("The engine-browser directory could not be enumerated".into());
    }
    // The duplicate shares its directory offset with the held root authority.
    // Reset the stream so every browse enumerates from the beginning.
    unsafe { libc::rewinddir(stream) };
    let mut entries = Vec::new();
    let mut scanned = 0usize;
    let mut serialized = 0usize;
    let mut has_more = false;
    let result = (|| {
        loop {
            let entry = unsafe { libc::readdir(stream) };
            if entry.is_null() {
                break;
            }
            let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
            if matches!(name, b"." | b"..") {
                continue;
            }
            scanned = scanned.saturating_add(1);
            if scanned > MAX_DIRECTORY_WORK {
                return Err("The engine-browser directory exceeds the bounded work limit".into());
            }
            if scanned <= offset {
                continue;
            }
            if entries.len() >= limit {
                has_more = true;
                break;
            }
            let name = std::str::from_utf8(name)
                .map_err(|_| "The engine-browser found a non-UTF-8 entry name".to_string())?;
            if name.len() > MAX_COMPONENT_BYTES {
                return Err("The engine-browser found an overlong entry name".into());
            }
            let child = open_component_any(&directory, name)?;
            let metadata = trusted_metadata(&child.file, Some(root.device))?;
            let relative_path = if components.is_empty() {
                name.to_string()
            } else {
                format!("{}/{}", components.join("/"), name)
            };
            let browse_entry = EngineBrowseEntry {
                name: name.to_string(),
                relative_path,
                kind: child.kind,
                size: metadata.size,
                modified_at_ms: metadata.modified_at_ms,
                executable: metadata.mode & 0o111 != 0,
            };
            let encoded = serde_json::to_vec(&browse_entry)
                .map_err(|_| "The engine-browser entry could not be encoded".to_string())?;
            if serialized.saturating_add(encoded.len()) > MAX_SERIALIZED_PAGE_BYTES {
                has_more = true;
                break;
            }
            serialized += encoded.len();
            entries.push(browse_entry);
        }
        Ok(BrowsePage {
            next_offset: has_more.then_some(offset.saturating_add(entries.len())),
            entries,
        })
    })();
    unsafe { libc::closedir(stream) };
    result
}

#[cfg(unix)]
struct OpenedChild {
    file: File,
    kind: EngineBrowseEntryKind,
}

#[cfg(unix)]
fn open_component_any(parent: &File, component: &str) -> Result<OpenedChild, String> {
    match open_component(parent, component, ExpectedKind::Directory) {
        Ok(file) => Ok(OpenedChild {
            file,
            kind: EngineBrowseEntryKind::Directory,
        }),
        Err(_) => open_component(parent, component, ExpectedKind::File).map(|file| OpenedChild {
            file,
            kind: EngineBrowseEntryKind::File,
        }),
    }
}

#[cfg(not(unix))]
fn open_relative(
    root: &TrustedRoot,
    components: &[String],
    expected: ExpectedKind,
) -> Result<File, String> {
    let mut current = root.path.clone();
    for component in components {
        current.push(component);
        let metadata = fs::symlink_metadata(&current)
            .map_err(|_| "The engine-browser entry is unavailable".to_string())?;
        if metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
            return Err("The engine-browser refused a link or reparse point".into());
        }
    }
    let metadata = fs::metadata(&current)
        .map_err(|_| "The engine-browser entry could not be inspected".to_string())?;
    if !match expected {
        ExpectedKind::Directory => metadata.is_dir(),
        ExpectedKind::File => metadata.is_file(),
    } {
        return Err("The engine-browser entry has the wrong type".into());
    }
    File::open(current).map_err(|_| "The engine-browser entry could not be opened".into())
}

#[cfg(not(unix))]
fn browse_directory(
    root: &TrustedRoot,
    components: &[String],
    offset: usize,
    limit: usize,
) -> Result<BrowsePage, String> {
    let mut path = root.path.clone();
    for component in components {
        path.push(component);
    }
    let _authority = open_relative(root, components, ExpectedKind::Directory)?;
    let mut entries = Vec::new();
    let mut scanned = 0usize;
    let mut serialized = 0usize;
    let mut has_more = false;
    for entry in fs::read_dir(path)
        .map_err(|_| "The engine-browser directory could not be enumerated".to_string())?
    {
        let entry = entry.map_err(|_| "An engine-browser entry could not be read".to_string())?;
        scanned = scanned.saturating_add(1);
        if scanned > MAX_DIRECTORY_WORK {
            return Err("The engine-browser directory exceeds the bounded work limit".into());
        }
        if scanned <= offset {
            continue;
        }
        if entries.len() >= limit {
            has_more = true;
            break;
        }
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|_| "An engine-browser entry could not be inspected".to_string())?;
        if metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
            return Err("The engine-browser refused a link or reparse point".into());
        }
        let kind = if metadata.is_dir() {
            EngineBrowseEntryKind::Directory
        } else if metadata.is_file() {
            EngineBrowseEntryKind::File
        } else {
            continue;
        };
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "The engine-browser found a non-Unicode entry name".to_string())?;
        let relative_path = if components.is_empty() {
            name.clone()
        } else {
            format!("{}/{}", components.join("/"), name)
        };
        let browse_entry = EngineBrowseEntry {
            name,
            relative_path,
            kind,
            size: metadata.len(),
            modified_at_ms: metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_millis() as u64),
            executable: metadata.is_file(),
        };
        let encoded = serde_json::to_vec(&browse_entry)
            .map_err(|_| "The engine-browser entry could not be encoded".to_string())?;
        if serialized.saturating_add(encoded.len()) > MAX_SERIALIZED_PAGE_BYTES {
            has_more = true;
            break;
        }
        serialized += encoded.len();
        entries.push(browse_entry);
    }
    Ok(BrowsePage {
        next_offset: has_more.then_some(offset.saturating_add(entries.len())),
        entries,
    })
}

#[cfg(windows)]
fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x400 != 0
}

#[cfg(not(any(unix, windows)))]
fn is_reparse_point(_metadata: &fs::Metadata) -> bool {
    false
}

#[cfg(unix)]
fn set_directory_mode(path: &Path, mode: u32) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|_| "The content-addressed engine store permissions could not be set".into())
}

#[cfg(not(unix))]
fn set_directory_mode(_path: &Path, _mode: u32) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
fn set_file_mode(file: &File, mode: u32) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(mode))
        .map_err(|_| "The content-addressed engine file permissions could not be set".into())
}

#[cfg(not(unix))]
fn set_file_mode(_file: &File, _mode: u32) -> Result<(), String> {
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::{copy_into_store, parse_relative_path, EngineAdmin, EngineBrowseRequest};
    use sha2::{Digest, Sha256};
    use std::{
        fs,
        os::unix::fs::{MetadataExt, PermissionsExt},
        path::Path,
    };
    use uuid::Uuid;

    fn temp_root(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("queen-runner-{label}-{}", Uuid::new_v4()))
    }

    fn configure(data: &Path, root: &Path) {
        fs::create_dir_all(data).unwrap();
        fs::create_dir_all(root).unwrap();
        fs::set_permissions(root, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(
            data.join("runner-config.json"),
            serde_json::json!({
                "engine_admin": "admin-installed",
                "engine_roots": [{"id": "trusted", "label": "Trusted engines", "path": root}],
            })
            .to_string(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn scoped_browser_rejects_parent_absolute_empty_and_overdeep_paths() {
        let data = temp_root("invalid-paths-data");
        let root = temp_root("invalid-paths-root");
        configure(&data, &root);
        let admin = EngineAdmin::load(&data).unwrap();
        for invalid in [
            "../outside",
            "/etc",
            "folder//engine",
            "folder/./engine",
            "a/a/a/a/a/a/a/a/a/a/a/a/a/a/a/a/a",
        ] {
            let result = admin
                .browse(EngineBrowseRequest {
                    root_id: "trusted".into(),
                    relative_path: invalid.into(),
                    cursor: None,
                    page_entries: None,
                })
                .await;
            assert!(
                result.is_err(),
                "{invalid} crossed the relative-path grammar"
            );
        }
        let _ = fs::remove_dir_all(data);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn scoped_browser_never_follows_a_symlink_outside_its_held_root() {
        let data = temp_root("symlink-data");
        let root = temp_root("symlink-root");
        let outside = temp_root("symlink-outside");
        configure(&data, &root);
        fs::create_dir_all(&outside).unwrap();
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(outside.join("secret-engine"), b"outside").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("escape")).unwrap();
        let admin = EngineAdmin::load(&data).unwrap();
        let error = admin
            .browse(EngineBrowseRequest {
                root_id: "trusted".into(),
                relative_path: "escape".into(),
                cursor: None,
                page_entries: None,
            })
            .await
            .unwrap_err();
        assert!(!error.contains(outside.to_string_lossy().as_ref()));
        let _ = fs::remove_dir_all(data);
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(outside);
    }

    #[tokio::test]
    async fn configured_root_is_opened_once_and_path_replacement_cannot_swap_its_authority() {
        let data = temp_root("held-root-data");
        let root = temp_root("held-root");
        configure(&data, &root);
        fs::write(root.join("original-engine"), b"original").unwrap();
        fs::set_permissions(
            root.join("original-engine"),
            fs::Permissions::from_mode(0o555),
        )
        .unwrap();
        let admin = EngineAdmin::load(&data).unwrap();
        let moved = root.with_extension("held");
        fs::rename(&root, &moved).unwrap();
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(root.join("replacement-engine"), b"replacement").unwrap();
        fs::set_permissions(
            root.join("replacement-engine"),
            fs::Permissions::from_mode(0o555),
        )
        .unwrap();
        let page = admin
            .browse(EngineBrowseRequest {
                root_id: "trusted".into(),
                relative_path: String::new(),
                cursor: None,
                page_entries: None,
            })
            .await
            .unwrap();
        assert!(page
            .entries
            .iter()
            .any(|entry| entry.name == "original-engine"));
        assert!(!page
            .entries
            .iter()
            .any(|entry| entry.name == "replacement-engine"));
        let _ = fs::remove_dir_all(data);
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(moved);
    }

    #[tokio::test]
    async fn repeated_browses_enumerate_root_and_subdirectory_from_start() {
        let data = temp_root("repeat-browse-data");
        let root = temp_root("repeat-browse-root");
        configure(&data, &root);
        let nested = root.join("nested");
        fs::create_dir(&nested).unwrap();
        fs::set_permissions(&nested, fs::Permissions::from_mode(0o755)).unwrap();
        for engine in [root.join("root-engine"), nested.join("nested-engine")] {
            fs::write(&engine, b"engine").unwrap();
            fs::set_permissions(engine, fs::Permissions::from_mode(0o555)).unwrap();
        }
        let admin = EngineAdmin::load(&data).unwrap();

        for relative_path in ["", "nested"] {
            let first = admin
                .browse(EngineBrowseRequest {
                    root_id: "trusted".into(),
                    relative_path: relative_path.into(),
                    cursor: None,
                    page_entries: None,
                })
                .await
                .unwrap();
            let second = admin
                .browse(EngineBrowseRequest {
                    root_id: "trusted".into(),
                    relative_path: relative_path.into(),
                    cursor: None,
                    page_entries: None,
                })
                .await
                .unwrap();
            assert!(!first.entries.is_empty());
            assert_eq!(second.entries, first.entries);
        }

        let _ = fs::remove_dir_all(data);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn scoped_browser_rejects_a_world_writable_traversed_directory_and_leaf() {
        let data = temp_root("ownership-data");
        let root = temp_root("ownership-root");
        configure(&data, &root);
        let writable = root.join("writable");
        fs::create_dir(&writable).unwrap();
        fs::set_permissions(&writable, fs::Permissions::from_mode(0o777)).unwrap();
        fs::write(root.join("engine"), b"uci").unwrap();
        fs::set_permissions(root.join("engine"), fs::Permissions::from_mode(0o777)).unwrap();
        let admin = EngineAdmin::load(&data).unwrap();
        assert!(admin
            .browse(EngineBrowseRequest {
                root_id: "trusted".into(),
                relative_path: "writable".into(),
                cursor: None,
                page_entries: None,
            })
            .await
            .is_err());
        assert!(admin
            .register("trusted".into(), "engine".into(), &test_core(&data))
            .await
            .is_err());
        let _ = fs::remove_dir_all(data);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn registration_copies_exact_bytes_to_an_immutable_content_address_before_source_mutation() {
        let data = temp_root("content-store-data");
        let root = temp_root("content-store-root");
        configure(&data, &root);
        let source = root.join("engine");
        fs::write(&source, b"trusted engine bytes v1").unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o555)).unwrap();
        let admin = EngineAdmin::load(&data).unwrap();
        let authority = admin.roots.get("trusted").unwrap();
        let stored = copy_into_store(
            authority,
            &parse_relative_path("engine", false).unwrap(),
            &admin.store,
            u64::MAX,
            0,
        )
        .unwrap();
        let expected = super::encode_hex(&Sha256::digest(b"trusted engine bytes v1"));
        assert_eq!(stored.file_name().unwrap().to_str().unwrap(), expected);
        assert_eq!(
            fs::metadata(&stored).unwrap().permissions().mode() & 0o777,
            0o555
        );
        assert_ne!(
            fs::metadata(&source).unwrap().ino(),
            fs::metadata(&stored).unwrap().ino(),
            "the managed identity must not alias the mutable source inode"
        );
        fs::set_permissions(&source, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(&source, b"changed later").unwrap();
        assert_eq!(fs::read(&stored).unwrap(), b"trusted engine bytes v1");
        let _ = fs::remove_dir_all(data);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn engine_store_quota_rejects_a_copy_before_any_temporary_bytes_are_written() {
        let data = temp_root("store-quota-data");
        let root = temp_root("store-quota-root");
        configure(&data, &root);
        fs::write(root.join("engine"), b"too many bytes").unwrap();
        fs::set_permissions(root.join("engine"), fs::Permissions::from_mode(0o555)).unwrap();
        let admin = EngineAdmin::load(&data).unwrap();
        let authority = admin.roots.get("trusted").unwrap();
        assert!(copy_into_store(
            authority,
            &parse_relative_path("engine", false).unwrap(),
            &admin.store,
            4,
            0,
        )
        .is_err());
        assert_eq!(fs::read_dir(&admin.store).unwrap().count(), 0);
        let _ = fs::remove_dir_all(data);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn pagination_cursors_are_opaque_single_use_and_bound_to_root_and_directory() {
        let data = temp_root("cursor-data");
        let root = temp_root("cursor-root");
        configure(&data, &root);
        for name in ["a", "b", "c"] {
            fs::write(root.join(name), b"engine").unwrap();
            fs::set_permissions(root.join(name), fs::Permissions::from_mode(0o555)).unwrap();
        }
        let admin = EngineAdmin::load(&data).unwrap();
        let first = admin
            .browse(EngineBrowseRequest {
                root_id: "trusted".into(),
                relative_path: String::new(),
                cursor: None,
                page_entries: Some(1),
            })
            .await
            .unwrap();
        let cursor = first.next_cursor.expect("bounded page cursor");
        assert!(!cursor.contains("trusted"));
        let mut tampered = cursor.clone();
        tampered.replace_range(0..1, if tampered.starts_with('A') { "B" } else { "A" });
        assert!(admin
            .browse(EngineBrowseRequest {
                root_id: "trusted".into(),
                relative_path: String::new(),
                cursor: Some(tampered),
                page_entries: Some(1),
            })
            .await
            .is_err());
        assert!(admin
            .browse(EngineBrowseRequest {
                root_id: "trusted".into(),
                relative_path: String::new(),
                cursor: Some(cursor.clone()),
                page_entries: Some(1),
            })
            .await
            .is_ok());
        assert!(admin
            .browse(EngineBrowseRequest {
                root_id: "trusted".into(),
                relative_path: String::new(),
                cursor: Some(cursor),
                page_entries: Some(1),
            })
            .await
            .is_err());
        let _ = fs::remove_dir_all(data);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn secure_install_remains_a_fail_closed_reserved_configuration() {
        let data = temp_root("secure-install");
        fs::create_dir_all(&data).unwrap();
        fs::write(
            data.join("runner-config.json"),
            r#"{"engine_admin":"secure-install"}"#,
        )
        .unwrap();
        let error = match EngineAdmin::load(&data) {
            Ok(_) => panic!("secure-install unexpectedly started"),
            Err(error) => error,
        };
        assert!(error.contains("reserved"));
        let _ = fs::remove_dir_all(data);
    }

    #[tokio::test]
    async fn startup_rejects_legacy_arbitrary_engine_paths_outside_the_managed_store() {
        let data = temp_root("legacy-engine-data");
        let root = temp_root("legacy-engine-root");
        configure(&data, &root);
        let admin = EngineAdmin::load(&data).unwrap();
        let config = config_with_engine(root.join("legacy-engine").to_string_lossy().to_string());
        let core = test_core_with_config(&data, config);
        let error = admin.validate_registered_engines(&core).await.unwrap_err();
        assert!(error.contains("predates trusted-engine storage"));
        let _ = fs::remove_dir_all(data);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn startup_rejects_store_bytes_that_no_longer_match_their_content_identity() {
        let data = temp_root("tampered-store-data");
        let root = temp_root("tampered-store-root");
        configure(&data, &root);
        let admin = EngineAdmin::load(&data).unwrap();
        let identity = super::encode_hex(&Sha256::digest(b"original bytes"));
        let stored = admin.store.join(&identity);
        fs::write(&stored, b"original bytes").unwrap();
        fs::set_permissions(&stored, fs::Permissions::from_mode(0o555)).unwrap();
        let core = test_core_with_config(
            &data,
            config_with_engine(stored.to_string_lossy().to_string()),
        );
        admin.validate_registered_engines(&core).await.unwrap();

        fs::set_permissions(&stored, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(&stored, b"changed by the trusted operator").unwrap();
        fs::set_permissions(&stored, fs::Permissions::from_mode(0o555)).unwrap();
        let error = admin.validate_registered_engines(&core).await.unwrap_err();
        assert!(error.contains("no longer matches its identity"));
        let _ = fs::remove_dir_all(data);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn startup_gc_removes_crash_temporaries_and_unreferenced_content_but_keeps_live_content()
    {
        let data = temp_root("store-gc-data");
        let root = temp_root("store-gc-root");
        configure(&data, &root);
        let admin = EngineAdmin::load(&data).unwrap();
        let live = "a".repeat(64);
        let orphan = "b".repeat(64);
        let temporary = ".0123456789abcdef0123456789abcdef.tmp";
        for name in [&live, &orphan, temporary] {
            fs::write(admin.store.join(name), b"engine").unwrap();
        }
        let core = test_core_with_config(
            &data,
            config_with_engine(admin.store.join(&live).to_string_lossy().to_string()),
        );
        admin.garbage_collect(&core).await;
        assert!(admin.store.join(live).exists());
        assert!(!admin.store.join(orphan).exists());
        assert!(!admin.store.join(temporary).exists());
        let _ = fs::remove_dir_all(data);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn legacy_uci_option_allowlist_key_does_not_brick_runner_config() {
        let data = temp_root("legacy-option-allowlist-data");
        let root = temp_root("legacy-option-allowlist-root");
        fs::create_dir_all(&data).unwrap();
        fs::create_dir_all(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(
            data.join("runner-config.json"),
            serde_json::json!({
                "engine_roots": [{"id": "trusted", "path": root}],
                "uci_option_allowlist": {"EvalFile": ["retired-value"]},
            })
            .to_string(),
        )
        .unwrap();
        let admin = EngineAdmin::load(&data).unwrap();
        assert_eq!(admin.roots().len(), 1);
        let _ = fs::remove_dir_all(data);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn arbitrary_string_option_values_register_persist_and_survive_restart() {
        let data = temp_root("arbitrary-option-data");
        let root = temp_root("arbitrary-option-root");
        configure(&data, &root);
        let source = root.join("engine");
        fs::write(
            &source,
            r#"#!/bin/sh
while IFS= read -r command; do
  case "$command" in
    uci) printf '%s\n' 'id name Operator Options' 'option name EvalFile type string default /engine/default/network.nnue' 'uciok' ;;
    "setoption name EvalFile value /operator/chosen/network.nnue") ;;
    setoption*) exit 9 ;;
    isready) printf '%s\n' 'readyok' ;;
    quit) exit 0 ;;
  esac
done
"#,
        )
        .unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o555)).unwrap();

        let core_data = data.join("core");
        let core = queen_core::AppState::new_with_secret_store(
            core_data.clone(),
            queen_core::models::AppConfig::default(),
            std::sync::Arc::new(queen_core::storage::FileSecretStore::new(
                core_data.join("secrets"),
            )),
        )
        .unwrap();
        let admin = EngineAdmin::load(&data).unwrap();
        let profile = admin
            .register("trusted".into(), "engine".into(), &core)
            .await
            .expect("register engine with a path-valued string option");
        let option = profile
            .options
            .iter()
            .find(|option| option.name == "EvalFile")
            .expect("probed EvalFile option");
        assert_eq!(
            option.default_value.as_deref(),
            Some("/engine/default/network.nnue")
        );

        queen_core::update_engine_options(
            profile.id,
            vec![queen_core::models::EngineOptionUpdate {
                name: "EvalFile".into(),
                value: Some("/operator/chosen/network.nnue".into()),
            }],
            queen_core::CoreStateRef::new(&core),
        )
        .await
        .expect("save the operator-chosen option value");
        drop(core);
        drop(admin);

        let restarted_admin = EngineAdmin::load(&data).expect("reconstruct engine store");
        let restarted_core = queen_core::AppState::load_with_secret_store(
            core_data.clone(),
            std::sync::Arc::new(queen_core::storage::FileSecretStore::new(
                core_data.join("secrets"),
            )),
        )
        .expect("reload persisted engine configuration");
        restarted_admin
            .validate_registered_engines(&restarted_core)
            .await
            .expect("accept the trusted engine and its operator-chosen option after restart");
        let snapshot = restarted_core.snapshot().await;
        let restarted_engine = &snapshot.engines[0];
        assert_eq!(
            restarted_engine.options[0].value.as_deref(),
            Some("/operator/chosen/network.nnue")
        );
        let mut launched = queen_core::uci::UciEngine::start(
            &restarted_engine.path,
            &restarted_engine.options,
            None,
        )
        .await
        .expect("launch the trusted engine with the persisted arbitrary value");
        launched.shutdown().await;

        drop(restarted_core);
        drop(restarted_admin);
        let _ = fs::remove_dir_all(data);
        let _ = fs::remove_dir_all(root);
    }

    fn config_with_engine(path: String) -> queen_core::models::AppConfig {
        queen_core::models::AppConfig {
            engines: vec![queen_core::models::EngineProfile {
                id: "engine".into(),
                name: "Test engine".into(),
                path,
                author: None,
                option_count: 0,
                last_probed_at_ms: None,
                probe_ok: None,
                options: Vec::new(),
                opening_book: None,
            }],
            ..queen_core::models::AppConfig::default()
        }
    }

    fn test_core(data: &Path) -> queen_core::AppState {
        test_core_with_config(data, queen_core::models::AppConfig::default())
    }

    fn test_core_with_config(
        data: &Path,
        config: queen_core::models::AppConfig,
    ) -> queen_core::AppState {
        let core_data = data.join(format!("core-{}", Uuid::new_v4()));
        queen_core::AppState::new_with_secret_store(
            core_data.clone(),
            config,
            std::sync::Arc::new(queen_core::storage::FileSecretStore::new(
                core_data.join("secrets"),
            )),
        )
        .unwrap()
    }
}
