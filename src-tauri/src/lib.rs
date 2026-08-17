use queen_client::{redeem_pairing_payload, RunnerClient};
use queen_core::{
    diagnostics, enginelog, history,
    models::{
        AddAccountRequest, AddAccountResult, AppSnapshot, CampaignSettings, ChallengeRequest,
        ChallengeResult, EngineOptionUpdate, EngineProfile, OpeningBookConfig, OpeningBookUpdate,
    },
    AppState, CoreEvent, CoreStateRef,
};
use queen_protocol::{
    EngineBrowseRequest, EngineBrowseResponse, EngineRoot, RunnerCommand, RunnerIdentity,
    PAIRING_PAYLOAD_VERSION,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::{Emitter, Manager, State};
use tokio_util::sync::CancellationToken;

const SNAPSHOT_EVENT: &str = "queenui://snapshot";
const LOGS_UPDATED_EVENT: &str = "queenui://logs-updated";
const HISTORY_UPDATED_EVENT: &str = "queenui://history-updated";
const DIAGNOSTIC_EVENT: &str = "queenui://diagnostic";
const CLOSE_REQUESTED_EVENT: &str = "queenui://close-requested";
const RUNNER_CONNECTION_EVENT: &str = "queenui://runner-connection";
const DESKTOP_CONFIG_FILE: &str = "queenui-desktop.json";
const REMOTE_EVENT_HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(5);
const RUNNER_IDENTITY_KEY: &str = "runner-identity-v2";
const LEGACY_RUNNER_TOKEN_KEY: &str = "active-runner";
const SWITCHING_RUNNERS_ERROR: &str = "QueenUI is switching runners; retry in a moment";
const BACKEND_LEASE_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const INTERRUPTED_SWITCH_ERROR: &str =
    "The runner switch was interrupted; save runner settings again to recover the backend";

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, rename_all = "camelCase")]
struct DesktopRunnerConfig {
    mode: String,
    url: Option<String>,
    /// Retained only to deserialize pre-B1 settings. A true value is rejected
    /// by every command path and is never reflected as active configuration.
    allow_insecure_remote_http: bool,
}

impl Default for DesktopRunnerConfig {
    fn default() -> Self {
        Self {
            mode: "embedded".into(),
            url: None,
            allow_insecure_remote_http: false,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyRunnerCredential {
    endpoint: String,
    #[serde(rename = "token")]
    _token: serde::de::IgnoredAny,
}

struct RunnerSettingsState {
    data_dir: PathBuf,
    configured: std::sync::RwLock<DesktopRunnerConfig>,
    active: std::sync::RwLock<ActiveRunner>,
    change_gate: tokio::sync::Mutex<()>,
    source: String,
}

#[derive(Clone, Debug, PartialEq)]
struct ActiveRunner {
    mode: String,
    url: Option<String>,
    available: bool,
    identity_generation: Option<u64>,
}

#[derive(Clone, Debug, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
struct RunnerSettingsView {
    mode: String,
    url: Option<String>,
    paired: bool,
    active_mode: String,
    source: String,
    restart_required: bool,
    allow_insecure_remote_http: bool,
}

#[derive(Clone, Debug, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
struct RunnerConnectionTest {
    hostname: String,
    operating_system: String,
    architecture: String,
    logical_cpus: usize,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize, ts_rs::TS)]
#[serde(rename_all = "lowercase")]
enum RunnerConnectionState {
    Embedded,
    Connected,
    Reconnecting,
    Disconnected,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
struct RunnerConnectionEvent {
    backend_generation: u64,
    state: RunnerConnectionState,
    attempt: u32,
    last_ok_ts: Option<u64>,
    detail: Option<String>,
}

#[derive(Clone, Debug)]
struct BackendCoreEvent {
    backend_generation: u64,
    event: CoreEvent,
}

#[derive(Clone, Debug, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
/// Shared envelope for both `get_snapshot` responses and snapshot events.
/// Consumers accept `payload` only while `backendGeneration` is current.
struct BackendSnapshotEvent {
    backend_generation: u64,
    payload: AppSnapshot,
}

#[derive(Clone, Copy, Debug, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
struct BackendNotificationEvent {
    backend_generation: u64,
    payload: (),
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CloseRequestedPayload {
    reported_count: usize,
}

struct EmbeddedBackend {
    backend_generation: u64,
    core: AppState,
    event_cancellation: CancellationToken,
    event_task: tauri::async_runtime::JoinHandle<()>,
}

struct RemoteBackend {
    backend_generation: u64,
    client: RunnerClient,
    event_cancellation: CancellationToken,
    event_task: tauri::async_runtime::JoinHandle<()>,
}

enum ActiveBackend {
    Embedded(EmbeddedBackend),
    Remote(RemoteBackend),
}

enum BackendSlot {
    Active(ActiveBackend),
    Switching,
    Unavailable(String),
}

struct BackendState {
    slot: std::sync::RwLock<BackendSlot>,
    leases: std::sync::Arc<BackendLeases>,
    generation: std::sync::atomic::AtomicU64,
}

#[derive(Clone)]
enum DispatchTarget {
    Embedded(AppState),
    Remote(RunnerClient),
}

struct DispatchBackend {
    target: DispatchTarget,
    backend_generation: u64,
    _lease: BackendLease,
}

impl DispatchBackend {
    fn target(&self) -> &DispatchTarget {
        &self.target
    }
}

#[derive(Default)]
struct BackendLeases {
    active: std::sync::atomic::AtomicUsize,
    drained: tokio::sync::Notify,
}

struct BackendLease {
    leases: std::sync::Arc<BackendLeases>,
}

enum PreviousBackend {
    Active(ActiveBackend),
    Unavailable(String),
}

struct BackendSwitchGuard<'a> {
    backend: &'a BackendState,
    settings: Option<&'a RunnerSettingsState>,
    previous: Option<PreviousBackend>,
    completed: bool,
}

enum EmbeddedLoadRecovery {
    RemoteClient {
        client: RunnerClient,
        backend_generation: u64,
    },
    Unavailable(String),
    Embedded(EmbeddedBackend),
}

#[derive(Clone)]
struct BackendEvents {
    core: std::sync::Arc<dyn Fn(BackendCoreEvent) + Send + Sync>,
    connection: std::sync::Arc<dyn Fn(RunnerConnectionEvent) + Send + Sync>,
}

impl BackendEvents {
    fn for_app(handle: tauri::AppHandle) -> Self {
        let core_handle = handle.clone();
        Self {
            core: std::sync::Arc::new(move |event| emit_core_event(&core_handle, event)),
            connection: std::sync::Arc::new(move |event| emit_runner_connection(&handle, event)),
        }
    }

    #[cfg(test)]
    fn noop() -> Self {
        Self {
            core: std::sync::Arc::new(|_| {}),
            connection: std::sync::Arc::new(|_| {}),
        }
    }

    fn emit_core(&self, backend_generation: u64, event: CoreEvent) {
        (self.core)(BackendCoreEvent {
            backend_generation,
            event,
        });
    }

    fn emit_connection(&self, event: RunnerConnectionEvent) {
        (self.connection)(event);
    }
}

impl EmbeddedBackend {
    fn new(core: AppState, events: BackendEvents) -> Self {
        Self::new_at_generation(core, events, 1)
    }

    fn new_at_generation(core: AppState, events: BackendEvents, backend_generation: u64) -> Self {
        let event_cancellation = CancellationToken::new();
        let event_task = forward_embedded_events(
            events,
            &core,
            backend_generation,
            event_cancellation.clone(),
        );
        Self {
            backend_generation,
            core,
            event_cancellation,
            event_task,
        }
    }
}

impl RemoteBackend {
    fn new(client: RunnerClient, events: BackendEvents) -> Self {
        Self::new_at_generation(client, events, 1)
    }

    fn new_at_generation(
        client: RunnerClient,
        events: BackendEvents,
        backend_generation: u64,
    ) -> Self {
        let event_cancellation = CancellationToken::new();
        let event_task = forward_remote_events(
            events,
            client.clone(),
            backend_generation,
            event_cancellation.clone(),
        );
        Self {
            backend_generation,
            client,
            event_cancellation,
            event_task,
        }
    }
}

impl BackendState {
    fn active(backend: ActiveBackend) -> Self {
        let generation = match &backend {
            ActiveBackend::Embedded(embedded) => embedded.backend_generation,
            ActiveBackend::Remote(remote) => remote.backend_generation,
        };
        Self {
            slot: std::sync::RwLock::new(BackendSlot::Active(backend)),
            leases: std::sync::Arc::new(BackendLeases::default()),
            generation: std::sync::atomic::AtomicU64::new(generation),
        }
    }

    fn unavailable(error: String) -> Self {
        Self {
            slot: std::sync::RwLock::new(BackendSlot::Unavailable(error)),
            leases: std::sync::Arc::new(BackendLeases::default()),
            generation: std::sync::atomic::AtomicU64::new(0),
        }
    }

    fn next_generation(&self) -> u64 {
        self.generation
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
            .saturating_add(1)
    }

    /// Clones the command-capable handle and admits a lease atomically under
    /// the holder lock. The returned value keeps the lease across the command.
    fn dispatch_backend(&self) -> Result<DispatchBackend, String> {
        let slot = self
            .slot
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (target, backend_generation) = match &*slot {
            BackendSlot::Active(ActiveBackend::Embedded(embedded)) => (
                DispatchTarget::Embedded(embedded.core.clone()),
                embedded.backend_generation,
            ),
            BackendSlot::Active(ActiveBackend::Remote(remote)) => (
                DispatchTarget::Remote(remote.client.clone()),
                remote.backend_generation,
            ),
            BackendSlot::Switching => return Err(SWITCHING_RUNNERS_ERROR.into()),
            BackendSlot::Unavailable(error) => return Err(error.clone()),
        };
        self.leases
            .active
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        Ok(DispatchBackend {
            target,
            backend_generation,
            _lease: BackendLease {
                leases: self.leases.clone(),
            },
        })
    }

    /// Closes command admission atomically, then waits for all commands that
    /// already cloned a backend handle to finish their full await span.
    async fn begin_switch(&self) -> Result<BackendSwitchGuard<'_>, String> {
        let previous = {
            let mut slot = self
                .slot
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match std::mem::replace(&mut *slot, BackendSlot::Switching) {
                BackendSlot::Active(backend) => PreviousBackend::Active(backend),
                BackendSlot::Unavailable(error) => PreviousBackend::Unavailable(error),
                BackendSlot::Switching => return Err(SWITCHING_RUNNERS_ERROR.into()),
            }
        };
        let mut guard = BackendSwitchGuard {
            backend: self,
            settings: None,
            previous: Some(previous),
            completed: false,
        };
        if tokio::time::timeout(
            BACKEND_LEASE_DRAIN_TIMEOUT,
            self.leases.wait_until_drained(),
        )
        .await
        .is_err()
        {
            guard.restore();
            return Err(SWITCHING_RUNNERS_ERROR.into());
        }
        Ok(guard)
    }

    async fn begin_settings_switch<'a>(
        &'a self,
        settings: &'a RunnerSettingsState,
    ) -> Result<BackendSwitchGuard<'a>, String> {
        let mut guard = self.begin_switch().await?;
        guard.settings = Some(settings);
        Ok(guard)
    }

    fn is_active(&self) -> bool {
        matches!(
            &*self
                .slot
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            BackendSlot::Active(_)
        )
    }

    fn live_game_count(&self) -> usize {
        let slot = self
            .slot
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match &*slot {
            BackendSlot::Active(ActiveBackend::Embedded(embedded)) => {
                embedded.core.live_game_count()
            }
            // A remote runner owns its games, so closing this window is safe.
            BackendSlot::Active(ActiveBackend::Remote(_))
            | BackendSlot::Switching
            | BackendSlot::Unavailable(_) => 0,
        }
    }
}

impl BackendLeases {
    async fn wait_until_drained(&self) {
        loop {
            let drained = self.drained.notified();
            if self.active.load(std::sync::atomic::Ordering::Acquire) == 0 {
                return;
            }
            drained.await;
        }
    }
}

impl Drop for BackendLease {
    fn drop(&mut self) {
        let prior = self
            .leases
            .active
            .fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
        if prior == 1 {
            // Only one change gate can drain at a time. `notify_one` stores a
            // permit if the final lease drops between the count check and the
            // waiter being polled, avoiding a lost wakeup.
            self.leases.drained.notify_one();
        }
    }
}

impl BackendSwitchGuard<'_> {
    fn previous(&self) -> Result<&PreviousBackend, String> {
        self.previous
            .as_ref()
            .ok_or_else(|| INTERRUPTED_SWITCH_ERROR.to_string())
    }

    fn take_previous(&mut self) -> Result<PreviousBackend, String> {
        self.previous
            .take()
            .ok_or_else(|| INTERRUPTED_SWITCH_ERROR.to_string())
    }

    fn restore(&mut self) {
        let slot = self
            .previous
            .take()
            .map(|previous| match previous {
                PreviousBackend::Active(backend) => BackendSlot::Active(backend),
                PreviousBackend::Unavailable(error) => BackendSlot::Unavailable(error),
            })
            .unwrap_or_else(|| BackendSlot::Unavailable(INTERRUPTED_SWITCH_ERROR.into()));
        *self
            .backend
            .slot
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = slot;
        self.completed = true;
    }

    fn finish(&mut self, backend: ActiveBackend) {
        *self
            .backend
            .slot
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = BackendSlot::Active(backend);
        self.previous = None;
        self.completed = true;
    }

    fn finish_previous(&mut self, previous: PreviousBackend) {
        *self
            .backend
            .slot
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = match previous {
            PreviousBackend::Active(backend) => BackendSlot::Active(backend),
            PreviousBackend::Unavailable(error) => BackendSlot::Unavailable(error),
        };
        self.previous = None;
        self.completed = true;
    }
}

impl Drop for BackendSwitchGuard<'_> {
    fn drop(&mut self) {
        if !self.completed {
            let backend_was_consumed = self.previous.is_none();
            self.restore();
            if backend_was_consumed {
                if let Some(settings) = self.settings {
                    settings
                        .active
                        .write()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .available = false;
                }
            }
        }
    }
}

macro_rules! dispatch {
    ($state:expr, $local:ident => $embedded:expr, $command:expr) => {{
        let dispatch = $state.dispatch_backend()?;
        match dispatch.target() {
            DispatchTarget::Embedded($local) => $embedded,
            DispatchTarget::Remote(client) => client.command($command).await,
        }
    }};
}

#[tauri::command]
async fn get_snapshot(state: State<'_, BackendState>) -> Result<BackendSnapshotEvent, String> {
    get_snapshot_inner(state.inner()).await
}

async fn get_snapshot_inner(state: &BackendState) -> Result<BackendSnapshotEvent, String> {
    let result = match state.dispatch_backend() {
        Ok(dispatch) => {
            let backend_generation = dispatch.backend_generation;
            match dispatch.target() {
                DispatchTarget::Embedded(local) => Ok(local.snapshot().await),
                DispatchTarget::Remote(client) => {
                    client.snapshot().await.map(redact_remote_snapshot)
                }
            }
            .map(|payload| BackendSnapshotEvent {
                backend_generation,
                payload,
            })
        }
        Err(error) if error == SWITCHING_RUNNERS_ERROR => return Err(error),
        Err(error) => Err(error),
    };
    result.map_err(|_| operator_safe_snapshot_error())
}

fn operator_safe_snapshot_error() -> String {
    "QueenUI could not retrieve a backend snapshot. Check the runner connection and Logs for a safe diagnostic summary."
        .into()
}

#[tauri::command]
fn write_pgn_file(path: String, contents: String) -> Result<(), String> {
    queen_core::write_pgn_file(path, contents)
}

#[tauri::command]
async fn add_engine(path: String, state: State<'_, BackendState>) -> Result<EngineProfile, String> {
    let dispatch = state.dispatch_backend()?;
    match dispatch.target() {
        DispatchTarget::Embedded(local) => {
            queen_core::add_engine(path, CoreStateRef::new(local)).await
        }
        DispatchTarget::Remote(_) => Err(
            "Remote arbitrary-path registration is disabled; use the scoped engine browser".into(),
        ),
    }
}

#[tauri::command]
async fn list_engine_roots(state: State<'_, BackendState>) -> Result<Vec<EngineRoot>, String> {
    let dispatch = state.dispatch_backend()?;
    match dispatch.target() {
        DispatchTarget::Remote(client) => client.engine_roots().await,
        DispatchTarget::Embedded(_) => {
            Err("The scoped engine browser is only available with a remote runner".into())
        }
    }
}

