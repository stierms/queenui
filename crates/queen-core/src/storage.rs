use crate::models::AppConfig;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

const CONFIG_FILE: &str = "queenui.json";
const AUTHORITY_LOCK_FILE: &str = ".queenui-authority.lock";
const ACTIVE_GAME_INTENTS_FILE: &str = "active-games.json";
const UNCERTAIN_CHALLENGE_CREATIONS_FILE: &str = "uncertain-challenge-creations.json";

/// Process-wide ownership of one QueenUI data directory. The operating-system
/// lock is released even after a crash, unlike a sentinel PID file. Keeping
/// the file in this value makes ownership last exactly as long as `AppState`.
#[derive(Debug)]
pub struct DataDirLock {
    file: fs::File,
    path: PathBuf,
}

impl DataDirLock {
    pub fn acquire(data_dir: &Path) -> Result<Self, String> {
        fs::create_dir_all(data_dir)
            .map_err(|error| format!("Could not create QueenUI data directory: {error}"))?;
        let path = data_dir.join(AUTHORITY_LOCK_FILE);
        let mut file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            // Truncation must happen only after ownership is acquired; a
            // second process may open this inode but must not alter it.
            .truncate(false)
            .open(&path)
            .map_err(|error| format!("Could not open the QueenUI authority lock: {error}"))?;
        file.try_lock_exclusive().map_err(|error| {
            format!(
                "QueenUI automation is already owned for data directory {} ({error})",
                data_dir.display()
            )
        })?;
        file.set_len(0)
            .and_then(|()| file.seek(SeekFrom::Start(0)).map(|_| ()))
            .and_then(|()| writeln!(file, "pid={}", std::process::id()))
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("Could not record QueenUI lock ownership: {error}"))?;
        Ok(Self { file, path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for DataDirLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveGameIntent {
    pub account_id: String,
    pub game_id: String,
}

pub fn active_game_intents_path(data_dir: &Path) -> PathBuf {
    data_dir.join(ACTIVE_GAME_INTENTS_FILE)
}

pub fn load_active_game_intents(path: &Path) -> Result<Vec<ActiveGameIntent>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    if path
        .metadata()
        .map_err(|error| format!("Could not inspect active-game recovery state: {error}"))?
        .len()
        > 256 * 1024
    {
        return Err("Active-game recovery state exceeds 256 KiB".into());
    }
    let bytes = fs::read(path)
        .map_err(|error| format!("Could not read active-game recovery state: {error}"))?;
    if bytes.len() > 256 * 1024 {
        return Err("Active-game recovery state exceeds 256 KiB".into());
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("Could not parse active-game recovery state: {error}"))
}

pub fn save_active_game_intents(path: &Path, intents: &[ActiveGameIntent]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Invalid active-game recovery path".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create QueenUI data directory: {error}"))?;
    let temporary = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(intents)
        .map_err(|error| format!("Could not encode active-game recovery state: {error}"))?;
    let mut file = fs::File::create(&temporary)
        .map_err(|error| format!("Could not write active-game recovery state: {error}"))?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("Could not flush active-game recovery state: {error}"))?;
    drop(file);
    fs::rename(&temporary, path)
        .map_err(|error| format!("Could not replace active-game recovery state: {error}"))
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UncertainChallengeCreation {
    pub account_id: String,
    pub opponent: String,
}

pub fn uncertain_challenge_creations_path(data_dir: &Path) -> PathBuf {
    data_dir.join(UNCERTAIN_CHALLENGE_CREATIONS_FILE)
}

pub fn load_uncertain_challenge_creations(
    path: &Path,
) -> Result<Vec<UncertainChallengeCreation>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    if path
        .metadata()
        .map_err(|error| format!("Could not inspect challenge-creation recovery state: {error}"))?
        .len()
        > 256 * 1024
    {
        return Err("Challenge-creation recovery state exceeds 256 KiB".into());
    }
    let bytes = fs::read(path)
        .map_err(|error| format!("Could not read challenge-creation recovery state: {error}"))?;
    if bytes.len() > 256 * 1024 {
        return Err("Challenge-creation recovery state exceeds 256 KiB".into());
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("Could not parse challenge-creation recovery state: {error}"))
}