#[tauri::command]
async fn browse_engine_root(
    request: EngineBrowseRequest,
    state: State<'_, BackendState>,
) -> Result<EngineBrowseResponse, String> {
    let dispatch = state.dispatch_backend()?;
    match dispatch.target() {
        DispatchTarget::Remote(client) => client.browse_engines(request).await,
        DispatchTarget::Embedded(_) => {
            Err("The scoped engine browser is only available with a remote runner".into())
        }
    }
}

#[tauri::command]
async fn register_engine(
    root_id: String,
    relative_path: String,
    state: State<'_, BackendState>,
) -> Result<(), String> {
    let dispatch = state.dispatch_backend()?;
    match dispatch.target() {
        DispatchTarget::Remote(client) => {
            let _: EngineProfile = client
                .command(RunnerCommand::RegisterEngine {
                    root_id,
                    relative_path,
                })
                .await?;
            Ok(())
        }
        DispatchTarget::Embedded(_) => {
            Err("Use the native engine picker for an embedded runner".into())
        }
    }
}

#[tauri::command]
async fn remove_engine(engine_id: String, state: State<'_, BackendState>) -> Result<(), String> {
    dispatch!(
        state,
        local => queen_core::remove_engine(engine_id.clone(), CoreStateRef::new(local)).await,
        RunnerCommand::RemoveEngine { engine_id }
    )
}

#[tauri::command]
async fn update_engine_options(
    engine_id: String,
    options: Vec<EngineOptionUpdate>,
    state: State<'_, BackendState>,
) -> Result<(), String> {
    dispatch!(
        state,
        local => queen_core::update_engine_options(
            engine_id.clone(),
            options.clone(),
            CoreStateRef::new(local),
        ).await,
        RunnerCommand::UpdateEngineOptions { engine_id, options }
    )
}

#[tauri::command]
async fn refresh_engine_options(
    engine_id: String,
    state: State<'_, BackendState>,
) -> Result<(), String> {
    dispatch!(
        state,
        local => queen_core::refresh_engine_options(
            engine_id.clone(),
            CoreStateRef::new(local),
        ).await,
        RunnerCommand::RefreshEngineOptions { engine_id }
    )
}

#[tauri::command]
async fn configure_opening_book(
    request: OpeningBookUpdate,
    state: State<'_, BackendState>,
) -> Result<OpeningBookConfig, String> {
    dispatch!(
        state,
        local => queen_core::configure_opening_book(
            request.clone(),
            CoreStateRef::new(local),
        ).await,
        RunnerCommand::ConfigureOpeningBook { request }
    )
}

#[tauri::command]
async fn clear_engine_opening_book(
    engine_id: String,
    state: State<'_, BackendState>,
) -> Result<(), String> {
    dispatch!(
        state,
        local => queen_core::clear_engine_opening_book(
            engine_id.clone(),
            CoreStateRef::new(local),
        ).await,
        RunnerCommand::ClearEngineOpeningBook { engine_id }
    )
}

#[tauri::command]
async fn add_lichess_account(
    request: AddAccountRequest,
    state: State<'_, BackendState>,
) -> Result<AddAccountResult, String> {
    add_lichess_account_inner(request, state.inner()).await
}

async fn add_lichess_account_inner(
    request: AddAccountRequest,
    state: &BackendState,
) -> Result<AddAccountResult, String> {
    dispatch!(
        state,
        local => queen_core::add_lichess_account(request.clone(), CoreStateRef::new(local)).await,
        RunnerCommand::AddLichessAccount { request }
    )
}

fn additive_runner_command_error(error: String, command: &str) -> String {
    if error.contains("HTTP 400") {
        format!(
            "The connected runner does not support {command}. Update queen-runner and try again."
        )
    } else {
        error
    }
}

#[tauri::command]
async fn update_lichess_account_token(
    account_id: String,
    token: String,
    state: State<'_, BackendState>,
) -> Result<AddAccountResult, String> {
    update_lichess_account_token_inner(account_id, token, state.inner()).await
}

async fn update_lichess_account_token_inner(
    account_id: String,
    token: String,
    state: &BackendState,
) -> Result<AddAccountResult, String> {
    let dispatch = state.dispatch_backend()?;
    match dispatch.target() {
        DispatchTarget::Embedded(local) => {
            queen_core::update_lichess_account_token(account_id, token, CoreStateRef::new(local))
                .await
        }
        DispatchTarget::Remote(client) => client
            .command(RunnerCommand::UpdateLichessAccountToken { account_id, token })
            .await
            .map_err(|error| additive_runner_command_error(error, "updateLichessAccountToken")),
    }
}

#[tauri::command]
async fn update_account_engine(
    account_id: String,
    engine_id: String,
    state: State<'_, BackendState>,
) -> Result<(), String> {
    dispatch!(
        state,
        local => queen_core::update_account_engine(
            account_id.clone(),
            engine_id.clone(),
            CoreStateRef::new(local),
        ).await,
        RunnerCommand::UpdateAccountEngine { account_id, engine_id }
    )
}

#[tauri::command]
async fn remove_lichess_account(
    account_id: String,
    state: State<'_, BackendState>,
) -> Result<(), String> {
    dispatch!(
        state,
        local => queen_core::remove_lichess_account(
            account_id.clone(),
            CoreStateRef::new(local),
        ).await,
        RunnerCommand::RemoveLichessAccount { account_id }
    )
}

#[tauri::command]
async fn dismiss_game_error(game_id: String, state: State<'_, BackendState>) -> Result<(), String> {
    dismiss_game_error_inner(game_id, state.inner()).await
}

async fn dismiss_game_error_inner(game_id: String, state: &BackendState) -> Result<(), String> {
    let dispatch = state.dispatch_backend()?;
    match dispatch.target() {
        DispatchTarget::Embedded(local) => {
            queen_core::dismiss_game_error(game_id, CoreStateRef::new(local)).await
        }
        DispatchTarget::Remote(client) => client
            .command(RunnerCommand::DismissGameError { game_id })
            .await
            .map_err(|error| additive_runner_command_error(error, "dismissGameError")),
    }
}

#[tauri::command]
async fn start_bot(account_id: String, state: State<'_, BackendState>) -> Result<(), String> {
    dispatch!(
        state,
        local => queen_core::start_bot(account_id.clone(), CoreStateRef::new(local)).await,
        RunnerCommand::StartBot { account_id }
    )
}

#[tauri::command]
async fn stop_bot(account_id: String, state: State<'_, BackendState>) -> Result<(), String> {
    dispatch!(
        state,
        local => queen_core::stop_bot(account_id.clone(), CoreStateRef::new(local)).await,
        RunnerCommand::StopBot { account_id }
    )
}

#[tauri::command]
async fn start_campaign(
    settings: CampaignSettings,
    state: State<'_, BackendState>,
) -> Result<(), String> {
    dispatch!(
        state,
        local => queen_core::start_campaign(settings.clone(), CoreStateRef::new(local)).await,
        RunnerCommand::StartCampaign { settings }
    )
}

#[tauri::command]
async fn stop_campaign(account_id: String, state: State<'_, BackendState>) -> Result<(), String> {
    dispatch!(
        state,
        local => queen_core::stop_campaign(account_id.clone(), CoreStateRef::new(local)).await,
        RunnerCommand::StopCampaign { account_id }
    )
}

#[tauri::command]
async fn create_challenge(
    request: ChallengeRequest,
    state: State<'_, BackendState>,
) -> Result<ChallengeResult, String> {
    dispatch!(
        state,
        local => queen_core::create_challenge(request.clone(), CoreStateRef::new(local)).await,
        RunnerCommand::CreateChallenge { request }
    )
}

#[tauri::command]
async fn get_scorebook_stats(
    filter: history::ScorebookFilter,
    state: State<'_, BackendState>,
) -> Result<history::ScorebookStats, String> {
    dispatch!(
        state,
        local => queen_core::get_scorebook_stats(filter.clone(), CoreStateRef::new(local)).await,
        RunnerCommand::GetScorebookStats { filter }
    )
}

#[tauri::command]
async fn import_lichess_history(
    account_id: String,
    max: Option<u32>,
    state: State<'_, BackendState>,
) -> Result<history::ImportReport, String> {
    dispatch!(
        state,
        local => queen_core::import_lichess_history(
            account_id.clone(),
            max,
            CoreStateRef::new(local),
        ).await,
        RunnerCommand::ImportLichessHistory { account_id, max }
    )
}

#[tauri::command]
async fn list_log_sessions(
    filter: enginelog::LogFilter,
    state: State<'_, BackendState>,
) -> Result<Vec<enginelog::LogSessionSummary>, String> {
    dispatch!(
        state,
        local => queen_core::list_log_sessions(filter.clone(), CoreStateRef::new(local)).await,
        RunnerCommand::ListLogSessions { filter }
    )
}

#[tauri::command]
async fn get_log_page(
    session_id: String,
    offset: u64,
    limit: u64,
    state: State<'_, BackendState>,
) -> Result<enginelog::LogPage, String> {
    dispatch!(
        state,
        local => queen_core::get_log_page(
            session_id.clone(),
            offset,
            limit,
            CoreStateRef::new(local),
        ).await,
        RunnerCommand::GetLogPage { session_id, offset, limit }
    )
}

#[tauri::command]
async fn get_log_outline(
    session_id: String,
    state: State<'_, BackendState>,
) -> Result<Vec<enginelog::LogSearchBlock>, String> {
    dispatch!(
        state,
        local => queen_core::get_log_outline(session_id.clone(), CoreStateRef::new(local)).await,
        RunnerCommand::GetLogOutline { session_id }
    )
}

#[tauri::command]
async fn search_log_session(
    session_id: String,
    query: enginelog::LogQuery,
    state: State<'_, BackendState>,
) -> Result<Vec<enginelog::LogMatch>, String> {
    dispatch!(
        state,
        local => queen_core::search_log_session(
            session_id.clone(),
            query.clone(),
            CoreStateRef::new(local),
        ).await,
        RunnerCommand::SearchLogSession { session_id, query }
    )
}

#[tauri::command]
async fn search_log_sessions(
    filter: enginelog::LogFilter,
    query: enginelog::LogQuery,
    state: State<'_, BackendState>,
) -> Result<Vec<enginelog::LogSessionMatches>, String> {
    dispatch!(
        state,
        local => queen_core::search_log_sessions(
            filter.clone(),
            query.clone(),
            CoreStateRef::new(local),
        ).await,
        RunnerCommand::SearchLogSessions { filter, query }
    )
}

#[tauri::command]
async fn export_log_session(
    session_id: String,
    path: String,
    mode: String,
    state: State<'_, BackendState>,
) -> Result<(), String> {
    let dispatch = state.dispatch_backend()?;
    match dispatch.target() {
        DispatchTarget::Embedded(local) => {
            queen_core::export_log_session(session_id, path, mode, CoreStateRef::new(local)).await
        }
        DispatchTarget::Remote(client) => {
            let bytes = client.log_export(&session_id, &mode).await?;
            tokio::fs::write(&path, bytes)
                .await
                .map_err(|error| format!("Could not save the runner log to {path}: {error}"))
        }
    }
}

#[tauri::command]
async fn delete_log_session(
    session_id: String,
    state: State<'_, BackendState>,
) -> Result<(), String> {
    dispatch!(
        state,
        local => queen_core::delete_log_session(session_id.clone(), CoreStateRef::new(local)).await,
        RunnerCommand::DeleteLogSession { session_id }
    )
}

#[tauri::command]
async fn clear_log_sessions(state: State<'_, BackendState>) -> Result<u64, String> {
    dispatch!(
        state,
        local => queen_core::clear_log_sessions(CoreStateRef::new(local)).await,
        RunnerCommand::ClearLogSessions
    )
}

#[tauri::command]
async fn get_logs_overview(
    state: State<'_, BackendState>,
) -> Result<enginelog::LogsOverview, String> {
    dispatch!(
        state,
        local => queen_core::get_logs_overview(CoreStateRef::new(local)).await,
        RunnerCommand::GetLogsOverview
    )
}

#[tauri::command]
async fn set_log_retention(
    retention: enginelog::LogRetention,
    state: State<'_, BackendState>,
) -> Result<(), String> {
    dispatch!(
        state,
        local => queen_core::set_log_retention(retention.clone(), CoreStateRef::new(local)).await,
        RunnerCommand::SetLogRetention { retention }
    )
}

#[tauri::command]
async fn get_diagnostics(
    filter: diagnostics::DiagnosticFilter,
    state: State<'_, BackendState>,
) -> Result<Vec<diagnostics::DiagnosticEntry>, String> {
    dispatch!(
        state,
        local => queen_core::get_diagnostics(filter.clone(), CoreStateRef::new(local)).await,
        RunnerCommand::GetDiagnostics { filter }
    )
}

#[tauri::command]
async fn clear_diagnostics(state: State<'_, BackendState>) -> Result<(), String> {
    dispatch!(
        state,
        local => queen_core::clear_diagnostics(CoreStateRef::new(local)).await,
        RunnerCommand::ClearDiagnostics
    )
}

#[tauri::command]
async fn confirm_close(window: tauri::Window) -> Result<(), String> {
    window
        .destroy()
        .map_err(|error| format!("Could not close the window: {error}"))
}

fn desktop_config_path(data_dir: &Path) -> PathBuf {
    data_dir.join(DESKTOP_CONFIG_FILE)
}

fn load_desktop_config(data_dir: &Path) -> Result<DesktopRunnerConfig, String> {
    let path = desktop_config_path(data_dir);
    if !path.exists() {
        return Ok(DesktopRunnerConfig::default());
    }
    let contents = std::fs::read_to_string(&path)
        .map_err(|error| format!("Could not read runner settings: {error}"))?;
    serde_json::from_str(&contents)
        .map_err(|error| format!("Could not parse runner settings: {error}"))
}

fn save_desktop_config(data_dir: &Path, config: &DesktopRunnerConfig) -> Result<(), String> {
    std::fs::create_dir_all(data_dir)
        .map_err(|error| format!("Could not create the desktop data directory: {error}"))?;
    let path = desktop_config_path(data_dir);
    let temporary = path.with_extension("json.tmp");
    let contents = serde_json::to_vec_pretty(config)
        .map_err(|error| format!("Could not encode runner settings: {error}"))?;
    std::fs::write(&temporary, contents)
        .map_err(|error| format!("Could not write runner settings: {error}"))?;
    std::fs::rename(&temporary, &path)
        .map_err(|error| format!("Could not replace runner settings: {error}"))
}

fn encode_runner_identity(identity: &RunnerIdentity) -> Result<String, String> {
    serde_json::to_string(identity)
        .map_err(|error| format!("Could not encode the runner identity: {error}"))
}

fn decode_runner_identity(encoded: &str) -> Result<RunnerIdentity, String> {
    let identity: RunnerIdentity = serde_json::from_str(encoded)
        .map_err(|_| "The saved runner identity is invalid; explicitly pair again".to_string())?;
    if identity.version != PAIRING_PAYLOAD_VERSION {
        return Err(
            "The saved runner identity version is unsupported; explicitly pair again".into(),
        );
    }
    Ok(identity)
}

#[cfg(windows)]
fn store_runner_identity(_data_dir: &Path, identity: &RunnerIdentity) -> Result<(), String> {
    let entry = keyring::v1::Entry::new("QueenUI Runner", RUNNER_IDENTITY_KEY)
        .map_err(|error| format!("Could not open Windows Credential Manager: {error}"))?;
    entry
        .set_password(&encode_runner_identity(identity)?)
        .map_err(|error| format!("Could not save the runner identity: {error}"))
}

#[cfg(windows)]
fn get_runner_identity(_data_dir: &Path) -> Result<RunnerIdentity, String> {
    let entry = keyring::v1::Entry::new("QueenUI Runner", RUNNER_IDENTITY_KEY)
        .map_err(|error| format!("Could not open Windows Credential Manager: {error}"))?;
    let encoded = entry
        .get_password()
        .map_err(|error| format!("Could not read the runner identity: {error}"))?;
    decode_runner_identity(&encoded)
}

#[cfg(windows)]
fn delete_runner_identity(_data_dir: &Path) -> Result<(), String> {
    delete_windows_credential(RUNNER_IDENTITY_KEY)
}

#[cfg(windows)]
fn delete_legacy_runner_credential(_data_dir: &Path) -> Result<(), String> {
    delete_windows_credential(LEGACY_RUNNER_TOKEN_KEY)
}

#[cfg(windows)]
fn delete_windows_credential(key: &str) -> Result<(), String> {
    let entry = keyring::v1::Entry::new("QueenUI Runner", key)
        .map_err(|error| format!("Could not open Windows Credential Manager: {error}"))?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(format!("Could not forget the runner credential: {error}")),
    }
}