pub fn save_uncertain_challenge_creations(
    path: &Path,
    creations: &[UncertainChallengeCreation],
) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Invalid challenge-creation recovery path".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create QueenUI data directory: {error}"))?;
    let temporary = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(creations)
        .map_err(|error| format!("Could not encode challenge-creation recovery state: {error}"))?;
    let mut file = fs::File::create(&temporary)
        .map_err(|error| format!("Could not write challenge-creation recovery state: {error}"))?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("Could not flush challenge-creation recovery state: {error}"))?;
    drop(file);
    fs::rename(&temporary, path)
        .map_err(|error| format!("Could not replace challenge-creation recovery state: {error}"))
}

pub fn config_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join(CONFIG_FILE)
}

pub fn load(path: &Path) -> Result<AppConfig, String> {
    if !path.exists() {
        return Ok(AppConfig::default());
    }

    let content = fs::read_to_string(path)
        .map_err(|error| format!("Could not read QueenUI configuration: {error}"))?;
    match serde_json::from_str(&content) {
        Ok(config) => Ok(config),
        Err(error) => {
            // Never brick app startup over a corrupt file: back it up and start fresh.
            let backup = path.with_extension("json.corrupt");
            let _ = fs::rename(path, &backup);
            crate::diagnostics::record(
                crate::diagnostics::DiagnosticEntry::error(
                    "storage",
                    "The configuration file could not be parsed, so QueenUI started from defaults",
                )
                .with_detail(format!(
                    "{error}; the file was moved to {}",
                    backup.display()
                )),
            );
            Ok(AppConfig::default())
        }
    }
}

pub fn save(path: &Path, config: &AppConfig) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Invalid configuration path".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create QueenUI data directory: {error}"))?;
    let temporary = path.with_extension("json.tmp");
    let content = serde_json::to_string_pretty(config)
        .map_err(|error| format!("Could not serialize QueenUI configuration: {error}"))?;
    let mut file = fs::File::create(&temporary)
        .map_err(|error| format!("Could not write QueenUI configuration: {error}"))?;
    file.write_all(content.as_bytes())
        .map_err(|error| format!("Could not write QueenUI configuration: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("Could not flush QueenUI configuration to disk: {error}"))?;
    drop(file);
    fs::rename(&temporary, path)
        .map_err(|error| format!("Could not replace QueenUI configuration: {error}"))
}

pub fn import_opening_book(
    config_path: &Path,
    engine_id: &str,
    source: &Path,
) -> Result<PathBuf, String> {
    let extension = source
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| "The opening book has no supported extension.".to_string())?;
    let directory = config_path
        .parent()
        .ok_or_else(|| "Invalid QueenUI data directory.".to_string())?
        .join("opening-books");
    fs::create_dir_all(&directory)
        .map_err(|error| format!("Could not create the opening-book directory: {error}"))?;
    let destination = directory.join(format!("{engine_id}.{extension}"));
    let source_canonical = source.canonicalize().ok();
    let destination_canonical = destination.canonicalize().ok();
    if source_canonical != destination_canonical {
        let temporary = directory.join(format!("{engine_id}.{extension}.tmp"));
        fs::copy(source, &temporary)
            .map_err(|error| format!("Could not import the opening book: {error}"))?;
        if destination.exists() {
            fs::remove_file(&destination)
                .map_err(|error| format!("Could not replace the opening book: {error}"))?;
        }
        fs::rename(&temporary, &destination)
            .map_err(|error| format!("Could not finish importing the opening book: {error}"))?;
    }
    Ok(destination)
}

pub fn remove_imported_opening_book(config_path: &Path, book_path: &Path) -> Result<(), String> {
    let managed_directory = config_path
        .parent()
        .ok_or_else(|| "Invalid QueenUI data directory.".to_string())?
        .join("opening-books");
    if book_path.parent() == Some(managed_directory.as_path()) && book_path.exists() {
        fs::remove_file(book_path)
            .map_err(|error| format!("Could not remove the imported opening book: {error}"))?;
    }
    Ok(())
}

/// Token persistence is supplied by the host so the portable runtime is not
/// tied to Windows Credential Manager or to a particular Linux secret service.
pub trait SecretStore: Send + Sync {
    fn store(&self, account_id: &str, token: &str) -> Result<(), String>;
    fn get(&self, account_id: &str) -> Result<String, String>;
    fn delete(&self, account_id: &str) -> Result<(), String>;
}

#[derive(Debug, Default)]
pub struct PlatformSecretStore;

impl SecretStore for PlatformSecretStore {
    fn store(&self, account_id: &str, token: &str) -> Result<(), String> {
        store_token(account_id, token)
    }

    fn get(&self, account_id: &str) -> Result<String, String> {
        get_token(account_id)
    }

    fn delete(&self, account_id: &str) -> Result<(), String> {
        delete_token(account_id)
    }
}

/// Headless Linux token store. Files are readable only by the service user;
/// the runner API never exposes their contents. This deliberately isolates the
/// storage policy so a system keyring or external secret manager can replace it
/// without touching game supervision.
#[derive(Clone, Debug)]
pub struct FileSecretStore {
    directory: PathBuf,
}

impl FileSecretStore {
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    fn path(&self, account_id: &str) -> Result<PathBuf, String> {
        if account_id.is_empty()
            || !account_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err("Invalid account id for token storage".into());
        }
        Ok(self.directory.join(format!("{account_id}.token")))
    }

    fn prepare_directory(&self) -> Result<(), String> {
        fs::create_dir_all(&self.directory)
            .map_err(|error| format!("Could not create the runner secret directory: {error}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.directory, fs::Permissions::from_mode(0o700)).map_err(
                |error| format!("Could not protect the runner secret directory: {error}"),
            )?;
        }
        Ok(())
    }
}

impl SecretStore for FileSecretStore {
    fn store(&self, account_id: &str, token: &str) -> Result<(), String> {
        self.prepare_directory()?;
        let path = self.path(account_id)?;
        let temporary = self
            .directory
            .join(format!(".{account_id}.{}.tmp", uuid::Uuid::new_v4()));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .map_err(|error| format!("Could not create the token file: {error}"))?;
        file.write_all(token.as_bytes())
            .map_err(|error| format!("Could not write the token file: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("Could not flush the token file: {error}"))?;
        drop(file);
        fs::rename(&temporary, &path)
            .map_err(|error| format!("Could not replace the token file: {error}"))?;
        Ok(())
    }

    fn get(&self, account_id: &str) -> Result<String, String> {
        let path = self.path(account_id)?;
        fs::read_to_string(path)
            .map(|token| token.trim().to_string())
            .map_err(|error| format!("Could not read the Lichess token: {error}"))
    }

    fn delete(&self, account_id: &str) -> Result<(), String> {
        let path = self.path(account_id)?;
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("Could not delete the Lichess token: {error}")),
        }
    }
}

#[cfg(windows)]
const CREDENTIAL_SERVICE: &str = "QueenUI Lichess";

#[cfg(windows)]
pub fn store_token(account_id: &str, token: &str) -> Result<(), String> {
    let entry = keyring::v1::Entry::new(CREDENTIAL_SERVICE, account_id)
        .map_err(|error| format!("Could not open Windows Credential Manager: {error}"))?;
    entry
        .set_password(token)
        .map_err(|error| format!("Could not save the Lichess token: {error}"))
}

#[cfg(windows)]
pub fn get_token(account_id: &str) -> Result<String, String> {
    let entry = keyring::v1::Entry::new(CREDENTIAL_SERVICE, account_id)
        .map_err(|error| format!("Could not open Windows Credential Manager: {error}"))?;
    entry
        .get_password()
        .map_err(|error| format!("Could not read the Lichess token: {error}"))
}

#[cfg(windows)]
pub fn delete_token(account_id: &str) -> Result<(), String> {
    let entry = keyring::v1::Entry::new(CREDENTIAL_SERVICE, account_id)
        .map_err(|error| format!("Could not open Windows Credential Manager: {error}"))?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(format!("Could not delete the Lichess token: {error}")),
    }
}

#[cfg(not(windows))]
pub fn store_token(_account_id: &str, _token: &str) -> Result<(), String> {
    Err("Lichess tokens are stored in Windows Credential Manager. Run QueenUI on Windows.".into())
}

#[cfg(not(windows))]
pub fn get_token(_account_id: &str) -> Result<String, String> {
    Err("Lichess tokens are stored in Windows Credential Manager. Run QueenUI on Windows.".into())
}