#[cfg(windows)]
fn legacy_runner_url_hint(_data_dir: &Path) -> Option<String> {
    let entry = keyring::v1::Entry::new("QueenUI Runner", LEGACY_RUNNER_TOKEN_KEY).ok()?;
    let encoded = entry.get_password().ok()?;
    serde_json::from_str::<LegacyRunnerCredential>(&encoded)
        .ok()
        .map(|legacy| legacy.endpoint)
}

#[cfg(not(windows))]
fn store_runner_identity(data_dir: &Path, identity: &RunnerIdentity) -> Result<(), String> {
    use queen_core::storage::SecretStore;
    queen_core::storage::FileSecretStore::new(data_dir.join("desktop-secrets"))
        .store(RUNNER_IDENTITY_KEY, &encode_runner_identity(identity)?)
}

#[cfg(not(windows))]
fn get_runner_identity(data_dir: &Path) -> Result<RunnerIdentity, String> {
    use queen_core::storage::SecretStore;
    let encoded = queen_core::storage::FileSecretStore::new(data_dir.join("desktop-secrets"))
        .get(RUNNER_IDENTITY_KEY)?;
    decode_runner_identity(&encoded)
}

#[cfg(not(windows))]
fn delete_runner_identity(data_dir: &Path) -> Result<(), String> {
    use queen_core::storage::SecretStore;
    queen_core::storage::FileSecretStore::new(data_dir.join("desktop-secrets"))
        .delete(RUNNER_IDENTITY_KEY)
}

#[cfg(not(windows))]
fn delete_legacy_runner_credential(data_dir: &Path) -> Result<(), String> {
    use queen_core::storage::SecretStore;
    queen_core::storage::FileSecretStore::new(data_dir.join("desktop-secrets"))
        .delete(LEGACY_RUNNER_TOKEN_KEY)
}

#[cfg(not(windows))]
fn legacy_runner_url_hint(data_dir: &Path) -> Option<String> {
    let path = data_dir
        .join("desktop-secrets")
        .join(format!("{LEGACY_RUNNER_TOKEN_KEY}.token"));
    let encoded = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<LegacyRunnerCredential>(&encoded)
        .ok()
        .map(|legacy| legacy.endpoint)
}

fn settings_view(state: &RunnerSettingsState) -> RunnerSettingsView {
    let configured = state
        .configured
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let active = state
        .active
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let stored_identity = get_runner_identity(&state.data_dir).ok();
    let paired = stored_identity.is_some();
    RunnerSettingsView {
        restart_required: configured.mode != active.mode
            || (configured.mode == "remote"
                && (configured.url != active.url
                    || !active.available
                    || stored_identity.as_ref().map(|identity| identity.generation)
                        != active.identity_generation)),
        mode: configured.mode,
        url: configured.url,
        paired,
        active_mode: active.mode,
        source: state.source.clone(),
        allow_insecure_remote_http: false,
    }
}

fn runner_client_for_endpoint(
    identity: RunnerIdentity,
    requested_endpoint: &str,
) -> Result<RunnerClient, String> {
    let endpoint = RunnerClient::canonical_endpoint(requested_endpoint)?;
    if RunnerClient::canonical_endpoint(&identity.url)? != endpoint {
        return Err(
            "The saved runner identity belongs to a different endpoint; explicitly pair this URL"
                .into(),
        );
    }
    RunnerClient::from_identity(identity)
}

#[tauri::command]
fn get_runner_settings(state: State<'_, RunnerSettingsState>) -> RunnerSettingsView {
    settings_view(&state)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn set_runner_settings(
    mode: String,
    url: Option<String>,
    token: Option<String>,
    allow_insecure_remote_http: Option<bool>,
    acknowledged_runner: Option<String>,
    state: State<'_, RunnerSettingsState>,
    backend: State<'_, BackendState>,
    app: tauri::AppHandle,
) -> Result<RunnerSettingsView, String> {
    set_runner_settings_inner(
        mode,
        url,
        token,
        allow_insecure_remote_http,
        acknowledged_runner,
        state.inner(),
        backend.inner(),
        BackendEvents::for_app(app),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn set_runner_settings_inner(
    mode: String,
    url: Option<String>,
    token: Option<String>,
    allow_insecure_remote_http: Option<bool>,
    acknowledged_runner: Option<String>,
    state: &RunnerSettingsState,
    backend: &BackendState,
    events: BackendEvents,
) -> Result<RunnerSettingsView, String> {
    let _change_guard = state
        .change_gate
        .try_lock()
        .map_err(|_| SWITCHING_RUNNERS_ERROR.to_string())?;
    let mode = mode.trim().to_ascii_lowercase();
    let config = match mode.as_str() {
        "embedded" => DesktopRunnerConfig::default(),
        "remote" => {
            if allow_insecure_remote_http.unwrap_or(false) {
                return Err(
                    "Remote cleartext HTTP is no longer supported; pair with pinned HTTPS".into(),
                );
            }
            if token
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
            {
                return Err(
                    "Direct bearer entry is retired; use the one-time runner pairing flow".into(),
                );
            }
            let url = url
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| "Enter the runner URL".to_string())?;
            let endpoint = RunnerClient::canonical_endpoint(&url)?;
            DesktopRunnerConfig {
                mode,
                url: Some(endpoint),
                allow_insecure_remote_http: false,
            }
        }
        _ => return Err("Runner mode must be embedded or remote".into()),
    };

    let active = state
        .active
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let remote_target = config.url.as_deref().map(|endpoint| {
        get_runner_identity(&state.data_dir).and_then(|identity| {
            let generation = identity.generation;
            runner_client_for_endpoint(identity, endpoint).map(|client| (client, generation))
        })
    });
    let target_generation = remote_target
        .as_ref()
        .and_then(|target| target.as_ref().ok())
        .map(|(_, generation)| *generation);
    let switch_required = !backend.is_active()
        || !active.available
        || remote_target.as_ref().is_some_and(Result::is_err)
        || config.mode != active.mode
        || (config.mode == "remote"
            && (config.url != active.url || target_generation != active.identity_generation));

    if !switch_required {
        save_and_publish_config(state, &config)?;
        if config.mode == "embedded" {
            delete_credentials_after_embedded_switch(state)?;
        }
        return Ok(settings_view(state));
    }

    let mut switching = backend.begin_settings_switch(state).await?;
    if let PreviousBackend::Active(ActiveBackend::Remote(remote)) = switching.previous()? {
        let leaves_current_runner =
            config.mode == "embedded" || config.url.as_deref() != Some(remote.client.base_url());
        if leaves_current_runner {
            verify_remote_handover(remote, acknowledged_runner.as_deref()).await?;
        }
    }
    let embedded_core = match switching.previous()? {
        PreviousBackend::Active(ActiveBackend::Embedded(embedded)) => Some(embedded.core.clone()),
        PreviousBackend::Active(ActiveBackend::Remote(_)) | PreviousBackend::Unavailable(_) => None,
    };
    let mut core_quiesce = if config.mode == "remote" {
        if let Some(core) = embedded_core.as_ref() {
            let quiesce = core.quiesce().await;
            let count = quiesce.live_game_count().await;
            if count > 0 {
                let error = live_games_switch_error(count);
                quiesce.restore().await;
                return Err(error);
            }
            if let Some(error) = quiesce
                .locally_unverifiable_outgoing_challenge_error()
                .await
            {
                quiesce.restore().await;
                return Err(error);
            }
            if let Err(error) = quiesce.verify_authoritative_handover().await {
                quiesce.restore().await;
                return Err(error);
            }
            Some(quiesce)
        } else {
            None
        }
    } else {
        None
    };

    let remote_target = match remote_target {
        Some(Ok(target)) => Some(target),
        Some(Err(error)) => {
            return refuse_after_quiesce(core_quiesce.take(), error).await;
        }
        None => None,
    };

    if let Err(error) = save_and_publish_config(state, &config) {
        return refuse_after_quiesce(core_quiesce.take(), error).await;
    }

    if config.mode == "remote" {
        let (client, identity_generation) = match remote_target {
            Some(target) => target,
            None => {
                return refuse_after_quiesce(
                    core_quiesce.take(),
                    "Remote runner settings did not include a validated identity".to_string(),
                )
                .await;
            }
        };
        let previous = match switching.take_previous() {
            Ok(previous) => previous,
            Err(error) => return refuse_after_quiesce(core_quiesce.take(), error).await,
        };
        match previous {
            PreviousBackend::Active(ActiveBackend::Embedded(embedded)) => {
                let shutdown = core_quiesce
                    .take()
                    .expect("embedded-to-remote switches quiesce before draining")
                    .shutdown()
                    .await;
                if let Err(error) = shutdown {
                    diagnostics::record(
                        diagnostics::DiagnosticEntry::error(
                            "runner",
                            "Embedded runner drain failed; live switch continued",
                        )
                        .with_detail(error),
                    );
                }
                let EmbeddedBackend {
                    backend_generation: _,
                    core,
                    event_cancellation,
                    event_task,
                } = embedded;
                event_cancellation.cancel();
                let _ = event_task.await;
                drop(core);
            }
            PreviousBackend::Active(ActiveBackend::Remote(remote)) => {
                drop(core_quiesce);
                let RemoteBackend {
                    backend_generation: _,
                    client,
                    event_cancellation,
                    event_task,
                } = remote;
                event_cancellation.cancel();
                let _ = event_task.await;
                drop(client);
            }
            PreviousBackend::Unavailable(_) => drop(core_quiesce),
        }
        let backend_generation = backend.next_generation();
        let remote = RemoteBackend::new_at_generation(client, events.clone(), backend_generation);
        switching.finish(ActiveBackend::Remote(remote));
        set_active_runner(
            state,
            ActiveRunner {
                mode: "remote".into(),
                url: config.url.clone(),
                available: true,
                identity_generation: Some(identity_generation),
            },
        );
        return Ok(settings_view(state));
    }

    let recovery = prepare_embedded_load_recovery(switching.take_previous()?).await;
    match AppState::load(state.data_dir.clone()) {
        Ok(core) => {
            drop(recovery);
            let backend_generation = backend.next_generation();
            let embedded =
                EmbeddedBackend::new_at_generation(core, events.clone(), backend_generation);
            let refresh_core = embedded.core.clone();
            switching.finish(ActiveBackend::Embedded(embedded));
            set_active_runner(
                state,
                ActiveRunner {
                    mode: "embedded".into(),
                    url: None,
                    available: true,
                    identity_generation: None,
                },
            );
            // Truthful same-generation data lands before the connection state
            // can clear frontend staleness for the new embedded backend.
            events.emit_core(
                backend_generation,
                CoreEvent::Snapshot(refresh_core.snapshot().await),
            );
            events.emit_connection(embedded_connection_event(backend_generation));
            delete_credentials_after_embedded_switch(state)?;
        }
        Err(error) => {
            switching.finish_previous(restore_after_failed_embedded_load(recovery, events));
            return Err(saved_switch_error(error));
        }
    }
    Ok(settings_view(state))
}

fn save_and_publish_config(
    state: &RunnerSettingsState,
    config: &DesktopRunnerConfig,
) -> Result<(), String> {
    save_desktop_config(&state.data_dir, config)?;
    *state
        .configured
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = config.clone();
    Ok(())
}

fn set_active_runner(state: &RunnerSettingsState, active: ActiveRunner) {
    *state
        .active
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = active;
}

fn live_games_switch_error(count: usize) -> String {
    let noun = if count == 1 { "game is" } else { "games are" };
    format!(
        "{count} {noun} still being played from this computer; finish or resign them before switching to a runner."
    )
}

async fn refuse_after_quiesce<T>(
    quiesce: Option<queen_core::CoreQuiesceGuard>,
    error: String,
) -> Result<T, String> {
    if let Some(quiesce) = quiesce {
        quiesce.restore().await;
    }
    Err(error)
}

async fn verify_remote_handover(
    remote: &RemoteBackend,
    acknowledged_runner: Option<&str>,
) -> Result<(), String> {
    let acknowledged = acknowledged_runner == Some(remote.client.base_url());
    match remote.client.handover_inventory().await {
        Ok(inventory) => {
            if inventory.live_games > 0 && inventory.outgoing_challenges == 0 && !acknowledged {
                let noun = if inventory.live_games == 1 {
                    "game"
                } else {
                    "games"
                };
                let pronoun = if inventory.live_games == 1 {
                    "it"
                } else {
                    "they"
                };
                return Err(format!(
                    "The remote runner at {} is still playing {} {noun}. Confirm that {pronoun} will keep playing there before switching runners.",
                    remote.client.base_url(),
                    inventory.live_games,
                ));
            }
            if inventory.outgoing_challenges > 0 && !acknowledged {
                let challenge_noun = if inventory.outgoing_challenges == 1 {
                    "challenge"
                } else {
                    "challenges"
                };
                if inventory.live_games == 0 {
                    let pronoun = if inventory.outgoing_challenges == 1 {
                        "it"
                    } else {
                        "they"
                    };
                    return Err(format!(
                        "The remote runner at {} still owns {} outgoing {challenge_noun}. Confirm that {pronoun} will remain there before switching runners.",
                        remote.client.base_url(),
                        inventory.outgoing_challenges,
                    ));
                }
                let game_noun = if inventory.live_games == 1 {
                    "game"
                } else {
                    "games"
                };
                return Err(format!(
                    "The remote runner at {} is still playing {} {game_noun} and owns {} outgoing {challenge_noun}. Confirm that this work will remain there before switching runners.",
                    remote.client.base_url(),
                    inventory.live_games,
                    inventory.outgoing_challenges,
                ));
            }
        }
        Err(_) if !acknowledged => {
            return Err(format!(
                "Could not verify the remote runner at {}; it may still be playing games. Confirm that its games will keep running there before switching runners.",
                remote.client.base_url()
            ));
        }
        Err(_) => {}
    }
    Ok(())
}

fn saved_switch_error(error: String) -> String {
    format!(
        "Runner settings were saved, but the switch could not complete; restarting QueenUI will retry it: {error}"
    )
}

fn delete_credentials_after_embedded_switch(state: &RunnerSettingsState) -> Result<(), String> {
    delete_runner_credentials(&state.data_dir).map_err(|error| {
        format!(
            "Runner mode is embedded, but the stored pairing record could not be removed: {error}. Use ‘Forget the paired runner’ to retry the deletion."
        )
    })
}

async fn prepare_embedded_load_recovery(previous: PreviousBackend) -> EmbeddedLoadRecovery {
    match previous {
        PreviousBackend::Active(ActiveBackend::Remote(remote)) => {
            let RemoteBackend {
                backend_generation,
                client,
                event_cancellation,
                event_task,
            } = remote;
            event_cancellation.cancel();
            let _ = event_task.await;
            // Retaining the proven in-memory client is the strongest rollback:
            // the stored identity may have been forgotten or replaced.
            EmbeddedLoadRecovery::RemoteClient {
                client,
                backend_generation,
            }
        }
        PreviousBackend::Unavailable(error) => EmbeddedLoadRecovery::Unavailable(error),
        PreviousBackend::Active(ActiveBackend::Embedded(embedded)) => {
            EmbeddedLoadRecovery::Embedded(embedded)
        }
    }
}

fn restore_after_failed_embedded_load(
    recovery: EmbeddedLoadRecovery,
    events: BackendEvents,
) -> PreviousBackend {
    match recovery {
        EmbeddedLoadRecovery::RemoteClient {
            client,
            backend_generation,
        } => {
            let replacement = RemoteBackend::new_at_generation(client, events, backend_generation);
            PreviousBackend::Active(ActiveBackend::Remote(replacement))
        }
        EmbeddedLoadRecovery::Unavailable(error) => PreviousBackend::Unavailable(error),
        EmbeddedLoadRecovery::Embedded(embedded) => {
            PreviousBackend::Active(ActiveBackend::Embedded(embedded))
        }
    }
}

#[tauri::command]
async fn test_runner_connection(
    url: String,
    token: Option<String>,
    allow_insecure_remote_http: Option<bool>,
    state: State<'_, RunnerSettingsState>,
) -> Result<RunnerConnectionTest, String> {
    if allow_insecure_remote_http.unwrap_or(false) {
        return Err("Remote cleartext HTTP is no longer supported".into());
    }
    if token
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Err("Direct bearer entry is retired; explicitly pair the runner".into());
    }
    let endpoint = RunnerClient::canonical_endpoint(&url)?;
    let identity = get_runner_identity(&state.data_dir)?;
    runner_connection_test(&runner_client_for_endpoint(identity, &endpoint)?).await
}

#[tauri::command]
fn forget_runner_credential(state: State<'_, RunnerSettingsState>) -> Result<(), String> {
    forget_runner_credential_inner(&state)
}

fn forget_runner_credential_inner(state: &RunnerSettingsState) -> Result<(), String> {
    let _change_guard = state
        .change_gate
        .try_lock()
        .map_err(|_| SWITCHING_RUNNERS_ERROR.to_string())?;
    delete_runner_credentials(&state.data_dir)
}

fn delete_runner_credentials(data_dir: &Path) -> Result<(), String> {
    delete_runner_identity(data_dir)?;
    delete_legacy_runner_credential(data_dir)
}

#[tauri::command]
async fn pair_runner_from_payload(
    payload: String,
    state: State<'_, RunnerSettingsState>,
    backend: State<'_, BackendState>,
    app: tauri::AppHandle,
) -> Result<RunnerConnectionTest, String> {
    pair_and_store(
        &payload,
        &state,
        &backend,
        BackendEvents::for_app(app),
        redeem_runner_pairing,
    )
    .await
}

#[tauri::command]
async fn pair_runner_via_ssh(
    alias: String,
    state: State<'_, RunnerSettingsState>,
    backend: State<'_, BackendState>,
    app: tauri::AppHandle,
) -> Result<RunnerConnectionTest, String> {
    pair_and_store(
        &alias,
        &state,
        &backend,
        BackendEvents::for_app(app),
        |alias| async move {
            let payload = fetch_pairing_payload_over_ssh(&alias).await?;
            redeem_runner_pairing(payload).await
        },
    )
    .await
}

async fn redeem_runner_pairing(payload: String) -> Result<RunnerIdentity, String> {
    redeem_pairing_payload(&payload).await
}

async fn pair_and_store<R, F>(
    payload: &str,
    state: &RunnerSettingsState,
    backend: &BackendState,
    events: BackendEvents,
    redeem: R,
) -> Result<RunnerConnectionTest, String>
where
    R: FnOnce(String) -> F,
    F: std::future::Future<Output = Result<RunnerIdentity, String>>,
{
    let _change_guard = state
        .change_gate
        .try_lock()
        .map_err(|_| SWITCHING_RUNNERS_ERROR.to_string())?;
    let identity = redeem(payload.to_string()).await?;
    // Persist immediately: on rotation the old bearer is already dead when
    // redeem commits, even if the following capability probe is interrupted.
    store_runner_identity(&state.data_dir, &identity)?;
    // The unauthenticated legacy bearer stays inert until this replacement is
    // stored, then is removed even if a later capability probe is interrupted.
    delete_legacy_runner_credential(&state.data_dir)?;
    let client = RunnerClient::from_identity(identity.clone())?;
    let test = runner_connection_test(&client).await?;
    let config = DesktopRunnerConfig {
        mode: "remote".into(),
        url: Some(identity.url.clone()),
        allow_insecure_remote_http: false,
    };
    let active = state
        .active
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let adopt_live = active.mode == "remote"
        && active.url.as_deref().is_some_and(|url| {
            matches!(
                (
                    RunnerClient::canonical_endpoint(url),
                    RunnerClient::canonical_endpoint(&identity.url),
                ),
                (Ok(active), Ok(paired)) if active == paired
            )
        });
    if adopt_live {
        // Same-endpoint pairing adopts the rotated bearer immediately. It is
        // a backend publication, but it does not leave the runner that owns
        // any remote games.
        let mut switching = backend.begin_settings_switch(state).await?;
        save_and_publish_config(state, &config)?;
        match switching.take_previous()? {
            PreviousBackend::Active(ActiveBackend::Remote(remote)) => {
                remote.event_cancellation.cancel();
                let _ = remote.event_task.await;
                drop(remote.client);
            }
            PreviousBackend::Unavailable(_) => {}
            previous @ PreviousBackend::Active(ActiveBackend::Embedded(_)) => {
                switching.finish_previous(previous);
                return Err(
                    "The active runner changed while pairing; save runner settings to adopt the new identity"
                        .into(),
                );
            }
        }
        let generation = identity.generation;
        let backend_generation = backend.next_generation();
        switching.finish(ActiveBackend::Remote(RemoteBackend::new_at_generation(
            client,
            events,
            backend_generation,
        )));
        set_active_runner(
            state,
            ActiveRunner {
                mode: "remote".into(),
                url: config.url.clone(),
                available: true,
                identity_generation: Some(generation),
            },
        );
    } else {
        save_and_publish_config(state, &config)?;
    }
    Ok(test)
}

async fn runner_connection_test(client: &RunnerClient) -> Result<RunnerConnectionTest, String> {
    let capabilities = client.capabilities().await?;
    Ok(RunnerConnectionTest {
        hostname: capabilities.hostname,
        operating_system: capabilities.operating_system,
        architecture: capabilities.architecture,
        logical_cpus: capabilities.logical_cpus,
    })
}

fn validate_ssh_alias(alias: &str) -> Result<&str, String> {
    if alias.is_empty()
        || alias.len() > 255
        || alias.starts_with('-')
        || alias.contains('=')
        || alias.chars().any(char::is_whitespace)
        || alias.chars().any(char::is_control)
    {
        return Err("SSH alias must be a host alias, not an option or command".into());
    }
    Ok(alias)
}

#[cfg(windows)]
fn trusted_ssh_binary() -> Result<PathBuf, String> {
    let windows = std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .ok_or_else(|| "Could not locate the Windows directory".to_string())?;
    let path = windows.join("System32").join("OpenSSH").join("ssh.exe");
    path.canonicalize()
        .map_err(|_| "Windows OpenSSH is not installed in System32".to_string())
}

#[cfg(unix)]
fn trusted_ssh_binary() -> Result<PathBuf, String> {
    use std::os::unix::fs::MetadataExt;
    for candidate in [Path::new("/usr/bin/ssh"), Path::new("/bin/ssh")] {
        let Ok(path) = candidate.canonicalize() else {
            continue;
        };
        let metadata = std::fs::metadata(&path)
            .map_err(|error| format!("Could not inspect the trusted SSH binary: {error}"))?;
        if metadata.is_file() && metadata.uid() == 0 && metadata.mode() & 0o022 == 0 {
            return Ok(path);
        }
    }
    Err("Could not locate a root-owned, non-writable OpenSSH binary".into())
}

async fn fetch_pairing_payload_over_ssh(alias: &str) -> Result<String, String> {
    use std::process::Stdio;
    use tokio::io::AsyncReadExt;

    const OUTPUT_LIMIT: u64 = 16 * 1024;
    let alias = validate_ssh_alias(alias)?;
    let mut child = tokio::process::Command::new(trusted_ssh_binary()?)
        .args([
            "-oBatchMode=yes",
            "-oStrictHostKeyChecking=yes",
            "-oConnectTimeout=10",
            "-oNumberOfPasswordPrompts=0",
            "-oLogLevel=ERROR",
            "--",
        ])
        .arg(alias)
        .args(["queen-runner", "pair", "--print"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("Could not start trusted OpenSSH: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Could not capture OpenSSH output".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Could not capture OpenSSH errors".to_string())?;
    let stdout_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stdout
            .take(OUTPUT_LIMIT + 1)
            .read_to_end(&mut bytes)
            .await?;
        Ok::<_, std::io::Error>(bytes)
    });
    let stderr_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stderr
            .take(OUTPUT_LIMIT + 1)
            .read_to_end(&mut bytes)
            .await?;
        Ok::<_, std::io::Error>(bytes)
    });
    let status = match tokio::time::timeout(std::time::Duration::from_secs(20), child.wait()).await
    {
        Ok(result) => result.map_err(|error| format!("Could not wait for OpenSSH: {error}"))?,
        Err(_) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err("OpenSSH pairing timed out after 20 seconds".into());
        }
    };
    let stdout = stdout_task
        .await
        .map_err(|_| "OpenSSH output reader stopped unexpectedly".to_string())?
        .map_err(|error| format!("Could not read OpenSSH output: {error}"))?;
    let stderr = stderr_task
        .await
        .map_err(|_| "OpenSSH error reader stopped unexpectedly".to_string())?
        .map_err(|error| format!("Could not read OpenSSH errors: {error}"))?;
    if stdout.len() > OUTPUT_LIMIT as usize || stderr.len() > OUTPUT_LIMIT as usize {
        return Err("OpenSSH pairing output exceeded 16 KiB".into());
    }
    if !status.success() {
        return Err(
            "OpenSSH pairing failed. Verify the saved host key and runner admin command.".into(),
        );
    }
    let payload = String::from_utf8(stdout)
        .map_err(|_| "OpenSSH pairing output was not UTF-8".to_string())?;
    let payload = payload.trim();
    if payload.lines().count() != 1 {
        return Err("OpenSSH pairing output must contain exactly one payload".into());
    }
    Ok(payload.to_string())
}

fn emit_core_event(handle: &tauri::AppHandle, event: BackendCoreEvent) {
    let BackendCoreEvent {
        backend_generation,
        event,
    } = event;
    match event {
        CoreEvent::Snapshot(snapshot) => {
            let _ = handle.emit(
                SNAPSHOT_EVENT,
                BackendSnapshotEvent {
                    backend_generation,
                    payload: snapshot,
                },
            );
        }
        CoreEvent::LogsUpdated => {
            let _ = handle.emit(
                LOGS_UPDATED_EVENT,
                BackendNotificationEvent {
                    backend_generation,
                    payload: (),
                },
            );
        }
        CoreEvent::HistoryUpdated => {
            let _ = handle.emit(
                HISTORY_UPDATED_EVENT,
                BackendNotificationEvent {
                    backend_generation,
                    payload: (),
                },
            );
        }
    }
}

fn forward_embedded_events(
    events: BackendEvents,
    state: &AppState,
    backend_generation: u64,
    cancellation: CancellationToken,
) -> tauri::async_runtime::JoinHandle<()> {
    let sink = events;
    let mut subscription = state.subscribe();
    tauri::async_runtime::spawn(async move {
        loop {
            let event = tokio::select! {
                _ = cancellation.cancelled() => return,
                event = subscription.recv() => event,
            };
            match event {
                Ok(event) => sink.emit_core(backend_generation, event),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    })
}

fn forward_remote_events(
    events: BackendEvents,
    client: RunnerClient,
    backend_generation: u64,
    cancellation: CancellationToken,
) -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        let connection_events = events.clone();
        let core_events = events;
        run_remote_event_loop(
            client,
            cancellation,
            REMOTE_EVENT_HEARTBEAT,
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(20),
            backend_generation,
            move |event| connection_events.emit_connection(event),
            move |event| {
                let event = match event {
                    CoreEvent::Snapshot(snapshot) => {
                        CoreEvent::Snapshot(redact_remote_snapshot(snapshot))
                    }
                    other => other,
                };
                core_events.emit_core(backend_generation, event);
            },
        )
        .await;
    })
}

fn redact_remote_snapshot(mut snapshot: AppSnapshot) -> AppSnapshot {
    for engine in &mut snapshot.engines {
        let path = Path::new(&engine.path);
        let Some(identity) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if path
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|directory| directory == "engine-store")
            && identity.len() == 64
            && identity.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            engine.path = format!("engine-store/{identity}");
        }
    }
    snapshot
}

#[allow(clippy::too_many_arguments)]
async fn run_remote_event_loop<C, E>(
    client: RunnerClient,
    cancellation: CancellationToken,
    heartbeat_interval: std::time::Duration,
    initial_retry: std::time::Duration,
    maximum_retry: std::time::Duration,
    backend_generation: u64,
    mut emit_connection: C,
    mut emit_core: E,
) where
    C: FnMut(RunnerConnectionEvent),
    E: FnMut(CoreEvent),
{
    let mut retry = initial_retry;
    let mut attempt = 0u32;
    let mut last_ok_ts = None;
    loop {
        attempt = attempt.saturating_add(1);
        emit_connection(RunnerConnectionEvent {
            backend_generation,
            state: RunnerConnectionState::Reconnecting,
            attempt,
            last_ok_ts,
            detail: None,
        });
        let connection = client.events();
        tokio::pin!(connection);
        let mut heartbeat = tokio::time::interval_at(
            tokio::time::Instant::now() + heartbeat_interval,
            heartbeat_interval,
        );
        let connection = loop {
            tokio::select! {
                _ = cancellation.cancelled() => return,
                result = &mut connection => break result,
                _ = heartbeat.tick() => emit_connection(RunnerConnectionEvent {
                    backend_generation,
                    state: RunnerConnectionState::Reconnecting,
                    attempt,
                    last_ok_ts,
                    detail: Some("Waiting for the runner event connection".into()),
                }),
            }
        };
        let disconnect_detail = match connection {
            Ok(mut events) => {
                retry = initial_retry;
                last_ok_ts = Some(epoch_millis());
                emit_connection(RunnerConnectionEvent {
                    backend_generation,
                    state: RunnerConnectionState::Connected,
                    attempt,
                    last_ok_ts,
                    detail: None,
                });
                loop {
                    let next = tokio::select! {
                        _ = cancellation.cancelled() => return,
                        next = events.next() => next,
                    };
                    match next {
                        Ok(Some(envelope)) => {
                            last_ok_ts = Some(epoch_millis());
                            emit_core(envelope.event);
                        }
                        Ok(None) => break "Runner closed the event stream".to_string(),
                        Err(error) => break error,
                    }
                }
            }
            Err(error) => error,
        };
        diagnostics::record(
            diagnostics::DiagnosticEntry::warn("runner", "Remote runner event connection failed")
                .with_detail(disconnect_detail.clone()),
        );
        emit_connection(RunnerConnectionEvent {
            backend_generation,
            state: RunnerConnectionState::Disconnected,
            attempt,
            last_ok_ts,
            detail: Some(disconnect_detail.clone()),
        });
        let deadline = tokio::time::Instant::now() + retry;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            tokio::select! {
                _ = cancellation.cancelled() => return,
                _ = tokio::time::sleep(remaining.min(heartbeat_interval)) => {}
            }
            if tokio::time::Instant::now() < deadline {
                emit_connection(RunnerConnectionEvent {
                    backend_generation,
                    state: RunnerConnectionState::Reconnecting,
                    attempt,
                    last_ok_ts,
                    detail: Some(disconnect_detail.clone()),
                });
            }
        }
        retry = (retry * 2).min(maximum_retry);
    }
}

fn emit_runner_connection(handle: &tauri::AppHandle, event: RunnerConnectionEvent) {
    let _ = handle.emit(RUNNER_CONNECTION_EVENT, event);
}

fn embedded_connection_event(backend_generation: u64) -> RunnerConnectionEvent {
    RunnerConnectionEvent {
        backend_generation,
        state: RunnerConnectionState::Embedded,
        attempt: 0,
        last_ok_ts: None,
        detail: None,
    }
}