#[cfg(not(windows))]
pub fn delete_token(_account_id: &str) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        active_game_intents_path, config_path, load, load_active_game_intents,
        load_uncertain_challenge_creations, save, save_active_game_intents,
        save_uncertain_challenge_creations, uncertain_challenge_creations_path, ActiveGameIntent,
        DataDirLock, FileSecretStore, SecretStore, UncertainChallengeCreation,
    };
    use crate::models::{AppConfig, CampaignRuntime, CampaignSettings, EngineProfile};

    #[test]
    fn engine_probe_truth_survives_a_config_round_trip() {
        let directory = std::env::temp_dir().join(format!(
            "queenui-engine-probe-config-test-{}",
            uuid::Uuid::new_v4()
        ));
        let path = config_path(&directory);
        let config = AppConfig {
            engines: vec![EngineProfile {
                id: "engine".into(),
                name: "Test UCI".into(),
                path: "/engines/test-uci".into(),
                author: None,
                option_count: 0,
                last_probed_at_ms: Some(1_723_456_789_012),
                probe_ok: Some(true),
                options: Vec::new(),
                opening_book: None,
            }],
            ..AppConfig::default()
        };

        save(&path, &config).expect("save config");
        let loaded = load(&path).expect("load config");

        assert_eq!(loaded.engines[0].last_probed_at_ms, Some(1_723_456_789_012));
        assert_eq!(loaded.engines[0].probe_ok, Some(true));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn engine_profiles_without_probe_truth_load_as_never_probed() {
        let directory = std::env::temp_dir().join(format!(
            "queenui-legacy-engine-config-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = config_path(&directory);
        std::fs::write(
            &path,
            r#"{
  "engines": [{
    "id": "legacy-engine",
    "name": "Legacy UCI",
    "path": "/engines/legacy-uci",
    "author": null,
    "optionCount": 0
  }],
  "accounts": []
}"#,
        )
        .unwrap();

        let loaded = load(&path).expect("load config without probe fields");

        assert_eq!(loaded.engines[0].last_probed_at_ms, None);
        assert_eq!(loaded.engines[0].probe_ok, None);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn campaign_rated_defaults_true_for_models_and_legacy_config() {
        assert!(CampaignSettings::default().rated);
        let directory = std::env::temp_dir().join(format!(
            "queenui-campaign-rated-default-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = config_path(&directory);
        std::fs::write(
            &path,
            r#"{
  "engines": [],
  "accounts": [],
  "campaigns": [{
    "accountId": "bot",
    "minRating": 1800,
    "maxRating": 2400,
    "concurrency": 2,
    "clockLimit": 180,
    "clockIncrement": 2,
    "color": "random"
  }]
}"#,
        )
        .unwrap();

        let loaded = load(&path).expect("load campaign without rated field");

        assert!(loaded.campaigns[0].rated);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn campaign_runtime_from_an_older_runner_defaults_additive_fields() {
        let runtime: CampaignRuntime = serde_json::from_value(serde_json::json!({
            "accountId": "bot",
            "status": "running",
            "activeGames": 1,
            "pendingChallenges": 0,
            "eligibleBots": 4,
            "onlineBotsScanned": 20,
            "challengesSent": 7,
            "lastOpponent": "Opponent",
            "activity": "At capacity",
            "error": null,
            "nextScanAt": null,
            "events": []
        }))
        .expect("deserialize a protocol-v2 campaign runtime");

        assert_eq!(runtime.games_started, 0);
        assert_eq!(runtime.games_completed, 0);
        assert_eq!(runtime.stop_at, None);
    }

    #[test]
    fn data_directory_lock_refuses_a_second_owner_and_releases_on_drop() {
        let directory =
            std::env::temp_dir().join(format!("queenui-lock-test-{}", uuid::Uuid::new_v4()));
        let first = DataDirLock::acquire(&directory).expect("first owner");
        assert!(DataDirLock::acquire(&directory).is_err());
        drop(first);
        assert!(DataDirLock::acquire(&directory).is_ok());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn active_game_intents_round_trip() {
        let directory =
            std::env::temp_dir().join(format!("queenui-intent-test-{}", uuid::Uuid::new_v4()));
        let path = active_game_intents_path(&directory);
        let intents = vec![ActiveGameIntent {
            account_id: "bot".into(),
            game_id: "game".into(),
        }];
        save_active_game_intents(&path, &intents).expect("save intents");
        assert_eq!(load_active_game_intents(&path).unwrap(), intents);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn uncertain_challenge_creations_round_trip() {
        let directory = std::env::temp_dir().join(format!(
            "queenui-challenge-recovery-test-{}",
            uuid::Uuid::new_v4()
        ));
        let path = uncertain_challenge_creations_path(&directory);
        let creations = vec![UncertainChallengeCreation {
            account_id: "bot".into(),
            opponent: "Opponent".into(),
        }];
        save_uncertain_challenge_creations(&path, &creations).expect("save creations");
        assert_eq!(
            load_uncertain_challenge_creations(&path).unwrap(),
            creations
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn file_secret_store_replaces_and_idempotently_deletes_credentials() {
        let directory =
            std::env::temp_dir().join(format!("queenui-secrets-test-{}", uuid::Uuid::new_v4()));
        let store = FileSecretStore::new(directory.clone());
        store.store("bot", "first").unwrap();
        assert_eq!(store.get("bot").unwrap(), "first");
        store.store("bot", "replacement").unwrap();
        assert_eq!(store.get("bot").unwrap(), "replacement");
        store.delete("bot").unwrap();
        store.delete("bot").unwrap();
        assert!(store.get("bot").is_err());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn authority_lock_subprocess_entrypoint() {
        let Some(mode) = std::env::var_os("QUEENUI_LOCK_TEST_MODE") else {
            return;
        };
        let directory = std::path::PathBuf::from(
            std::env::var_os("QUEENUI_LOCK_TEST_DIR").expect("lock test directory"),
        );
        let marker = std::path::PathBuf::from(
            std::env::var_os("QUEENUI_LOCK_TEST_MARKER").expect("lock test marker"),
        );
        match mode.to_str().expect("lock test mode") {
            "hold" => {
                let _owner = DataDirLock::acquire(&directory).expect("subprocess authority");
                std::fs::write(marker, b"owned").expect("write ownership marker");
                loop {
                    std::thread::park();
                }
            }
            "attempt" => match DataDirLock::acquire(&directory) {
                Ok(_owner) => {
                    std::fs::write(marker, b"automation-started").expect("write automation marker");
                }
                Err(_) => std::process::exit(23),
            },
            other => panic!("unknown lock test mode {other}"),
        }
    }

    #[test]
    fn data_directory_lock_fences_processes_and_recovers_after_kill() {
        let root =
            std::env::temp_dir().join(format!("queenui-process-lock-{}", uuid::Uuid::new_v4()));
        let directory = root.join("data");
        let owner_marker = root.join("owner-ready");
        let blocked_marker = root.join("blocked-automation");
        let recovered_marker = root.join("recovered-automation");
        std::fs::create_dir_all(&root).unwrap();
        let executable = std::env::current_exe().expect("current test executable");
        let spawn = |mode: &str, marker: &std::path::Path| {
            std::process::Command::new(&executable)
                .args([
                    "--exact",
                    "storage::tests::authority_lock_subprocess_entrypoint",
                    "--nocapture",
                ])
                .env("QUEENUI_LOCK_TEST_MODE", mode)
                .env("QUEENUI_LOCK_TEST_DIR", &directory)
                .env("QUEENUI_LOCK_TEST_MARKER", marker)
                .spawn()
                .expect("spawn authority subprocess")
        };

        let mut owner = spawn("hold", &owner_marker);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !owner_marker.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            owner_marker.exists(),
            "first authority never acquired the lock"
        );

        let blocked = spawn("attempt", &blocked_marker)
            .wait()
            .expect("wait for blocked authority");
        assert_eq!(blocked.code(), Some(23));
        assert!(
            !blocked_marker.exists(),
            "the second process reached automation before owning the data directory"
        );

        owner.kill().expect("kill first authority");
        owner.wait().expect("reap first authority");
        let recovered = spawn("attempt", &recovered_marker)
            .wait()
            .expect("wait for recovered authority");
        assert!(recovered.success());
        assert!(recovered_marker.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn authority_lock_is_deliberately_scoped_to_one_data_directory() {
        let root =
            std::env::temp_dir().join(format!("queenui-lock-scope-{}", uuid::Uuid::new_v4()));
        let first = DataDirLock::acquire(&root.join("first")).unwrap();
        let second = DataDirLock::acquire(&root.join("second")).unwrap();
        drop((first, second));
        let _ = std::fs::remove_dir_all(root);
    }
}