fn epoch_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let result = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let app_data_dir = app.path().app_data_dir()?;
            let mut configured =
                load_desktop_config(&app_data_dir).map_err(std::io::Error::other)?;
            if configured.url.is_none() {
                // The legacy record supplies only an untrusted UI hint. Its
                // bearer is never decoded into a RunnerClient or sent.
                configured.url = legacy_runner_url_hint(&app_data_dir);
            }
            let events = BackendEvents::for_app(app.handle().clone());
            let (state, active_mode, active_url, active_available, active_generation, source) =
                if configured.mode == "remote" {
                    let client = get_runner_identity(&app_data_dir).and_then(|identity| {
                        let configured_url = configured.url.as_deref().ok_or_else(|| {
                            "Remote runner URL is missing; explicitly pair again".to_string()
                        })?;
                        let generation = identity.generation;
                        runner_client_for_endpoint(identity, configured_url)
                            .map(|client| (client, generation))
                    });
                    match client {
                        Ok((client, generation)) => {
                            let active_url = Some(client.base_url().to_string());
                            let remote = RemoteBackend::new(client, events.clone());
                            (
                                BackendState::active(ActiveBackend::Remote(remote)),
                                "remote".to_string(),
                                active_url,
                                true,
                                Some(generation),
                                "saved".to_string(),
                            )
                        }
                        Err(error) => (
                            BackendState::unavailable(format!(
                                "The configured remote runner is unavailable: {error}"
                            )),
                            "remote".to_string(),
                            configured.url.clone(),
                            false,
                            None,
                            "saved".to_string(),
                        ),
                    }
                } else {
                    let core =
                        AppState::load(app_data_dir.clone()).map_err(std::io::Error::other)?;
                    let embedded = EmbeddedBackend::new(core, events.clone());
                    events.emit_connection(embedded_connection_event(1));
                    (
                        BackendState::active(ActiveBackend::Embedded(embedded)),
                        "embedded".to_string(),
                        None,
                        true,
                        None,
                        "saved".to_string(),
                    )
                };
            // Embedded AppState acquires the data-directory authority lock
            // before installing diagnostics. Only install here as a fallback
            // for remote mode, which owns no local automation state.
            let diagnostics = diagnostics::global().unwrap_or_else(|| {
                diagnostics::install(diagnostics::DiagnosticsLog::load(&app_data_dir))
            });
            let diagnostic_handle = app.handle().clone();
            diagnostics.set_observer(Box::new(move |entry| {
                let _ = diagnostic_handle.emit(DIAGNOSTIC_EVENT, entry);
            }));
            app.manage(RunnerSettingsState {
                data_dir: app_data_dir,
                configured: std::sync::RwLock::new(configured),
                active: std::sync::RwLock::new(ActiveRunner {
                    mode: active_mode,
                    url: active_url,
                    available: active_available,
                    identity_generation: active_generation,
                }),
                change_gate: tokio::sync::Mutex::new(()),
                source,
            });
            app.manage(state);
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let live = window
                    .app_handle()
                    .try_state::<BackendState>()
                    .map(|state| state.live_game_count())
                    .unwrap_or(0);
                if live > 0 {
                    api.prevent_close();
                    let _ = window.emit(
                        CLOSE_REQUESTED_EVENT,
                        CloseRequestedPayload {
                            reported_count: live,
                        },
                    );
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_runner_settings,
            set_runner_settings,
            test_runner_connection,
            forget_runner_credential,
            pair_runner_from_payload,
            pair_runner_via_ssh,
            get_snapshot,
            write_pgn_file,
            add_engine,
            list_engine_roots,
            browse_engine_root,
            register_engine,
            remove_engine,
            update_engine_options,
            refresh_engine_options,
            configure_opening_book,
            clear_engine_opening_book,
            add_lichess_account,
            update_lichess_account_token,
            update_account_engine,
            remove_lichess_account,
            dismiss_game_error,
            start_bot,
            stop_bot,
            start_campaign,
            stop_campaign,
            create_challenge,
            get_scorebook_stats,
            import_lichess_history,
            list_log_sessions,
            get_log_page,
            get_log_outline,
            search_log_session,
            search_log_sessions,
            export_log_session,
            delete_log_session,
            clear_log_sessions,
            get_logs_overview,
            set_log_retention,
            get_diagnostics,
            clear_diagnostics,
            confirm_close,
        ])
        .run(tauri::generate_context!());
    if let Err(error) = result {
        eprintln!("QueenUI stopped because the Tauri runtime failed: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::{
        add_lichess_account_inner, decode_runner_identity, desktop_config_path,
        dismiss_game_error_inner, embedded_connection_event, encode_runner_identity,
        forget_runner_credential_inner, get_runner_identity, get_snapshot_inner,
        legacy_runner_url_hint, load_desktop_config, operator_safe_snapshot_error, pair_and_store,
        redact_remote_snapshot, run_remote_event_loop, runner_client_for_endpoint,
        save_desktop_config, set_runner_settings_inner, settings_view, store_runner_identity,
        update_lichess_account_token_inner, validate_ssh_alias, verify_remote_handover,
        ActiveBackend, ActiveRunner, BackendEvents, BackendNotificationEvent, BackendSnapshotEvent,
        BackendState, CloseRequestedPayload, DesktopRunnerConfig, EmbeddedBackend, PreviousBackend,
        RemoteBackend, RunnerConnectionEvent, RunnerConnectionState, RunnerConnectionTest,
        RunnerSettingsState, RunnerSettingsView, INTERRUPTED_SWITCH_ERROR, LEGACY_RUNNER_TOKEN_KEY,
        REMOTE_EVENT_HEARTBEAT, RUNNER_CONNECTION_EVENT, RUNNER_IDENTITY_KEY,
    };
    use axum::{
        extract::{ws::Message, ws::WebSocketUpgrade, State},
        http::{HeaderMap, StatusCode},
        response::{IntoResponse, Response},
        routing::{get, post},
        Json, Router,
    };
    use queen_core::{diagnostics, enginelog, history, models, AppState, CoreEvent};
    use queen_protocol::{
        CommandRequest, CommandResponse, EventEnvelope, HandoverInventory, RunnerCapabilities,
        RunnerCommand, RunnerIdentity, SnapshotResponse, PAIRING_PAYLOAD_VERSION, PROTOCOL_VERSION,
    };
    use std::{
        fs,
        path::PathBuf,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc, Mutex,
        },
        time::Duration,
    };
    use tokio_util::sync::CancellationToken;
    use ts_rs::TS;

    fn settings_state(
        data_dir: PathBuf,
        configured: DesktopRunnerConfig,
        active_mode: &str,
        active_url: Option<String>,
    ) -> RunnerSettingsState {
        let identity_generation = if active_mode == "remote" {
            get_runner_identity(&data_dir)
                .ok()
                .map(|identity| identity.generation)
        } else {
            None
        };
        RunnerSettingsState {
            data_dir,
            configured: std::sync::RwLock::new(configured),
            active: std::sync::RwLock::new(ActiveRunner {
                mode: active_mode.into(),
                url: active_url,
                available: true,
                identity_generation,
            }),
            change_gate: tokio::sync::Mutex::new(()),
            source: "settings".into(),
        }
    }

    fn remote_backend(identity: RunnerIdentity) -> BackendState {
        BackendState::active(ActiveBackend::Remote(RemoteBackend::new(
            queen_client::RunnerClient::from_identity(identity).unwrap(),
            BackendEvents::noop(),
        )))
    }

    #[derive(Clone)]
    struct EventScript {
        attempts: Arc<AtomicUsize>,
        release: CancellationToken,
    }

    #[cfg(not(windows))]
    #[derive(Clone)]
    struct PairingCapabilityState {
        bearer: Arc<Mutex<String>>,
    }

    #[derive(Clone, Default)]
    struct CommandScript {
        calls: Arc<AtomicUsize>,
        result: Arc<Mutex<Option<serde_json::Value>>>,
        last_command: Arc<Mutex<Option<serde_json::Value>>>,
    }

    #[cfg(not(windows))]
    #[derive(Clone)]
    struct SnapshotScript {
        calls: Arc<AtomicUsize>,
        snapshot: models::AppSnapshot,
        inventory: HandoverInventory,
    }

    #[cfg(not(windows))]
    async fn authenticated_capabilities(
        State(state): State<PairingCapabilityState>,
        headers: HeaderMap,
    ) -> Response {
        let expected = format!("Bearer {}", state.bearer.lock().unwrap());
        if headers
            .get(reqwest::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            != Some(expected.as_str())
        {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        Json(RunnerCapabilities {
            protocol_version: PROTOCOL_VERSION,
            instance_id: uuid::Uuid::nil(),
            hostname: "paired-runner".into(),
            operating_system: "test".into(),
            architecture: "test".into(),
            logical_cpus: 1,
        })
        .into_response()
    }

    #[cfg(not(windows))]
    async fn authenticated_command(
        State(state): State<PairingCapabilityState>,
        headers: HeaderMap,
        Json(request): Json<CommandRequest>,
    ) -> Response {
        let expected = format!("Bearer {}", state.bearer.lock().unwrap());
        if headers
            .get(reqwest::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            != Some(expected.as_str())
        {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        Json(CommandResponse::success(
            request.request_id,
            serde_json::Value::Null,
        ))
        .into_response()
    }

    async fn scripted_command(
        State(script): State<CommandScript>,
        Json(request): Json<CommandRequest>,
    ) -> Json<CommandResponse> {
        script.calls.fetch_add(1, Ordering::SeqCst);
        *script.last_command.lock().unwrap() =
            Some(serde_json::to_value(&request.command).unwrap());
        let result = if matches!(request.command, RunnerCommand::HandoverInventory) {
            serde_json::to_value(HandoverInventory {
                live_games: 0,
                outgoing_challenges: 0,
            })
            .unwrap()
        } else {
            script
                .result
                .lock()
                .unwrap()
                .clone()
                .unwrap_or(serde_json::Value::Null)
        };
        Json(CommandResponse::success(request.request_id, result))
    }

    async fn empty_snapshot() -> Json<SnapshotResponse> {
        Json(SnapshotResponse {
            protocol_version: PROTOCOL_VERSION,
            instance_id: uuid::Uuid::nil(),
            snapshot: models::AppSnapshot::default(),
        })
    }

    async fn reject_additive_command() -> StatusCode {
        StatusCode::BAD_REQUEST
    }

    #[cfg(not(windows))]
    async fn scripted_snapshot(State(script): State<SnapshotScript>) -> Json<SnapshotResponse> {
        Json(SnapshotResponse {
            protocol_version: PROTOCOL_VERSION,
            instance_id: uuid::Uuid::nil(),
            snapshot: script.snapshot,
        })
    }

    #[cfg(not(windows))]
    async fn scripted_inventory(
        State(script): State<SnapshotScript>,
        Json(request): Json<CommandRequest>,
    ) -> Json<CommandResponse> {
        script.calls.fetch_add(1, Ordering::SeqCst);
        let result = match request.command {
            RunnerCommand::HandoverInventory => serde_json::to_value(&script.inventory).unwrap(),
            _ => serde_json::Value::Null,
        };
        Json(CommandResponse::success(request.request_id, result))
    }

    #[cfg(not(windows))]
    async fn snapshot_runner(
        snapshot: models::AppSnapshot,
    ) -> (String, SnapshotScript, tokio::task::JoinHandle<()>) {
        let live_games = snapshot
            .games
            .iter()
            .filter(|game| game.status == "created" || game.status == "started")
            .count();
        handover_runner(
            snapshot,
            HandoverInventory {
                live_games,
                outgoing_challenges: 0,
            },
        )
        .await
    }

    #[cfg(not(windows))]
    async fn handover_runner(
        snapshot: models::AppSnapshot,
        inventory: HandoverInventory,
    ) -> (String, SnapshotScript, tokio::task::JoinHandle<()>) {
        let script = SnapshotScript {
            calls: Arc::new(AtomicUsize::new(0)),
            snapshot,
            inventory,
        };
        let app = Router::new()
            .route("/v2/snapshot", get(scripted_snapshot))
            .route("/v2/commands", post(scripted_inventory))
            .with_state(script.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (endpoint, script, server)
    }

    #[cfg(not(windows))]
    fn live_game(id: &str) -> models::LiveGame {
        models::LiveGame {
            id: id.into(),
            account_id: "bot".into(),
            bot_username: "Bot".into(),
            opponent: "Opponent".into(),
            bot_rating: Some(2000),
            opponent_rating: Some(2000),
            color: "white".into(),
            initial_fen: "startpos".into(),
            moves: String::new(),
            status: "started".into(),
            white_time: 60_000,
            black_time: 60_000,
            white_increment: 0,
            black_increment: 0,
            clock_updated_at: 0,
            result: None,
            engine_line: None,
            engine_info: None,
            engine_thinking: false,
            error: None,
        }
    }

    async fn command_runner() -> (String, CommandScript, tokio::task::JoinHandle<()>) {
        let script = CommandScript::default();
        let app = Router::new()
            .route("/v2/commands", post(scripted_command))
            .route("/v2/snapshot", get(empty_snapshot))
            .with_state(script.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (endpoint, script, server)
    }

    #[tokio::test]
    async fn account_scope_result_crosses_remote_runner_dispatch() {
        let (endpoint, script, server) = command_runner().await;
        let expected = models::AddAccountResult {
            account: models::AccountProfile {
                id: "bot".into(),
                username: "Bot".into(),
                engine_id: "engine".into(),
                rating: Some(2100),
                enabled: false,
            },
            scopes: vec!["bot:play".into(), "challenge:read".into()],
            missing_for_matchmaking: vec!["challenge:write".into()],
            can_play_games: true,
        };
        *script.result.lock().unwrap() = Some(serde_json::to_value(&expected).unwrap());
        let backend = remote_backend(loopback_identity(&endpoint));

        let actual = add_lichess_account_inner(
            models::AddAccountRequest {
                token: "pasted-token".into(),
                engine_id: "engine".into(),
            },
            &backend,
        )
        .await
        .unwrap();

        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
        assert_eq!(script.calls.load(Ordering::SeqCst), 1);
        stop_backend_forwarding(&backend).await;
        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn token_update_and_game_error_dismiss_cross_remote_runner_dispatch() {
        let (endpoint, script, server) = command_runner().await;
        let expected = models::AddAccountResult {
            account: models::AccountProfile {
                id: "bot".into(),
                username: "Bot".into(),
                engine_id: "engine".into(),
                rating: Some(2100),
                enabled: true,
            },
            scopes: vec!["bot:play".into(), "challenge:read".into()],
            missing_for_matchmaking: vec!["challenge:write".into()],
            can_play_games: true,
        };
        *script.result.lock().unwrap() = Some(serde_json::to_value(&expected).unwrap());
        let backend = remote_backend(loopback_identity(&endpoint));

        let actual =
            update_lichess_account_token_inner("bot".into(), "replacement-token".into(), &backend)
                .await
                .unwrap();

        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
        assert_eq!(
            script.last_command.lock().unwrap().clone().unwrap(),
            serde_json::json!({
                "command": "updateLichessAccountToken",
                "payload": { "accountId": "bot", "token": "replacement-token" }
            })
        );

        *script.result.lock().unwrap() = Some(serde_json::Value::Null);
        dismiss_game_error_inner("failed-game".into(), &backend)
            .await
            .unwrap();
        assert_eq!(
            script.last_command.lock().unwrap().clone().unwrap(),
            serde_json::json!({
                "command": "dismissGameError",
                "payload": { "gameId": "failed-game" }
            })
        );
        assert_eq!(script.calls.load(Ordering::SeqCst), 2);
        stop_backend_forwarding(&backend).await;
        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn older_runner_reports_clear_additive_command_errors() {
        let app = Router::new()
            .route("/v2/commands", post(reject_additive_command))
            .route("/v2/snapshot", get(empty_snapshot));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let backend = remote_backend(loopback_identity(&endpoint));

        assert_eq!(
            update_lichess_account_token_inner("bot".into(), "token".into(), &backend)
                .await
                .unwrap_err(),
            "The connected runner does not support updateLichessAccountToken. Update queen-runner and try again."
        );
        assert_eq!(
            dismiss_game_error_inner("failed-game".into(), &backend)
                .await
                .unwrap_err(),
            "The connected runner does not support dismissGameError. Update queen-runner and try again."
        );

        stop_backend_forwarding(&backend).await;
        server.abort();
        let _ = server.await;
    }

    fn loopback_identity(endpoint: &str) -> RunnerIdentity {
        RunnerIdentity {
            version: PAIRING_PAYLOAD_VERSION,
            url: endpoint.into(),
            cert_fp: String::new(),
            bearer: "x".repeat(32),
            generation: 1,
        }
    }

    async fn stop_backend_forwarding(backend: &BackendState) {
        if let Ok(mut switching) = backend.begin_switch().await {
            if let Ok(PreviousBackend::Active(active)) = switching.take_previous() {
                match active {
                    ActiveBackend::Embedded(embedded) => {
                        embedded.event_cancellation.cancel();
                        embedded.event_task.abort();
                    }
                    ActiveBackend::Remote(remote) => {
                        remote.event_cancellation.cancel();
                        remote.event_task.abort();
                    }
                }
            }
            switching.finish_previous(PreviousBackend::Unavailable(
                "test backend forwarding stopped".into(),
            ));
        }
    }

    async fn scripted_events(
        State(script): State<EventScript>,
        websocket: WebSocketUpgrade,
    ) -> Response {
        let attempt = script.attempts.fetch_add(1, Ordering::SeqCst) + 1;
        if attempt == 2 {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        if attempt == 1 {
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        websocket
            .on_upgrade(move |mut socket| async move {
                let event = EventEnvelope {
                    protocol_version: PROTOCOL_VERSION,
                    instance_id: uuid::Uuid::nil(),
                    sequence: attempt as u64,
                    event: if attempt == 1 {
                        CoreEvent::Snapshot(models::AppSnapshot::default())
                    } else {
                        CoreEvent::LogsUpdated
                    },
                };
                socket
                    .send(Message::Text(serde_json::to_string(&event).unwrap().into()))
                    .await
                    .unwrap();
                if attempt == 1 {
                    let _ = socket.send(Message::Close(None)).await;
                } else {
                    script.release.cancelled().await;
                }
            })
            .into_response()
    }

    fn push<T: TS>(output: &mut String) {
        // Tauri/serde_json transports Rust 64-bit integers as JavaScript JSON
        // numbers, not native BigInts. ts-rs deliberately defaults them to
        // `bigint`, so normalize the generated wire contract to the actual
        // serializer and export every declaration as a usable module member.
        // ts-rs can also emit spaces before line breaks, so normalize those at
        // generation time to keep the checked-in module diff-clean.
        output.push_str("export ");
        let declaration = T::decl().replace("bigint", "number");
        for (index, line) in declaration.lines().enumerate() {
            if index > 0 {
                output.push('\n');
            }
            output.push_str(line.trim_end());
        }
        output.push_str("\n\n");
    }

    /// ts-rs contract: every `cargo test` of the Tauri crate refreshes the one
    /// generated frontend model module. Existing handwritten frontend files do
    /// not import it until the frontend owner performs that migration.
    #[test]
    fn generate_frontend_ipc_models() {
        let mut output = String::from(
            "// @generated by `cargo test --manifest-path src-tauri/Cargo.toml`\n// Do not edit by hand.\n\n",
        );
        push::<models::UciOption>(&mut output);
        push::<models::OpeningBookConfig>(&mut output);
        push::<models::EngineProfile>(&mut output);
        push::<models::EngineOptionUpdate>(&mut output);
        push::<models::OpeningBookUpdate>(&mut output);
        push::<models::AccountProfile>(&mut output);
        push::<models::BotRuntime>(&mut output);
        push::<models::EngineTelemetry>(&mut output);
        push::<models::LiveGame>(&mut output);
        push::<models::CampaignEvent>(&mut output);
        push::<models::CampaignSettings>(&mut output);
        push::<models::CampaignStatus>(&mut output);
        push::<models::CampaignRuntime>(&mut output);
        push::<models::AppSnapshot>(&mut output);
        push::<models::AddAccountRequest>(&mut output);
        push::<models::AddAccountResult>(&mut output);
        push::<models::ChallengeRequest>(&mut output);
        push::<models::ChallengeResult>(&mut output);
        push::<history::ImportReport>(&mut output);
        push::<history::ScorebookFilter>(&mut output);
        push::<history::Streak>(&mut output);
        push::<history::EngineLine>(&mut output);
        push::<history::ColorLine>(&mut output);
        push::<history::PerfLine>(&mut output);
        push::<history::BandLine>(&mut output);
        push::<history::TerminationLine>(&mut output);
        push::<history::OpponentLine>(&mut output);
        push::<history::DayLine>(&mut output);
        push::<history::RatingPoint>(&mut output);
        push::<history::OpeningLine>(&mut output);
        push::<history::AccountRef>(&mut output);
        push::<history::EngineRef>(&mut output);
        push::<history::LabGame>(&mut output);
        push::<history::EngineLabLine>(&mut output);
        push::<history::DepthLine>(&mut output);
        push::<history::BookLab>(&mut output);
        push::<history::ReliabilityTotals>(&mut output);
        push::<history::ConfigLine>(&mut output);
        push::<history::ScorebookLab>(&mut output);
        push::<history::ScorebookStats>(&mut output);
        push::<enginelog::LogRetention>(&mut output);
        push::<enginelog::LogSessionSummary>(&mut output);
        push::<enginelog::LogFilter>(&mut output);
        push::<enginelog::LogQuery>(&mut output);
        push::<enginelog::LogLine>(&mut output);
        push::<enginelog::LogHeaderField>(&mut output);
        push::<enginelog::LogPage>(&mut output);
        push::<enginelog::LogSearchBlock>(&mut output);
        push::<enginelog::LogMatch>(&mut output);
        push::<enginelog::LogSessionMatches>(&mut output);
        push::<enginelog::LogsOverview>(&mut output);
        push::<enginelog::ExportMode>(&mut output);
        push::<diagnostics::DiagnosticEntry>(&mut output);
        push::<diagnostics::DiagnosticFilter>(&mut output);
        push::<queen_protocol::EngineRoot>(&mut output);
        push::<queen_protocol::EngineBrowseEntryKind>(&mut output);
        push::<queen_protocol::EngineBrowseEntry>(&mut output);
        push::<queen_protocol::EngineBrowseRequest>(&mut output);
        push::<queen_protocol::EngineBrowseResponse>(&mut output);
        push::<RunnerSettingsView>(&mut output);
        push::<RunnerConnectionTest>(&mut output);
        push::<RunnerConnectionState>(&mut output);
        push::<RunnerConnectionEvent>(&mut output);
        push::<BackendSnapshotEvent>(&mut output);
        push::<BackendNotificationEvent>(&mut output);

        let content_len = output.trim_end().len();
        output.truncate(content_len);
        output.push('\n');

        let target = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("repository root")
            .join("src/types/models.gen.ts");
        if fs::read_to_string(&target).ok().as_deref() != Some(output.as_str()) {
            fs::write(&target, output).expect("write generated IPC models");
        }
    }

    #[test]
    fn runner_identity_is_one_versioned_atomic_record() {
        let identity = RunnerIdentity {
            version: PAIRING_PAYLOAD_VERSION,
            url: "https://runner.example".into(),
            cert_fp: "ab".repeat(32),
            bearer: "x".repeat(43),
            generation: 4,
        };
        let encoded = encode_runner_identity(&identity).unwrap();
        assert_eq!(decode_runner_identity(&encoded).unwrap(), identity);
        assert!(decode_runner_identity(&"x".repeat(32)).is_err());
    }

    #[test]
    fn changed_url_never_builds_a_client_with_the_old_bearer() {
        let identity = RunnerIdentity {
            version: PAIRING_PAYLOAD_VERSION,
            url: "https://runner.example".into(),
            cert_fp: "ab".repeat(32),
            bearer: "old-bearer-that-must-stay-local-forever".into(),
            generation: 1,
        };
        assert!(runner_client_for_endpoint(identity, "https://attacker.example").is_err());
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn embedded_to_remote_atomically_refuses_a_pre_snapshot_game_reservation() {
        let directory = std::env::temp_dir().join(format!(
            "queenui-live-switch-refusal-{}",
            uuid::Uuid::new_v4()
        ));
        let endpoint = "https://runner.example";
        let original = DesktopRunnerConfig::default();
        save_desktop_config(&directory, &original).unwrap();
        let settings = settings_state(directory.clone(), original.clone(), "embedded", None);
        let core = AppState::new(directory.clone(), models::AppConfig::default()).unwrap();
        core.reserve_game_for_test("bot", "reserved-before-snapshot")
            .await;
        let backend = BackendState::active(ActiveBackend::Embedded(EmbeddedBackend::new(
            core.clone(),
            BackendEvents::noop(),
        )));

        let error = set_runner_settings_inner(
            "remote".into(),
            Some(endpoint.into()),
            None,
            None,
            None,
            &settings,
            &backend,
            BackendEvents::noop(),
        )
        .await
        .unwrap_err();

        assert_eq!(
            error,
            "1 game is still being played from this computer; finish or resign them before switching to a runner."
        );
        assert_eq!(load_desktop_config(&directory).unwrap(), original);
        assert!(matches!(
            backend.dispatch_backend().unwrap().target(),
            super::DispatchTarget::Embedded(_)
        ));
        assert_eq!(
            core.live_game_count(),
            0,
            "the presentation counter must not drive switch refusal"
        );
        stop_backend_forwarding(&backend).await;
        drop(core);
        let _ = fs::remove_dir_all(directory);
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn embedded_to_remote_ignores_finished_task_drains_and_dispatches_remotely() {
        let (endpoint, script, server) = command_runner().await;
        let directory = std::env::temp_dir().join(format!(
            "queenui-live-switch-remote-{}",
            uuid::Uuid::new_v4()
        ));
        let identity = loopback_identity(&endpoint);
        store_runner_identity(&directory, &identity).unwrap();
        let original = DesktopRunnerConfig::default();
        save_desktop_config(&directory, &original).unwrap();
        let settings = settings_state(directory.clone(), original, "embedded", None);
        let core = AppState::new(directory.clone(), models::AppConfig::default()).unwrap();
        core.install_finished_game_task_for_test("bot", "finished-game")
            .await;
        let backend = BackendState::active(ActiveBackend::Embedded(EmbeddedBackend::new(
            core,
            BackendEvents::noop(),
        )));

        let view = set_runner_settings_inner(
            "remote".into(),
            Some(endpoint.clone()),
            None,
            None,
            None,
            &settings,
            &backend,
            BackendEvents::noop(),
        )
        .await
        .unwrap();

        assert_eq!(view.active_mode, "remote");
        assert!(!view.restart_required);
        let dispatch = backend.dispatch_backend().unwrap();
        let super::DispatchTarget::Remote(client) = dispatch.target() else {
            panic!("command dispatch did not switch to the remote client");
        };
        client
            .command::<()>(RunnerCommand::ClearDiagnostics)
            .await
            .unwrap();
        assert_eq!(script.calls.load(Ordering::SeqCst), 1);
        let authority = queen_core::storage::DataDirLock::acquire(&directory)
            .expect("the drained embedded AppState must release its data-directory lock");
        drop(authority);

        stop_backend_forwarding(&backend).await;
        server.abort();
        let _ = server.await;
        let _ = fs::remove_dir_all(directory);
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn embedded_drain_error_records_diagnostic_and_still_publishes_remote() {
        let (endpoint, _, server) = command_runner().await;
        let directory = std::env::temp_dir().join(format!(
            "queenui-drain-error-proceeds-{}",
            uuid::Uuid::new_v4()
        ));
        store_runner_identity(&directory, &loopback_identity(&endpoint)).unwrap();
        let settings = settings_state(
            directory.clone(),
            DesktopRunnerConfig::default(),
            "embedded",
            None,
        );
        let core = AppState::new(directory.clone(), models::AppConfig::default()).unwrap();
        core.install_failing_supervisor_for_test("failing-bot")
            .await;
        let diagnostics_before: std::collections::HashSet<_> = diagnostics::global()
            .unwrap()
            .recent(&diagnostics::DiagnosticFilter::default())
            .into_iter()
            .map(|entry| entry.id)
            .collect();
        let backend = BackendState::active(ActiveBackend::Embedded(EmbeddedBackend::new(
            core,
            BackendEvents::noop(),
        )));

        let view = set_runner_settings_inner(
            "remote".into(),
            Some(endpoint),
            None,
            None,
            None,
            &settings,
            &backend,
            BackendEvents::noop(),
        )
        .await
        .unwrap();

        assert_eq!(view.active_mode, "remote");
        assert!(!view.restart_required);
        let diagnostic = diagnostics::global()
            .unwrap()
            .recent(&diagnostics::DiagnosticFilter {
                level: Some("error".into()),
                query: Some("live switch continued".into()),
                ..Default::default()
            })
            .into_iter()
            .find(|entry| !diagnostics_before.contains(&entry.id))
            .expect("the destructive drain error must be recorded");
        assert_eq!(
            diagnostic.message,
            "Embedded runner drain failed; live switch continued"
        );
        assert!(diagnostic
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("failed while joining")));

        stop_backend_forwarding(&backend).await;
        server.abort();
        let _ = server.await;
        let _ = fs::remove_dir_all(directory);
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn embedded_to_unreachable_remote_succeeds_and_leaves_reconnect_to_forwarder() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let directory = std::env::temp_dir().join(format!(
            "queenui-live-switch-unreachable-{}",
            uuid::Uuid::new_v4()
        ));
        store_runner_identity(&directory, &loopback_identity(&endpoint)).unwrap();
        let settings = settings_state(
            directory.clone(),
            DesktopRunnerConfig::default(),
            "embedded",
            None,
        );
        let core = AppState::new(directory.clone(), models::AppConfig::default()).unwrap();
        let backend = BackendState::active(ActiveBackend::Embedded(EmbeddedBackend::new(
            core,
            BackendEvents::noop(),
        )));

        let view = set_runner_settings_inner(
            "remote".into(),
            Some(endpoint.clone()),
            None,
            None,
            None,
            &settings,
            &backend,
            BackendEvents::noop(),
        )
        .await
        .unwrap();

        assert_eq!(view.active_mode, "remote");
        assert_eq!(view.url.as_deref(), Some(endpoint.as_str()));
        assert!(!view.restart_required);
        assert!(matches!(
            backend.dispatch_backend().unwrap().target(),
            super::DispatchTarget::Remote(_)
        ));

        stop_backend_forwarding(&backend).await;
        let _ = fs::remove_dir_all(directory);
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn remote_to_embedded_lock_failure_restores_functional_remote_and_restart_required() {
        let (endpoint, script, server) = command_runner().await;
        let directory = std::env::temp_dir().join(format!(
            "queenui-live-switch-lock-failure-{}",
            uuid::Uuid::new_v4()
        ));
        let identity = loopback_identity(&endpoint);
        store_runner_identity(&directory, &identity).unwrap();
        let remote_config = DesktopRunnerConfig {
            mode: "remote".into(),
            url: Some(endpoint.clone()),
            allow_insecure_remote_http: false,
        };
        save_desktop_config(&directory, &remote_config).unwrap();
        let settings = settings_state(
            directory.clone(),
            remote_config,
            "remote",
            Some(endpoint.clone()),
        );
        let remote = RemoteBackend::new(
            queen_client::RunnerClient::from_identity(identity.clone()).unwrap(),
            BackendEvents::noop(),
        );
        let backend = BackendState::active(ActiveBackend::Remote(remote));
        let authority = queen_core::storage::DataDirLock::acquire(&directory).unwrap();

        let error = set_runner_settings_inner(
            "embedded".into(),
            None,
            None,
            None,
            Some(endpoint.clone()),
            &settings,
            &backend,
            BackendEvents::noop(),
        )
        .await
        .unwrap_err();

        assert!(error.starts_with(
            "Runner settings were saved, but the switch could not complete; restarting QueenUI will retry it: QueenUI automation is already owned"
        ), "{error}");
        assert_eq!(
            load_desktop_config(&directory).unwrap(),
            DesktopRunnerConfig::default()
        );
        let view = settings_view(&settings);
        assert!(view.restart_required);
        assert_eq!(view.active_mode, "remote");
        let dispatch = backend.dispatch_backend().unwrap();
        let super::DispatchTarget::Remote(client) = dispatch.target() else {
            panic!("the remote backend was not restored after local load failed");
        };
        client
            .command::<()>(RunnerCommand::ClearDiagnostics)
            .await
            .unwrap();
        assert_eq!(script.calls.load(Ordering::SeqCst), 2);

        stop_backend_forwarding(&backend).await;
        drop(authority);
        server.abort();
        let _ = server.await;
        let _ = fs::remove_dir_all(directory);
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn remote_handover_uses_authoritative_inventory_and_then_proceeds() {
        let (endpoint, script, server) = handover_runner(
            models::AppSnapshot::default(),
            HandoverInventory {
                live_games: 2,
                outgoing_challenges: 0,
            },
        )
        .await;
        let directory = std::env::temp_dir().join(format!(
            "queenui-remote-handover-live-{}",
            uuid::Uuid::new_v4()
        ));
        let identity = loopback_identity(&endpoint);
        store_runner_identity(&directory, &identity).unwrap();
        let config = DesktopRunnerConfig {
            mode: "remote".into(),
            url: Some(endpoint.clone()),
            allow_insecure_remote_http: false,
        };
        save_desktop_config(&directory, &config).unwrap();
        let settings = settings_state(directory.clone(), config, "remote", Some(endpoint.clone()));
        let backend = remote_backend(identity);

        let error = set_runner_settings_inner(
            "embedded".into(),
            None,
            None,
            None,
            None,
            &settings,
            &backend,
            BackendEvents::noop(),
        )
        .await
        .unwrap_err();
        assert_eq!(
            error,
            format!(
                "The remote runner at {endpoint} is still playing 2 games. Confirm that they will keep playing there before switching runners."
            )
        );
        assert_eq!(script.calls.load(Ordering::SeqCst), 1);
        assert!(matches!(
            backend.dispatch_backend().unwrap().target(),
            super::DispatchTarget::Remote(_)
        ));

        let view = set_runner_settings_inner(
            "embedded".into(),
            None,
            None,
            None,
            Some(endpoint.clone()),
            &settings,
            &backend,
            BackendEvents::noop(),
        )
        .await
        .unwrap();
        assert_eq!(script.calls.load(Ordering::SeqCst), 2);
        assert_eq!(view.active_mode, "embedded");

        stop_backend_forwarding(&backend).await;
        server.abort();
        let _ = server.await;
        let _ = fs::remove_dir_all(directory);
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn different_remote_handover_names_the_current_runner_and_live_count() {
        let current_snapshot = models::AppSnapshot {
            games: vec![live_game("game-1")],
            ..models::AppSnapshot::default()
        };
        let (current_endpoint, current_script, current_server) =
            snapshot_runner(current_snapshot).await;
        let (target_endpoint, _, target_server) =
            snapshot_runner(models::AppSnapshot::default()).await;
        let directory = std::env::temp_dir().join(format!(
            "queenui-different-remote-handover-{}",
            uuid::Uuid::new_v4()
        ));
        let current_config = DesktopRunnerConfig {
            mode: "remote".into(),
            url: Some(current_endpoint.clone()),
            allow_insecure_remote_http: false,
        };
        save_desktop_config(&directory, &current_config).unwrap();
        store_runner_identity(&directory, &loopback_identity(&target_endpoint)).unwrap();
        let settings = settings_state(
            directory.clone(),
            current_config,
            "remote",
            Some(current_endpoint.clone()),
        );
        let backend = remote_backend(loopback_identity(&current_endpoint));

        let error = set_runner_settings_inner(
            "remote".into(),
            Some(target_endpoint),
            None,
            None,
            None,
            &settings,
            &backend,
            BackendEvents::noop(),
        )
        .await
        .unwrap_err();
        assert_eq!(
            error,
            format!(
                "The remote runner at {current_endpoint} is still playing 1 game. Confirm that it will keep playing there before switching runners."
            )
        );
        assert_eq!(current_script.calls.load(Ordering::SeqCst), 1);

        stop_backend_forwarding(&backend).await;
        current_server.abort();
        target_server.abort();
        let _ = current_server.await;
        let _ = target_server.await;
        let _ = fs::remove_dir_all(directory);
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn stale_acknowledged_runner_cannot_authorize_the_current_remote() {
        let (endpoint, script, server) = handover_runner(
            models::AppSnapshot::default(),
            HandoverInventory {
                live_games: 1,
                outgoing_challenges: 0,
            },
        )
        .await;
        let remote = RemoteBackend::new(
            queen_client::RunnerClient::from_identity(loopback_identity(&endpoint)).unwrap(),
            BackendEvents::noop(),
        );

        let error = verify_remote_handover(&remote, Some("http://previous-runner.invalid"))
            .await
            .unwrap_err();
        assert_eq!(
            error,
            format!(
                "The remote runner at {endpoint} is still playing 1 game. Confirm that it will keep playing there before switching runners."
            )
        );
        verify_remote_handover(&remote, Some(&endpoint))
            .await
            .unwrap();
        assert_eq!(script.calls.load(Ordering::SeqCst), 2);

        remote.event_cancellation.cancel();
        remote.event_task.abort();
        server.abort();
        let _ = server.await;
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn remote_outgoing_challenges_are_disclosed_by_the_inventory() {
        let (endpoint, _, server) = handover_runner(
            models::AppSnapshot::default(),
            HandoverInventory {
                live_games: 0,
                outgoing_challenges: 2,
            },
        )
        .await;
        let remote = RemoteBackend::new(
            queen_client::RunnerClient::from_identity(loopback_identity(&endpoint)).unwrap(),
            BackendEvents::noop(),
        );

        assert_eq!(
            verify_remote_handover(&remote, None).await.unwrap_err(),
            format!(
                "The remote runner at {endpoint} still owns 2 outgoing challenges. Confirm that they will remain there before switching runners."
            )
        );

        remote.event_cancellation.cancel();
        remote.event_task.abort();
        server.abort();
        let _ = server.await;
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn older_runner_without_handover_inventory_is_cannot_verify() {
        let app = Router::new().route("/v2/snapshot", get(empty_snapshot));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let remote = RemoteBackend::new(
            queen_client::RunnerClient::from_identity(loopback_identity(&endpoint)).unwrap(),
            BackendEvents::noop(),
        );

        assert_eq!(
            verify_remote_handover(&remote, None).await.unwrap_err(),
            format!(
                "Could not verify the remote runner at {endpoint}; it may still be playing games. Confirm that its games will keep running there before switching runners."
            )
        );

        remote.event_cancellation.cancel();
        remote.event_task.abort();
        server.abort();
        let _ = server.await;
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn unreachable_remote_handover_requires_cannot_verify_acknowledgement() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let directory = std::env::temp_dir().join(format!(
            "queenui-unreachable-remote-handover-{}",
            uuid::Uuid::new_v4()
        ));
        let config = DesktopRunnerConfig {
            mode: "remote".into(),
            url: Some(endpoint.clone()),
            allow_insecure_remote_http: false,
        };
        let settings = settings_state(directory.clone(), config, "remote", Some(endpoint.clone()));
        let backend = remote_backend(loopback_identity(&endpoint));

        let error = set_runner_settings_inner(
            "embedded".into(),
            None,
            None,
            None,
            None,
            &settings,
            &backend,
            BackendEvents::noop(),
        )
        .await
        .unwrap_err();
        assert_eq!(
            error,
            format!(
                "Could not verify the remote runner at {endpoint}; it may still be playing games. Confirm that its games will keep running there before switching runners."
            )
        );
        assert!(matches!(
            backend.dispatch_backend().unwrap().target(),
            super::DispatchTarget::Remote(_)
        ));

        stop_backend_forwarding(&backend).await;
        let _ = fs::remove_dir_all(directory);
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn embedded_publication_emits_fresh_snapshot_before_same_generation_connection() {
        let (endpoint, _, server) = snapshot_runner(models::AppSnapshot::default()).await;
        let directory = std::env::temp_dir().join(format!(
            "queenui-embedded-publication-order-{}",
            uuid::Uuid::new_v4()
        ));
        let identity = loopback_identity(&endpoint);
        store_runner_identity(&directory, &identity).unwrap();
        let config = DesktopRunnerConfig {
            mode: "remote".into(),
            url: Some(endpoint.clone()),
            allow_insecure_remote_http: false,
        };
        save_desktop_config(&directory, &config).unwrap();
        let settings = settings_state(directory.clone(), config, "remote", Some(endpoint));
        let backend = remote_backend(identity);
        let emitted = Arc::new(Mutex::new(Vec::<String>::new()));
        let core_emitted = emitted.clone();
        let connection_emitted = emitted.clone();
        let events = BackendEvents {
            core: Arc::new(move |event| {
                if matches!(event.event, CoreEvent::Snapshot(_)) {
                    core_emitted
                        .lock()
                        .unwrap()
                        .push(format!("snapshot:{}", event.backend_generation));
                }
            }),
            connection: Arc::new(move |event| {
                if event.state == RunnerConnectionState::Embedded {
                    connection_emitted
                        .lock()
                        .unwrap()
                        .push(format!("embedded:{}", event.backend_generation));
                }
            }),
        };

        set_runner_settings_inner(
            "embedded".into(),
            None,
            None,
            None,
            None,
            &settings,
            &backend,
            events,
        )
        .await
        .unwrap();

        assert_eq!(
            &*emitted.lock().unwrap(),
            &["snapshot:2".to_string(), "embedded:2".to_string()]
        );
        assert_eq!(backend.generation.load(Ordering::SeqCst), 2);

        stop_backend_forwarding(&backend).await;
        server.abort();
        let _ = server.await;
        let _ = fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn backend_command_during_swap_fails_fast_with_switching_error() {
        let backend = remote_backend(RunnerIdentity {
            version: PAIRING_PAYLOAD_VERSION,
            url: "https://runner.example".into(),
            cert_fp: "ab".repeat(32),
            bearer: "paired-runner-bearer-long-enough".into(),
            generation: 1,
        });
        let mut switching = backend.begin_switch().await.unwrap();

        let error = match backend.dispatch_backend() {
            Ok(_) => panic!("a command entered the backend while its swap gate was active"),
            Err(error) => error,
        };
        assert_eq!(error, "QueenUI is switching runners; retry in a moment");

        switching.restore();
        assert!(backend.dispatch_backend().is_ok());
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn in_flight_command_lease_finishes_before_remote_backend_is_published() {
        let (endpoint, _, server) = command_runner().await;
        let directory = std::env::temp_dir().join(format!(
            "queenui-command-lease-fence-{}",
            uuid::Uuid::new_v4()
        ));
        store_runner_identity(&directory, &loopback_identity(&endpoint)).unwrap();
        let settings = Arc::new(settings_state(
            directory.clone(),
            DesktopRunnerConfig::default(),
            "embedded",
            None,
        ));
        let core = AppState::new(directory.clone(), models::AppConfig::default()).unwrap();
        let backend = Arc::new(BackendState::active(ActiveBackend::Embedded(
            EmbeddedBackend::new(core.clone(), BackendEvents::noop()),
        )));
        let dispatch = backend.dispatch_backend().unwrap();
        let release = CancellationToken::new();
        let command_release = release.clone();
        let effects = Arc::new(AtomicUsize::new(0));
        let command_effects = effects.clone();
        let command = tokio::spawn(async move {
            command_release.cancelled().await;
            let super::DispatchTarget::Embedded(old_core) = dispatch.target() else {
                panic!("the pre-switch command did not lease the embedded core");
            };
            let _ = old_core.snapshot().await;
            command_effects.fetch_add(1, Ordering::SeqCst);
        });

        let switch_settings = settings.clone();
        let switch_backend = backend.clone();
        let switch_endpoint = endpoint.clone();
        let switching = tokio::spawn(async move {
            set_runner_settings_inner(
                "remote".into(),
                Some(switch_endpoint),
                None,
                None,
                None,
                &switch_settings,
                &switch_backend,
                BackendEvents::noop(),
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if matches!(
                    &*backend
                        .slot
                        .read()
                        .unwrap_or_else(std::sync::PoisonError::into_inner),
                    super::BackendSlot::Switching
                ) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(
            !switching.is_finished(),
            "the new backend published while an old command lease was active"
        );

        release.cancel();
        command.await.unwrap();
        assert_eq!(
            backend.leases.active.load(Ordering::SeqCst),
            0,
            "the completed command retained its backend lease"
        );
        let view = switching.await.unwrap().unwrap();
        assert_eq!(effects.load(Ordering::SeqCst), 1);
        assert_eq!(view.active_mode, "remote");
        assert!(matches!(
            backend.dispatch_backend().unwrap().target(),
            super::DispatchTarget::Remote(_)
        ));
        tokio::task::yield_now().await;
        assert_eq!(
            effects.load(Ordering::SeqCst),
            1,
            "a pre-swap command affected the old core after publication"
        );

        drop(core);
        stop_backend_forwarding(&backend).await;
        server.abort();
        let _ = server.await;
        let _ = fs::remove_dir_all(directory);
    }

    #[tokio::test(start_paused = true)]
    async fn lease_drain_timeout_restores_the_previous_backend() {
        let backend = Arc::new(remote_backend(RunnerIdentity {
            version: PAIRING_PAYLOAD_VERSION,
            url: "https://runner.example".into(),
            cert_fp: "ab".repeat(32),
            bearer: "paired-runner-bearer-long-enough".into(),
            generation: 1,
        }));
        let lease = backend.dispatch_backend().unwrap();
        let switching_backend = backend.clone();
        let switching = tokio::spawn(async move {
            switching_backend.begin_switch().await.map(|mut guard| {
                guard.restore();
            })
        });
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(11)).await;

        let error = switching.await.unwrap().unwrap_err();
        assert_eq!(error, super::SWITCHING_RUNNERS_ERROR);
        assert!(matches!(
            backend.dispatch_backend().unwrap().target(),
            super::DispatchTarget::Remote(_)
        ));

        drop(lease);
        stop_backend_forwarding(&backend).await;
    }

    #[tokio::test]
    async fn abandoned_switch_guard_publishes_a_clear_unavailable_state() {
        let directory =
            std::env::temp_dir().join(format!("queenui-abandoned-switch-{}", uuid::Uuid::new_v4()));
        let config = DesktopRunnerConfig {
            mode: "remote".into(),
            url: Some("https://runner.example".into()),
            allow_insecure_remote_http: false,
        };
        let settings = settings_state(
            directory.clone(),
            config,
            "remote",
            Some("https://runner.example".into()),
        );
        let backend = remote_backend(RunnerIdentity {
            version: PAIRING_PAYLOAD_VERSION,
            url: "https://runner.example".into(),
            cert_fp: "ab".repeat(32),
            bearer: "paired-runner-bearer-long-enough".into(),
            generation: 1,
        });
        let mut switching = backend.begin_settings_switch(&settings).await.unwrap();
        let previous = switching.take_previous().unwrap();
        if let PreviousBackend::Active(ActiveBackend::Remote(remote)) = previous {
            remote.event_cancellation.cancel();
            remote.event_task.abort();
        }
        drop(switching);

        let error = match backend.dispatch_backend() {
            Ok(_) => panic!("an abandoned switch retained a dispatchable backend"),
            Err(error) => error,
        };
        assert_eq!(error, INTERRUPTED_SWITCH_ERROR);
        assert!(settings_view(&settings).restart_required);
        let _ = fs::remove_dir_all(directory);
    }

    #[cfg(not(windows))]
    #[test]
    fn legacy_bearer_is_only_an_untrusted_url_hint() {
        let directory =
            std::env::temp_dir().join(format!("queenui-legacy-hint-{}", uuid::Uuid::new_v4()));
        let secret_directory = directory.join("desktop-secrets");
        fs::create_dir_all(&secret_directory).unwrap();
        let bearer = "legacy-bearer-must-remain-inert";
        fs::write(
            secret_directory.join("active-runner.token"),
            serde_json::json!({
                "endpoint": "https://runner.example",
                "token": bearer
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(
            legacy_runner_url_hint(&directory).as_deref(),
            Some("https://runner.example")
        );
        assert!(get_runner_identity(&directory).is_err());
        let state = settings_state(
            directory.clone(),
            DesktopRunnerConfig {
                mode: "remote".into(),
                url: legacy_runner_url_hint(&directory),
                allow_insecure_remote_http: false,
            },
            "remote",
            Some("https://runner.example".into()),
        );
        let legacy_view = settings_view(&state);
        assert!(!legacy_view.paired);
        assert!(legacy_view.url.is_some());

        store_runner_identity(
            &directory,
            &RunnerIdentity {
                version: PAIRING_PAYLOAD_VERSION,
                url: "https://runner.example".into(),
                cert_fp: "ab".repeat(32),
                bearer: "paired-runner-bearer".into(),
                generation: 1,
            },
        )
        .unwrap();
        assert!(settings_view(&state).paired);
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn switching_to_embedded_deletes_stored_runner_identity() {
        use queen_core::storage::SecretStore;

        let (endpoint, _, server) = command_runner().await;
        let directory =
            std::env::temp_dir().join(format!("queenui-embedded-switch-{}", uuid::Uuid::new_v4()));
        let identity = RunnerIdentity {
            version: PAIRING_PAYLOAD_VERSION,
            url: endpoint.clone(),
            cert_fp: String::new(),
            bearer: "stored-runner-bearer-that-must-be-deleted".into(),
            generation: 3,
        };
        store_runner_identity(&directory, &identity).unwrap();
        queen_core::storage::FileSecretStore::new(directory.join("desktop-secrets"))
            .store(
                LEGACY_RUNNER_TOKEN_KEY,
                &serde_json::json!({
                    "endpoint": identity.url,
                    "token": "legacy-bearer-that-must-also-be-deleted"
                })
                .to_string(),
            )
            .unwrap();
        let state = settings_state(
            directory.clone(),
            DesktopRunnerConfig {
                mode: "remote".into(),
                url: Some(endpoint.clone()),
                allow_insecure_remote_http: false,
            },
            "remote",
            Some(endpoint.clone()),
        );
        save_desktop_config(
            &directory,
            &DesktopRunnerConfig {
                mode: "remote".into(),
                url: Some(endpoint.clone()),
                allow_insecure_remote_http: false,
            },
        )
        .unwrap();

        let backend = remote_backend(identity);
        let view = set_runner_settings_inner(
            "embedded".into(),
            None,
            None,
            None,
            Some(endpoint),
            &state,
            &backend,
            BackendEvents::noop(),
        )
        .await
        .unwrap();
        assert_eq!(view.mode, "embedded");
        assert_eq!(view.active_mode, "embedded");
        assert!(!view.restart_required);
        assert!(view.url.is_none());
        assert!(!view.paired);
        assert!(get_runner_identity(&directory).is_err());
        assert!(legacy_runner_url_hint(&directory).is_none());
        assert_eq!(
            load_desktop_config(&directory).unwrap(),
            DesktopRunnerConfig::default()
        );

        stop_backend_forwarding(&backend).await;
        server.abort();
        let _ = server.await;
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn embedded_config_save_failure_preserves_stored_runner_identity() {
        let (endpoint, _, server) = command_runner().await;
        let directory = std::env::temp_dir().join(format!(
            "queenui-embedded-save-failure-{}",
            uuid::Uuid::new_v4()
        ));
        let remote = DesktopRunnerConfig {
            mode: "remote".into(),
            url: Some(endpoint.clone()),
            allow_insecure_remote_http: false,
        };
        save_desktop_config(&directory, &remote).unwrap();
        let identity = RunnerIdentity {
            version: PAIRING_PAYLOAD_VERSION,
            url: endpoint,
            cert_fp: String::new(),
            bearer: "stored-runner-bearer-long-enough".into(),
            generation: 3,
        };
        store_runner_identity(&directory, &identity).unwrap();
        fs::create_dir(desktop_config_path(&directory).with_extension("json.tmp")).unwrap();
        let state = settings_state(
            directory.clone(),
            remote.clone(),
            "remote",
            remote.url.clone(),
        );
        let backend = remote_backend(identity.clone());

        let error = set_runner_settings_inner(
            "embedded".into(),
            None,
            None,
            None,
            remote.url.clone(),
            &state,
            &backend,
            BackendEvents::noop(),
        )
        .await
        .unwrap_err();
        assert!(error.contains("Could not write runner settings"), "{error}");
        assert_eq!(get_runner_identity(&directory).unwrap(), identity);
        assert_eq!(load_desktop_config(&directory).unwrap(), remote);

        assert!(!settings_view(&state).restart_required);
        assert!(matches!(
            backend.dispatch_backend().unwrap().target(),
            super::DispatchTarget::Remote(_)
        ));
        stop_backend_forwarding(&backend).await;
        server.abort();
        let _ = server.await;
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn embedded_switch_persists_mode_before_pairing_record_deletion_failure() {
        let directory = std::env::temp_dir().join(format!(
            "queenui-embedded-delete-failure-{}",
            uuid::Uuid::new_v4()
        ));
        let remote = DesktopRunnerConfig {
            mode: "remote".into(),
            url: Some("https://runner.example".into()),
            allow_insecure_remote_http: false,
        };
        save_desktop_config(&directory, &remote).unwrap();
        let identity_path = directory
            .join("desktop-secrets")
            .join(format!("{RUNNER_IDENTITY_KEY}.token"));
        fs::create_dir_all(&identity_path).unwrap();
        let state = settings_state(
            directory.clone(),
            remote.clone(),
            "remote",
            remote.url.clone(),
        );
        let backend = BackendState::unavailable("test remote is unavailable".into());

        let error = set_runner_settings_inner(
            "embedded".into(),
            None,
            None,
            None,
            None,
            &state,
            &backend,
            BackendEvents::noop(),
        )
        .await
        .unwrap_err();
        assert!(error.contains("mode is embedded"), "{error}");
        assert!(
            error.contains("stored pairing record could not be removed"),
            "{error}"
        );
        assert!(error.contains("Forget the paired runner"), "{error}");
        assert_eq!(
            load_desktop_config(&directory).unwrap(),
            DesktopRunnerConfig::default()
        );
        assert_eq!(
            state
                .configured
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
            DesktopRunnerConfig::default()
        );

        let view = settings_view(&state);
        assert_eq!(view.active_mode, "embedded");
        assert!(!view.restart_required);
        assert!(matches!(
            backend.dispatch_backend().unwrap().target(),
            super::DispatchTarget::Embedded(_)
        ));
        stop_backend_forwarding(&backend).await;
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn same_endpoint_re_pair_replaces_identity_and_revokes_the_old_bearer() {
        let old_bearer = "old-runner-bearer-that-will-be-revoked".to_string();
        let new_bearer = "new-runner-bearer-that-replaces-the-old".to_string();
        let capability_state = PairingCapabilityState {
            bearer: Arc::new(Mutex::new(old_bearer.clone())),
        };
        let app = Router::new()
            .route("/v2/capabilities", get(authenticated_capabilities))
            .route("/v2/commands", post(authenticated_command))
            .with_state(capability_state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let directory =
            std::env::temp_dir().join(format!("queenui-repair-{}", uuid::Uuid::new_v4()));
        let old_identity = RunnerIdentity {
            version: PAIRING_PAYLOAD_VERSION,
            url: endpoint.clone(),
            cert_fp: "aa".repeat(32),
            bearer: old_bearer,
            generation: 1,
        };
        let rotated_identity = RunnerIdentity {
            version: PAIRING_PAYLOAD_VERSION,
            url: endpoint.clone(),
            cert_fp: "bb".repeat(32),
            bearer: new_bearer.clone(),
            generation: 2,
        };
        store_runner_identity(&directory, &old_identity).unwrap();
        let old_client = queen_client::RunnerClient::from_identity(old_identity.clone()).unwrap();
        assert!(old_client.capabilities().await.is_ok());
        let state = settings_state(
            directory.clone(),
            DesktopRunnerConfig {
                mode: "remote".into(),
                url: Some(endpoint.clone()),
                allow_insecure_remote_http: false,
            },
            "remote",
            Some(endpoint.clone()),
        );
        let active_bearer = capability_state.bearer.clone();
        let redeemed_identity = rotated_identity.clone();
        let backend = remote_backend(old_identity);

        let connection = pair_and_store(
            "same-endpoint-rotation",
            &state,
            &backend,
            BackendEvents::noop(),
            move |_| {
                *active_bearer.lock().unwrap() = new_bearer;
                std::future::ready(Ok(redeemed_identity))
            },
        )
        .await
        .unwrap();
        assert_eq!(connection.hostname, "paired-runner");
        assert_eq!(get_runner_identity(&directory).unwrap(), rotated_identity);
        assert!(old_client.capabilities().await.is_err());
        let dispatch = backend.dispatch_backend().unwrap();
        let super::DispatchTarget::Remote(current) = dispatch.target() else {
            panic!("re-pair did not publish the replacement remote backend");
        };
        current
            .command::<()>(RunnerCommand::ClearDiagnostics)
            .await
            .unwrap();
        assert!(!settings_view(&state).restart_required);
        assert_eq!(backend.generation.load(Ordering::SeqCst), 2);
        {
            let slot = backend
                .slot
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert!(matches!(
                &*slot,
                super::BackendSlot::Active(ActiveBackend::Remote(remote))
                    if remote.backend_generation == 2
            ));
        }

        drop(dispatch);
        stop_backend_forwarding(&backend).await;
        server.abort();
        let _ = server.await;
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn unavailable_remote_recovers_through_pair_adoption_and_same_url_save() {
        let bearer = "recovered-runner-bearer-long-enough".to_string();
        let capability_state = PairingCapabilityState {
            bearer: Arc::new(Mutex::new(bearer.clone())),
        };
        let app = Router::new()
            .route("/v2/capabilities", get(authenticated_capabilities))
            .route("/v2/commands", post(authenticated_command))
            .with_state(capability_state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let directory = std::env::temp_dir().join(format!(
            "queenui-unavailable-pair-recovery-{}",
            uuid::Uuid::new_v4()
        ));
        let config = DesktopRunnerConfig {
            mode: "remote".into(),
            url: Some(endpoint.clone()),
            allow_insecure_remote_http: false,
        };
        save_desktop_config(&directory, &config).unwrap();
        let settings = settings_state(directory.clone(), config, "remote", Some(endpoint.clone()));
        {
            let mut active = settings
                .active
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            active.available = false;
            active.identity_generation = None;
        }
        let backend = BackendState::unavailable("startup identity load failed".into());
        let identity = RunnerIdentity {
            version: PAIRING_PAYLOAD_VERSION,
            url: endpoint.clone(),
            cert_fp: "cc".repeat(32),
            bearer,
            generation: 7,
        };

        pair_and_store(
            "unavailable-recovery",
            &settings,
            &backend,
            BackendEvents::noop(),
            move |_| std::future::ready(Ok(identity)),
        )
        .await
        .unwrap();
        let view = set_runner_settings_inner(
            "remote".into(),
            Some(endpoint),
            None,
            None,
            None,
            &settings,
            &backend,
            BackendEvents::noop(),
        )
        .await
        .unwrap();
        assert!(!view.restart_required);

        let dispatch = backend.dispatch_backend().unwrap();
        let super::DispatchTarget::Remote(client) = dispatch.target() else {
            panic!("pairing did not recover the unavailable remote backend");
        };
        client
            .command::<()>(RunnerCommand::ClearDiagnostics)
            .await
            .unwrap();

        drop(dispatch);
        stop_backend_forwarding(&backend).await;
        server.abort();
        let _ = server.await;
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn pair_and_forget_share_the_live_switch_change_gate() {
        let directory =
            std::env::temp_dir().join(format!("queenui-pair-forget-gate-{}", uuid::Uuid::new_v4()));
        let settings = settings_state(
            directory.clone(),
            DesktopRunnerConfig::default(),
            "embedded",
            None,
        );
        let backend = BackendState::unavailable("test unavailable".into());
        let _switch = settings.change_gate.lock().await;

        let pair_error = pair_and_store(
            "blocked-pair",
            &settings,
            &backend,
            BackendEvents::noop(),
            |_| {
                std::future::ready(Ok(RunnerIdentity {
                    version: PAIRING_PAYLOAD_VERSION,
                    url: "https://runner.example".into(),
                    cert_fp: "dd".repeat(32),
                    bearer: "blocked-pair-bearer-long-enough".into(),
                    generation: 1,
                }))
            },
        )
        .await
        .unwrap_err();
        assert_eq!(pair_error, super::SWITCHING_RUNNERS_ERROR);

        let forget_error = forget_runner_credential_inner(&settings).unwrap_err();
        assert_eq!(forget_error, super::SWITCHING_RUNNERS_ERROR);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn runner_connection_payload_matches_the_frontend_contract() {
        assert_eq!(RUNNER_CONNECTION_EVENT, "queenui://runner-connection");
        let value = serde_json::to_value(RunnerConnectionEvent {
            backend_generation: 7,
            state: RunnerConnectionState::Reconnecting,
            attempt: 3,
            last_ok_ts: Some(42),
            detail: Some("closed".into()),
        })
        .unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "backendGeneration": 7,
                "state": "reconnecting",
                "attempt": 3,
                "lastOkTs": 42,
                "detail": "closed"
            })
        );
        assert_eq!(
            serde_json::to_value(embedded_connection_event(8)).unwrap(),
            serde_json::json!({
                "backendGeneration": 8,
                "state": "embedded",
                "attempt": 0,
                "lastOkTs": null,
                "detail": null
            })
        );
    }

    #[test]
    fn core_event_envelopes_match_the_frontend_contract() {
        assert_eq!(
            serde_json::to_value(BackendSnapshotEvent {
                backend_generation: 11,
                payload: models::AppSnapshot::default(),
            })
            .unwrap(),
            serde_json::json!({
                "backendGeneration": 11,
                "payload": {
                    "engines": [],
                    "accounts": [],
                    "runtimes": [],
                    "games": [],
                    "campaigns": [],
                    "campaignRuntimes": []
                }
            })
        );
        assert_eq!(
            serde_json::to_value(BackendNotificationEvent {
                backend_generation: 11,
                payload: (),
            })
            .unwrap(),
            serde_json::json!({
                "backendGeneration": 11,
                "payload": null
            })
        );
    }

    #[tokio::test]
    async fn direct_snapshot_fetch_uses_the_active_backend_generation_envelope() {
        let directory =
            std::env::temp_dir().join(format!("queenui-stamped-fetch-{}", uuid::Uuid::new_v4()));
        let core = AppState::new(directory.clone(), models::AppConfig::default()).unwrap();
        let backend = BackendState::active(ActiveBackend::Embedded(
            EmbeddedBackend::new_at_generation(core, BackendEvents::noop(), 7),
        ));

        let snapshot = get_snapshot_inner(&backend).await.unwrap();
        assert_eq!(snapshot.backend_generation, 7);
        assert_eq!(
            serde_json::to_value(snapshot.payload).unwrap(),
            serde_json::to_value(models::AppSnapshot::default()).unwrap()
        );

        stop_backend_forwarding(&backend).await;
        let _ = fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn remote_forwarding_loop_emits_heartbeat_disconnect_retry_recovery_and_diagnostic() {
        assert_eq!(REMOTE_EVENT_HEARTBEAT, Duration::from_secs(5));
        let server_release = CancellationToken::new();
        let script = EventScript {
            attempts: Arc::new(AtomicUsize::new(0)),
            release: server_release.clone(),
        };
        let app = Router::new()
            .route("/v2/events", get(scripted_events))
            .with_state(script);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = queen_client::RunnerClient::from_identity(RunnerIdentity {
            version: PAIRING_PAYLOAD_VERSION,
            url: format!("http://{address}"),
            cert_fp: String::new(),
            bearer: "x".repeat(32),
            generation: 1,
        })
        .unwrap();
        let diagnostic_root = std::env::temp_dir().join(format!(
            "queenui-forwarder-diagnostics-{}",
            uuid::Uuid::new_v4()
        ));
        let diagnostic_log = diagnostics::global().unwrap_or_else(|| {
            diagnostics::install(diagnostics::DiagnosticsLog::load(&diagnostic_root))
        });
        let diagnostics_before: std::collections::HashSet<_> = diagnostic_log
            .recent(&diagnostics::DiagnosticFilter::default())
            .into_iter()
            .map(|entry| entry.id)
            .collect();
        let cancellation = CancellationToken::new();
        let connections = Arc::new(Mutex::new(Vec::<RunnerConnectionEvent>::new()));
        let core_events = Arc::new(Mutex::new(Vec::<String>::new()));
        let connection_sink = connections.clone();
        let core_sink = core_events.clone();
        let loop_cancellation = cancellation.clone();
        let forwarder = tokio::spawn(async move {
            run_remote_event_loop(
                client,
                loop_cancellation,
                Duration::from_millis(10),
                Duration::from_millis(5),
                Duration::from_millis(20),
                9,
                move |event| connection_sink.lock().unwrap().push(event),
                move |event| {
                    core_sink.lock().unwrap().push(
                        match event {
                            CoreEvent::Snapshot(_) => "snapshot",
                            CoreEvent::LogsUpdated => "logs",
                            CoreEvent::HistoryUpdated => "history",
                        }
                        .into(),
                    )
                },
            )
            .await;
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let recovered = connections.lock().unwrap().iter().any(|event| {
                    event.state == RunnerConnectionState::Connected && event.attempt >= 3
                });
                if recovered && core_events.lock().unwrap().len() >= 2 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        cancellation.cancel();
        forwarder.await.unwrap();
        server_release.cancel();
        server.abort();

        let connections = connections.lock().unwrap();
        assert!(
            connections
                .iter()
                .all(|event| event.backend_generation == 9),
            "every connection transition must retain its backend provenance"
        );
        assert!(connections.iter().any(|event| {
            event.state == RunnerConnectionState::Reconnecting
                && event.attempt == 1
                && event.detail.as_deref() == Some("Waiting for the runner event connection")
        }));
        assert!(connections
            .iter()
            .any(|event| event.state == RunnerConnectionState::Disconnected));
        assert!(connections.iter().any(|event| {
            event.state == RunnerConnectionState::Connected
                && event.attempt >= 3
                && event.last_ok_ts.is_some()
        }));
        assert_eq!(&*core_events.lock().unwrap(), &["snapshot", "logs"]);
        let new_diagnostics: Vec<_> = diagnostic_log
            .recent(&diagnostics::DiagnosticFilter {
                source: Some("runner".into()),
                ..diagnostics::DiagnosticFilter::default()
            })
            .into_iter()
            .filter(|entry| !diagnostics_before.contains(&entry.id))
            .collect();
        assert!(new_diagnostics.iter().all(|entry| {
            entry.level == "warn" && entry.message == "Remote runner event connection failed"
        }));
        assert!(new_diagnostics.iter().any(|entry| {
            entry
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("closed"))
        }));
        assert!(new_diagnostics.iter().any(|entry| {
            entry
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("503"))
        }));
        let _ = std::fs::remove_dir_all(diagnostic_root);
    }

    #[test]
    fn snapshot_error_boundary_never_exposes_a_secret() {
        let secret = "bearer-enroll-super-secret";
        let raw = format!("request failed Authorization: Bearer {secret} enroll={secret}");
        let safe = operator_safe_snapshot_error();
        assert!(!safe.contains(secret));
        assert!(!safe.contains(&raw));
    }

    #[test]
    fn remote_snapshot_exposes_the_content_identity_not_the_runner_data_directory() {
        let identity = "a".repeat(64);
        let snapshot = models::AppSnapshot {
            engines: vec![models::EngineProfile {
                id: "engine".into(),
                name: "Trusted".into(),
                path: format!("/home/operator/.local/share/queenui/engine-store/{identity}"),
                author: None,
                option_count: 0,
                last_probed_at_ms: None,
                probe_ok: None,
                options: Vec::new(),
                opening_book: None,
            }],
            ..models::AppSnapshot::default()
        };
        let redacted = redact_remote_snapshot(snapshot);
        assert_eq!(redacted.engines[0].path, format!("engine-store/{identity}"));
        assert!(!redacted.engines[0].path.contains("operator"));
    }

    #[test]
    fn close_payload_uses_reported_count_contract() {
        assert_eq!(
            serde_json::to_value(CloseRequestedPayload { reported_count: 3 }).unwrap(),
            serde_json::json!({ "reportedCount": 3 })
        );
    }

    #[test]
    fn ssh_alias_is_data_not_an_option_or_assignment() {
        assert_eq!(validate_ssh_alias("runner-host").unwrap(), "runner-host");
        for invalid in ["-oProxyCommand=bad", "host name", "host=command", ""] {
            assert!(validate_ssh_alias(invalid).is_err());
        }
    }
}
