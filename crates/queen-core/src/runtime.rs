use crate::{
    campaign, diagnostics, enginelog, history, lichess, models, opening_book, storage, uci,
};

use diagnostics::DiagnosticEntry;
use futures_util::{FutureExt, StreamExt};
use models::{
    AccountProfile, AddAccountRequest, AddAccountResult, AppConfig, AppSnapshot, BotRuntime,
    CampaignRuntime, CampaignSettings, ChallengeRequest, ChallengeResult, EngineOptionUpdate,
    EngineProfile, LiveGame, OpeningBookConfig, OpeningBookUpdate,
};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    panic::AssertUnwindSafe,
    path::PathBuf,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering},
        Arc, RwLock as StdRwLock,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    sync::{broadcast, mpsc, watch, Mutex, RwLock, Semaphore},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

/// How long app diagnostics are kept. Independent of the engine-log caps: this
/// is a low-volume operational record, not a transcript archive.
const DIAGNOSTICS_MAX_AGE_DAYS: u32 = 90;

#[derive(Clone)]
pub struct AppState(pub(crate) Arc<AppStateInner>);

/// Exclusive fence over supervisor and game-task reservations. Dropping the
/// guard reopens core admission, including on unwind.
pub struct CoreQuiesceGuard {
    state: AppState,
    ownership: Option<tokio::sync::OwnedRwLockWriteGuard<()>>,
}

impl CoreQuiesceGuard {
    /// Counts authoritative automation ownership while new reservations are
    /// fenced. The union includes the short pre-snapshot reservation window.
    pub async fn live_game_count(&self) -> usize {
        self.state.live_game_ownership_count().await
    }

    /// Refuses locally-known challenge ownership before the network checks.
    /// Successful manual and campaign challenge POSTs remain tracked until an
    /// account event proves that they resolved.
    pub async fn locally_known_outgoing_challenge_error(&self) -> Option<String> {
        self.state.local_outgoing_challenge_error(true).await
    }

    /// Durable uncertainty and campaign-local ownership cannot be repaired by
    /// the switch verifier's known-map pass. Known entries are deliberately
    /// excluded so the authoritative outgoing read gets a chance to prune a
    /// missed resolution event before the final verdict.
    pub async fn locally_unverifiable_outgoing_challenge_error(&self) -> Option<String> {
        self.state.local_outgoing_challenge_error(false).await
    }

    /// Reopens admission after a refused switch, then reconciles every enabled
    /// account before returning control to the restored embedded backend.
    pub async fn restore(mut self) {
        self.ownership.take();
        self.state.0.quiescing.store(false, Ordering::Release);
        self.state.reconcile_enabled_accounts_after_quiesce().await;
    }

    /// Checks Lichess after local reservations and non-repairable challenge
    /// state are clear. The write guard keeps every local challenge/game
    /// producer fenced for the full verification window.
    pub async fn verify_authoritative_handover(&self) -> Result<(), String> {
        let accounts: Vec<_> = self
            .state
            .0
            .config
            .read()
            .await
            .accounts
            .iter()
            .filter(|account| account.enabled)
            .cloned()
            .collect();
        let mut authoritative = Vec::with_capacity(accounts.len());
        for account in accounts {
            let token = self.state.token(&account.id).map_err(|error| {
                diagnostics::record(
                    DiagnosticEntry::warn(
                        "lichess",
                        "Could not verify an enabled account before switching runners",
                    )
                    .with_account(&account.id)
                    .with_detail(error),
                );
                authoritative_handover_verification_error(&account.username)
            })?;
            let _api_gate = self.state.0.matchmaking_api_gate.lock().await;
            let (outgoing, games) = tokio::join!(
                lichess::outgoing_challenges(
                    &self.state.0.api_base,
                    &self.state.0.api_client,
                    &token,
                ),
                lichess::ongoing_game_ids(&self.state.0.api_base, &self.state.0.api_client, &token,),
            );
            let outgoing = outgoing.map_err(|error| {
                diagnostics::record(
                    DiagnosticEntry::warn(
                        "lichess",
                        "Could not verify outgoing challenges before switching runners",
                    )
                    .with_account(&account.id)
                    .with_detail(error.to_string()),
                );
                authoritative_handover_verification_error(&account.username)
            })?;
            let games = games.map_err(|error| {
                diagnostics::record(
                    DiagnosticEntry::warn(
                        "lichess",
                        "Could not verify live games before switching runners",
                    )
                    .with_account(&account.id)
                    .with_detail(error.to_string()),
                );
                authoritative_handover_verification_error(&account.username)
            })?;
            self.state
                .reconcile_known_outgoing_challenges(&account.id, &outgoing, &games)
                .await;
            authoritative.push((account, outgoing, games));
        }

        // The two Lichess reads can straddle a challenge-to-game transition.
        // A quiesced gameStart preserves both its known entry and durable game
        // intent, so re-reading the maps after every network response closes
        // that local side of the race.
        let local_error = self.locally_known_outgoing_challenge_error().await;
        for (account, outgoing, games) in authoritative {
            if !games.is_empty() {
                let noun = if games.len() == 1 { "game" } else { "games" };
                return Err(format!(
                    "Lichess account {} still has {} live {noun} ({}); finish or resign them before switching to a runner.",
                    account.username,
                    games.len(),
                    games.join(", ")
                ));
            }
            if !outgoing.is_empty() {
                let noun = if outgoing.len() == 1 {
                    "challenge"
                } else {
                    "challenges"
                };
                let mut opponents: Vec<_> = outgoing
                    .iter()
                    .map(|challenge| challenge.opponent.clone())
                    .collect();
                opponents.sort();
                return Err(format!(
                    "Lichess account {} still has {} outgoing {noun} ({}); cancel them or let them resolve before switching to a runner.",
                    account.username,
                    outgoing.len(),
                    opponents.join(", ")
                ));
            }
        }
        if let Some(error) = local_error {
            return Err(error);
        }
        Ok(())
    }

    /// Drains an already-quiesced core. Supervisor/game handles are removed
    /// while the exclusive reservation guard is held, then joined only after
    /// the guard is released. The atomic quiesce flag keeps admission closed.
    pub async fn shutdown(mut self) -> Result<(), String> {
        let handles = self.state.take_shutdown_task_handles().await;
        self.ownership.take();
        let joins = futures_util::future::join_all(
            handles
                .into_iter()
                .map(|(handle, label)| join_owned_task(handle, label)),
        );
        let cleanup = self.state.shutdown_after_admission_closed();
        let (task_results, cleanup_result) = tokio::join!(joins, cleanup);
        let mut errors: Vec<_> = task_results.into_iter().filter_map(Result::err).collect();
        if let Err(error) = cleanup_result {
            errors.push(error);
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
}

fn authoritative_handover_verification_error(username: &str) -> String {
    format!(
        "Could not verify Lichess account {username} before switching runners; live games or outgoing challenges may still exist."
    )
}

impl Drop for CoreQuiesceGuard {
    fn drop(&mut self) {
        self.state.0.quiescing.store(false, Ordering::Release);
    }
}

/// Borrowed core state used by the transport-neutral command functions below.
/// Adapters construct this from either Tauri-managed state or runner state.
pub struct CoreStateRef<'a> {
    state: &'a AppState,
}

impl<'a> CoreStateRef<'a> {
    pub fn new(state: &'a AppState) -> Self {
        Self { state }
    }

    pub fn inner(&self) -> &AppState {
        self.state
    }
}

impl std::ops::Deref for CoreStateRef<'_> {
    type Target = AppState;

    fn deref(&self) -> &Self::Target {
        self.state
    }
}

/// Domain notifications consumed by either the desktop adapter or the remote
/// runner transport. The core never knows which one is attached.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", content = "payload", rename_all = "camelCase")]
pub enum CoreEvent {
    Snapshot(AppSnapshot),
    LogsUpdated,
    HistoryUpdated,
}

/// Games are keyed by (account id, game id): when two locally managed bots play
/// each other, both accounts receive a gameStart for the same game id and each
/// needs its own independent entry.
type GameKey = (String, String);

const TASK_JOIN_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) struct SupervisorTask {
    generation: u64,
    cancellation: CancellationToken,
    handle: Option<JoinHandle<()>>,
}

pub(crate) struct GameTask {
    generation: u64,
    cancellation: CancellationToken,
    handle: Option<JoinHandle<()>>,
}

#[cfg(feature = "test-support")]
enum TestGameCommand {
    Submit(tokio::sync::oneshot::Sender<Result<(), String>>),
}

/// Test-only control surface for a production-shaped owned game task and its
/// move submission coordinator.
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub struct TestGameTaskProbe {
    commands: mpsc::Sender<TestGameCommand>,
    game_cancellation: CancellationToken,
    game_stopped: CancellationToken,
    campaign_stopped: CancellationToken,
}

#[cfg(feature = "test-support")]
impl TestGameTaskProbe {
    pub async fn submit_move(&self) -> Result<(), String> {
        let (reply, response) = tokio::sync::oneshot::channel();
        self.commands
            .send(TestGameCommand::Submit(reply))
            .await
            .map_err(|_| "The test game task has stopped".to_string())?;
        response
            .await
            .map_err(|_| "The test game task stopped before submitting".to_string())?
    }

    pub fn game_is_stopped(&self) -> bool {
        self.game_stopped.is_cancelled()
    }

    pub fn game_cancellation_requested(&self) -> bool {
        self.game_cancellation.is_cancelled()
    }

    pub fn campaign_is_stopped(&self) -> bool {
        self.campaign_stopped.is_cancelled()
    }
}

#[cfg(feature = "test-support")]
struct TestMoveTransport {
    submissions: watch::Sender<u64>,
}

#[cfg(feature = "test-support")]
impl MoveTransport for TestMoveTransport {
    fn submit<'a>(
        &'a self,
        _game_id: &'a str,
        _chess_move: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), lichess::LichessError>> + Send + 'a>> {
        Box::pin(async move {
            self.submissions.send_modify(|count| *count += 1);
            Ok(())
        })
    }
}

async fn join_owned_task(mut handle: JoinHandle<()>, label: &str) -> Result<(), String> {
    match tokio::time::timeout(TASK_JOIN_TIMEOUT, &mut handle).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(format!("The {label} failed while joining: {error}")),
        Err(_) => {
            handle.abort();
            let _ = handle.await;
            Err(format!(
                "The {label} did not exit within {} seconds and was aborted",
                TASK_JOIN_TIMEOUT.as_secs()
            ))
        }
    }
}

fn panic_detail(panic: Box<dyn std::any::Any + Send>) -> String {
    panic
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| panic.downcast_ref::<&str>().map(|value| value.to_string()))
        .unwrap_or_else(|| "unknown panic payload".into())
}

fn spawn_supervisor_wrapper<F>(
    state: AppState,
    account_id: String,
    generation: u64,
    inner: F,
) -> JoinHandle<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        let outcome = AssertUnwindSafe(inner).catch_unwind().await;
        if let Err(panic) = outcome {
            let detail = panic_detail(panic);
            if state.supervisor_is_current(&account_id, generation).await {
                state
                    .set_runtime(
                        &account_id,
                        "error",
                        Some(format!("Account supervisor panicked: {detail}")),
                    )
                    .await;
                diagnostics::record(
                    DiagnosticEntry::error("app", "Account supervisor panicked")
                        .with_account(&account_id)
                        .with_detail(detail),
                );
            }
        }
    })
}

fn spawn_game_wrapper<F>(
    state: AppState,
    task_account: AccountProfile,
    task_key: GameKey,
    task_cancellation: CancellationToken,
    generation: u64,
    inner: F,
) -> JoinHandle<()>
where
    F: Future<Output = Result<(), String>> + Send + 'static,
{
    tokio::spawn(async move {
        if !state
            .supervisor_is_current(&task_account.id, generation)
            .await
        {
            return;
        }
        state.set_runtime(&task_account.id, "playing", None).await;
        let outcome = AssertUnwindSafe(inner).catch_unwind().await;
        state.0.active_games.lock().await.remove(&task_key);
        let failure = match outcome {
            Ok(result) => result.err(),
            Err(panic) => Some(format!("Game task panicked: {}", panic_detail(panic))),
        };
        if let Some(error) = failure.clone() {
            let mut games = state.0.games.write().await;
            if let Some(game) = games.get_mut(&task_key) {
                game.status = "error".into();
                game.error = Some(error.clone());
            }
            prune_finished_games(&mut games);
            diagnostics::record(
                DiagnosticEntry::error("app", "A supervised game task failed")
                    .with_account(&task_account.id)
                    .with_game(&task_key.1)
                    .with_detail(error.clone()),
            );
        }
        if !task_cancellation.is_cancelled()
            && state
                .supervisor_is_current(&task_account.id, generation)
                .await
        {
            let still_playing = state.0.games.read().await.values().any(|game| {
                game.account_id == task_account.id
                    && (game.status == "started" || game.status == "created")
            });
            state
                .set_runtime(
                    &task_account.id,
                    if still_playing { "playing" } else { "online" },
                    failure,
                )
                .await;
        }
        state.emit_snapshot().await;
    })
}

async fn persist_intents(
    path: PathBuf,
    mut intents: Vec<storage::ActiveGameIntent>,
) -> Result<(), String> {
    intents.sort_by(|left, right| {
        (&left.account_id, &left.game_id).cmp(&(&right.account_id, &right.game_id))
    });
    tokio::task::spawn_blocking(move || storage::save_active_game_intents(&path, &intents))
        .await
        .map_err(|error| format!("Active-game recovery writer failed: {error}"))?
}

async fn persist_uncertain_challenge_creations(
    path: PathBuf,
    mut creations: Vec<storage::UncertainChallengeCreation>,
) -> Result<(), String> {
    creations.sort_by(|left, right| left.account_id.cmp(&right.account_id));
    tokio::task::spawn_blocking(move || {
        storage::save_uncertain_challenge_creations(&path, &creations)
    })
    .await
    .map_err(|error| format!("Challenge-creation recovery writer failed: {error}"))?
}

pub(crate) struct AppStateInner {
    /// Held for the full authority lifetime. If acquisition fails AppState is
    /// never constructed and no automation can start.
    pub(crate) _authority: storage::DataDirLock,
    pub(crate) events: broadcast::Sender<CoreEvent>,
    pub(crate) secrets: Arc<dyn storage::SecretStore>,
    pub(crate) config_path: PathBuf,
    pub(crate) config: RwLock<AppConfig>,
    pub(crate) runtimes: RwLock<HashMap<String, BotRuntime>>,
    pub(crate) games: RwLock<HashMap<GameKey, LiveGame>>,
    /// Set before taking the exclusive ownership gate so producers arriving
    /// behind an in-progress reservation fail instead of queueing behind it.
    pub(crate) quiescing: AtomicBool,
    pub(crate) ownership_admission: Arc<RwLock<()>>,
    pub(crate) supervisors: Mutex<HashMap<String, SupervisorTask>>,
    /// Serializes Start/Stop for one account without delaying unrelated bots.
    /// The supervisor/game reservations remain installed for the same span.
    pub(crate) bot_transitions: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    pub(crate) supervisor_generation: AtomicU64,
    pub(crate) active_games: Mutex<HashSet<GameKey>>,
    pub(crate) game_tasks: Mutex<HashMap<GameKey, GameTask>>,
    pub(crate) active_intents_path: PathBuf,
    pub(crate) active_intents: Mutex<HashSet<storage::ActiveGameIntent>>,
    pub(crate) campaign_runtimes: RwLock<HashMap<String, CampaignRuntime>>,
    pub(crate) campaign_tasks: Mutex<HashMap<String, campaign::CampaignTask>>,
    pub(crate) campaign_generation: AtomicU64,
    pub(crate) challenge_outcomes: Mutex<HashMap<String, HashSet<String>>>,
    pub(crate) uncertain_challenge_creations_path: PathBuf,
    pub(crate) uncertain_challenge_creations: Mutex<HashMap<String, String>>,
    pub(crate) known_outgoing_challenges: Mutex<HashMap<GameKey, String>>,
    pub(crate) matchmaking_api_gate: Mutex<()>,
    pub(crate) opening_books: Mutex<HashMap<String, Arc<opening_book::OpeningBook>>>,
    pub(crate) engine_governor: StdRwLock<uci::EngineGovernor>,
    /// Blocking log work holds its permit inside the worker closure. If a
    /// timed remote request abandons its JoinHandle, the still-running worker
    /// therefore cannot silently escape the aggregate ceiling.
    pub(crate) blocking_workers: StdRwLock<Arc<Semaphore>>,
    pub(crate) history: history::HistoryStore,
    /// How many games are being played right now. Kept as an atomic because
    /// the window's close handler runs on the UI thread and cannot await the
    /// games lock without risking a deadlock against a game task.
    pub(crate) live_games: AtomicUsize,
    /// The complete UCI conversation of every game, on disk.
    pub(crate) logs: enginelog::EngineLogStore,
    /// QueenUI's own operational events. Process-global so pure helpers can
    /// record without threading a handle through them.
    pub(crate) diagnostics: &'static diagnostics::DiagnosticsLog,
    /// Untimed client, used only for long-lived NDJSON streams.
    pub(crate) client: Client,
    /// Client with a total request timeout, used for every unary API call so a
    /// stalled response can never hang a game task forever.
    pub(crate) api_client: Client,
    /// Injected only by loopback acceptance tests. Production always uses the
    /// fixed HTTPS Lichess origin returned by `default_api_base`.
    pub(crate) api_base: Url,
}

impl AppState {
    pub fn load(data_dir: PathBuf) -> Result<Self, String> {
        Self::load_with_secret_store(data_dir, Arc::new(storage::PlatformSecretStore))
    }

    pub fn load_with_secret_store(
        data_dir: PathBuf,
        secrets: Arc<dyn storage::SecretStore>,
    ) -> Result<Self, String> {
        let authority = storage::DataDirLock::acquire(&data_dir)?;
        if diagnostics::global().is_none() {
            diagnostics::install(diagnostics::DiagnosticsLog::load(&data_dir));
        }
        let config = storage::load(&storage::config_path(&data_dir))?;
        Self::new_with_authority(data_dir, config, secrets, authority)
    }

    pub fn new(data_dir: PathBuf, config: AppConfig) -> Result<Self, String> {
        Self::new_with_secret_store(data_dir, config, Arc::new(storage::PlatformSecretStore))
    }

    pub fn new_with_secret_store(
        data_dir: PathBuf,
        config: AppConfig,
        secrets: Arc<dyn storage::SecretStore>,
    ) -> Result<Self, String> {
        let authority = storage::DataDirLock::acquire(&data_dir)?;
        Self::new_with_authority(data_dir, config, secrets, authority)
    }

    fn new_with_authority(
        data_dir: PathBuf,
        config: AppConfig,
        secrets: Arc<dyn storage::SecretStore>,
        authority: storage::DataDirLock,
    ) -> Result<Self, String> {
        Self::new_with_authority_and_api(
            data_dir,
            config,
            secrets,
            authority,
            lichess::default_api_base()?,
        )
    }

    fn new_with_authority_and_api(
        data_dir: PathBuf,
        config: AppConfig,
        secrets: Arc<dyn storage::SecretStore>,
        authority: storage::DataDirLock,
        api_base: Url,
    ) -> Result<Self, String> {
        let active_intents_path = storage::active_game_intents_path(&data_dir);
        let active_intents = storage::load_active_game_intents(&active_intents_path)?
            .into_iter()
            .collect();
        let uncertain_challenge_creations_path =
            storage::uncertain_challenge_creations_path(&data_dir);
        let uncertain_challenge_creations =
            storage::load_uncertain_challenge_creations(&uncertain_challenge_creations_path)?
                .into_iter()
                .map(|creation| (creation.account_id, creation.opponent))
                .collect();
        let config_path = storage::config_path(&data_dir);
        let runtimes = config
            .accounts
            .iter()
            .map(|account| {
                (
                    account.id.clone(),
                    BotRuntime {
                        account_id: account.id.clone(),
                        status: "stopped".into(),
                        error: None,
                    },
                )
            })
            .collect();
        let campaign_runtimes = config
            .campaigns
            .iter()
            .map(|settings| {
                (
                    settings.account_id.clone(),
                    CampaignRuntime::stopped(settings.account_id.clone()),
                )
            })
            .collect();
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|error| format!("Could not initialize networking: {error}"))?;
        let api_client = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|error| format!("Could not initialize networking: {error}"))?;
        let history = history::HistoryStore::load(history::HistoryStore::path_in(&data_dir));
        let logs = enginelog::EngineLogStore::load(&data_dir, config.log_retention.clone());
        logs.prune();
        // Normally installed during Tauri setup, before the configuration is
        // even read; the fallback keeps this constructor usable on its own.
        let diagnostics = diagnostics::global()
            .unwrap_or_else(|| diagnostics::install(diagnostics::DiagnosticsLog::load(&data_dir)));
        // Diagnostics keep their own horizon: they are a few entries an hour,
        // not an engine firehose, and tying them to the engine-log age cap
        // would silently discard a month of operational history the moment
        // someone shortened how long transcripts are kept.
        diagnostics.prune(DIAGNOSTICS_MAX_AGE_DAYS);
        let (events, _) = broadcast::channel(64);
        Ok(Self(Arc::new(AppStateInner {
            _authority: authority,
            events,
            secrets,
            config_path,
            config: RwLock::new(config),
            runtimes: RwLock::new(runtimes),
            games: RwLock::new(HashMap::new()),
            quiescing: AtomicBool::new(false),
            ownership_admission: Arc::new(RwLock::new(())),
            supervisors: Mutex::new(HashMap::new()),
            bot_transitions: Mutex::new(HashMap::new()),
            supervisor_generation: AtomicU64::new(0),
            active_games: Mutex::new(HashSet::new()),
            game_tasks: Mutex::new(HashMap::new()),
            active_intents_path,
            active_intents: Mutex::new(active_intents),
            campaign_runtimes: RwLock::new(campaign_runtimes),
            campaign_tasks: Mutex::new(HashMap::new()),
            campaign_generation: AtomicU64::new(0),
            challenge_outcomes: Mutex::new(HashMap::new()),
            uncertain_challenge_creations_path,
            uncertain_challenge_creations: Mutex::new(uncertain_challenge_creations),
            known_outgoing_challenges: Mutex::new(HashMap::new()),
            matchmaking_api_gate: Mutex::new(()),
            opening_books: Mutex::new(HashMap::new()),
            engine_governor: StdRwLock::new(uci::EngineGovernor::default()),
            blocking_workers: StdRwLock::new(Arc::new(Semaphore::new(4))),
            live_games: AtomicUsize::new(0),
            history,
            logs,
            diagnostics,
            client,
            api_client,
            api_base,
        })))
    }

    #[cfg(test)]
    pub(crate) fn new_with_test_api(
        data_dir: PathBuf,
        config: AppConfig,
        secrets: Arc<dyn storage::SecretStore>,
        api_base: Url,
    ) -> Result<Self, String> {
        let authority = storage::DataDirLock::acquire(&data_dir)?;
        Self::new_with_authority_and_api(data_dir, config, secrets, authority, api_base)
    }

    #[cfg(test)]
    pub(crate) fn load_with_test_api(
        data_dir: PathBuf,
        secrets: Arc<dyn storage::SecretStore>,
        api_base: Url,
    ) -> Result<Self, String> {
        let authority = storage::DataDirLock::acquire(&data_dir)?;
        let config = storage::load(&storage::config_path(&data_dir))?;
        Self::new_with_authority_and_api(data_dir, config, secrets, authority, api_base)
    }

    pub async fn snapshot(&self) -> AppSnapshot {
        let config = self.0.config.read().await;
        let runtimes = self.0.runtimes.read().await;
        let games = self.0.games.read().await;
        let campaign_runtimes = self.0.campaign_runtimes.read().await;
        let mut runtimes: Vec<_> = runtimes.values().cloned().collect();
        let mut games: Vec<_> = games.values().cloned().collect();
        let mut campaign_runtimes: Vec<_> = campaign_runtimes.values().cloned().collect();
        runtimes.sort_by(|left, right| left.account_id.cmp(&right.account_id));
        games.sort_by(|left, right| left.id.cmp(&right.id));
        campaign_runtimes.sort_by(|left, right| left.account_id.cmp(&right.account_id));
        // Every state change rebuilds the snapshot, so this is the one place
        // that always knows how many games are in progress.
        self.0.live_games.store(
            games.iter().filter(|game| is_live(game)).count(),
            Ordering::Relaxed,
        );
        AppSnapshot {
            engines: config.engines.clone(),
            accounts: config.accounts.clone(),
            runtimes,
            games,
            campaigns: config.campaigns.clone(),
            campaign_runtimes,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<CoreEvent> {
        self.0.events.subscribe()
    }

    pub fn configure_engine_limits(&self, limits: uci::EngineLimits) -> Result<(), String> {
        let governor = uci::EngineGovernor::new(limits)?;
        *self
            .0
            .engine_governor
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = governor;
        Ok(())
    }

    pub fn configure_blocking_workers(&self, workers: usize) -> Result<(), String> {
        if !(1..=32).contains(&workers) {
            return Err("The blocking worker ceiling must be between 1 and 32".into());
        }
        *self
            .0
            .blocking_workers
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::new(Semaphore::new(workers));
        Ok(())
    }

    pub async fn enforce_engine_log_byte_ceiling(&self, max_bytes: u64) -> Result<(), String> {
        let max_total_mb = (max_bytes / (1024 * 1024)).max(1);
        let retention = {
            let mut config = self.0.config.write().await;
            if config.log_retention.max_total_mb != 0
                && config.log_retention.max_total_mb <= max_total_mb
            {
                return Ok(());
            }
            config.log_retention.max_total_mb = max_total_mb;
            storage::save(&self.0.config_path, &config)?;
            config.log_retention.clone()
        };
        self.0.logs.set_retention(retention);
        Ok(())
    }

    fn engine_governor(&self) -> uci::EngineGovernor {
        self.0
            .engine_governor
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn live_game_count(&self) -> usize {
        self.0.live_games.load(Ordering::Relaxed)
    }

    /// Authoritative in-process ownership used by both quiesce and the remote
    /// handover inventory. Presentation snapshots intentionally do not enter
    /// this count.
    pub async fn live_game_ownership_count(&self) -> usize {
        // Hold every source together so a reservation-to-active transition
        // cannot produce a torn inventory observation.
        let active_games = self.0.active_games.lock().await;
        let game_tasks = self.0.game_tasks.lock().await;
        let active_intents = self.0.active_intents.lock().await;
        let mut games = active_games.clone();
        games.extend(
            game_tasks
                .iter()
                .filter(|(_, task)| {
                    task.handle
                        .as_ref()
                        .is_none_or(|handle| !handle.is_finished())
                })
                .map(|(key, _)| key.clone()),
        );
        games.extend(
            active_intents
                .iter()
                .map(|intent| (intent.account_id.clone(), intent.game_id.clone())),
        );
        games.len()
    }

    /// Counts challenge ownership once per account across definitive known
    /// entries, campaign-local capacity, and the durable uncertain-POST
    /// barrier. These sources overlap by design, so the per-account maximum is
    /// the truthful union count.
    pub async fn outstanding_outgoing_challenge_count(&self) -> usize {
        // Keep the write-ahead, durable, and campaign views stable together;
        // the caller gets one coherent observation even though the live runner
        // can change immediately after this method returns.
        let known = self.0.known_outgoing_challenges.lock().await;
        let uncertain = self.0.uncertain_challenge_creations.lock().await;
        let campaigns = self.0.campaign_runtimes.read().await;
        let mut counts: HashMap<String, usize> = HashMap::new();
        for (account_id, _) in known.keys() {
            *counts.entry(account_id.clone()).or_default() += 1;
        }
        for account_id in uncertain.keys() {
            counts
                .entry(account_id.clone())
                .and_modify(|count| *count = (*count).max(1))
                .or_insert(1);
        }
        for (account_id, runtime) in campaigns.iter() {
            counts
                .entry(account_id.clone())
                .and_modify(|count| *count = (*count).max(runtime.pending_challenges as usize))
                .or_insert(runtime.pending_challenges as usize);
        }
        counts.values().sum()
    }

    async fn local_outgoing_challenge_error(&self, include_known: bool) -> Option<String> {
        let uncertain = self.0.uncertain_challenge_creations.lock().await.clone();
        if let Some((account_id, opponent)) = uncertain.into_iter().next() {
            return Some(format!(
                "An outgoing challenge creation for {account_id} against {opponent} is still uncertain; let QueenUI reconcile it before switching to a runner."
            ));
        }

        if include_known {
            let known: Vec<_> = self
                .0
                .known_outgoing_challenges
                .lock()
                .await
                .values()
                .cloned()
                .collect();
            if known.len() == 1 {
                return Some(format!(
                    "An outgoing challenge to {} is still unresolved; cancel it or let it resolve before switching to a runner.",
                    known[0]
                ));
            }
            if !known.is_empty() {
                let mut opponents = known.clone();
                opponents.sort();
                opponents.dedup();
                return Some(format!(
                    "{} outgoing challenges are still unresolved ({}); cancel them or let them resolve before switching to a runner.",
                    known.len(),
                    opponents.join(", ")
                ));
            }
        }

        let campaign_pending: u32 = self
            .0
            .campaign_runtimes
            .read()
            .await
            .values()
            .map(|runtime| runtime.pending_challenges)
            .sum();
        match campaign_pending {
            0 => None,
            1 => Some(
                "A campaign challenge is still unresolved; cancel it or let it resolve before switching to a runner."
                    .into(),
            ),
            count => Some(format!(
                "{count} campaign challenges are still unresolved; cancel them or let them resolve before switching to a runner."
            )),
        }
    }

    /// Closes supervisor/game reservation admission and waits for any
    /// reservation already in progress to become authoritative.
    pub async fn quiesce(&self) -> CoreQuiesceGuard {
        let ownership = self.0.ownership_admission.clone().write_owned().await;
        self.0.quiescing.store(true, Ordering::Release);
        CoreQuiesceGuard {
            state: self.clone(),
            ownership: Some(ownership),
        }
    }

    /// Test-only seam that seeds the same ownership reservation used by a
    /// gameStart before its first presentation snapshot exists.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn reserve_game_for_test(&self, account_id: &str, game_id: &str) {
        let key = (account_id.to_string(), game_id.to_string());
        self.0.active_games.lock().await.insert(key.clone());
        self.0.game_tasks.lock().await.insert(
            key,
            GameTask {
                generation: 1,
                cancellation: CancellationToken::new(),
                handle: None,
            },
        );
    }

    /// Test-only completed task entry, modeling the interval before the next
    /// account start gets an opportunity to reap it.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub async fn install_finished_game_task_for_test(&self, account_id: &str, game_id: &str) {
        let handle = tokio::spawn(async {});
        while !handle.is_finished() {
            tokio::task::yield_now().await;
        }
        self.0.game_tasks.lock().await.insert(
            (account_id.to_string(), game_id.to_string()),
            GameTask {
                generation: 1,
                cancellation: CancellationToken::new(),
                handle: Some(handle),
            },
        );
    }

    /// Installs the owned-task shape used by a running campaign game without
    /// requiring Lichess or an engine process in runner lifecycle tests.
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub async fn install_running_campaign_game_for_test(
        &self,
        account_id: &str,
        game_id: &str,
    ) -> TestGameTaskProbe {
        let game_key = (account_id.to_string(), game_id.to_string());
        let game_cancellation = CancellationToken::new();
        let task_cancellation = game_cancellation.clone();
        let probe_cancellation = game_cancellation.clone();
        let game_stopped = CancellationToken::new();
        let task_stopped = game_stopped.clone();
        let (commands, mut requests) = mpsc::channel(1);
        let (submissions, mut submitted) = watch::channel(0u64);
        let transport = Arc::new(TestMoveTransport { submissions });
        let task_state = self.clone();
        let task_key = game_key.clone();
        let handle = tokio::spawn(async move {
            let mut coordinator = SubmissionCoordinator::start(
                task_state,
                task_key,
                transport,
                Arc::new(AtomicI64::new(0)),
                &task_cancellation,
            );
            loop {
                tokio::select! {
                    _ = task_cancellation.cancelled() => break,
                    request = requests.recv() => match request {
                        Some(TestGameCommand::Submit(reply)) => {
                            let expected = submitted.borrow().saturating_add(1);
                            let queued = coordinator.submit(String::new(), "e2e4".into()).await;
                            let result = match queued {
                                Ok(()) => tokio::select! {
                                    _ = task_cancellation.cancelled() => {
                                        Err("The test game task stopped before submitting".into())
                                    }
                                    result = submitted.wait_for(|count| *count >= expected) => {
                                        result
                                            .map(|_| ())
                                            .map_err(|_| "The move submission coordinator stopped".into())
                                    }
                                },
                                Err(error) => Err(error),
                            };
                            let _ = reply.send(result);
                        }
                        None => break,
                    },
                }
            }
            let _ = coordinator.shutdown().await;
            task_stopped.cancel();
        });
        self.0.active_games.lock().await.insert(game_key.clone());
        self.0.game_tasks.lock().await.insert(
            game_key,
            GameTask {
                generation: 1,
                cancellation: game_cancellation,
                handle: Some(handle),
            },
        );

        let campaign_cancellation = CancellationToken::new();
        let task_cancellation = campaign_cancellation.clone();
        let campaign_stopped = CancellationToken::new();
        let task_stopped = campaign_stopped.clone();
        self.0.campaign_tasks.lock().await.insert(
            account_id.to_string(),
            campaign::CampaignTask {
                generation: 1,
                cancellation: campaign_cancellation,
                handle: Some(tokio::spawn(async move {
                    task_cancellation.cancelled().await;
                    task_stopped.cancel();
                    Ok(())
                })),
            },
        );
        let mut runtime = CampaignRuntime::stopped(account_id.to_string());
        runtime.status = models::CampaignStatus::Running;
        runtime.active_games = 1;
        self.0
            .campaign_runtimes
            .write()
            .await
            .insert(account_id.to_string(), runtime);

        TestGameTaskProbe {
            commands,
            game_cancellation: probe_cancellation,
            game_stopped,
            campaign_stopped,
        }
    }

    /// Test-only owned task whose join deterministically reports an error.
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub async fn install_failing_supervisor_for_test(&self, account_id: &str) {
        self.0.supervisors.lock().await.insert(
            account_id.to_string(),
            SupervisorTask {
                generation: 1,
                cancellation: CancellationToken::new(),
                handle: Some(tokio::spawn(async {
                    panic!("test supervisor join failure")
                })),
            },
        );
    }

    pub(crate) fn token(&self, account_id: &str) -> Result<String, String> {
        self.0.secrets.get(account_id)
    }

    pub(crate) async fn emit_snapshot(&self) {
        let _ = self
            .0
            .events
            .send(CoreEvent::Snapshot(self.snapshot().await));
    }

    /// Tells the Logs page that the session list changed — one opened, closed,
    /// was deleted, or retention pruned some.
    fn emit_logs_updated(&self) {
        let _ = self.0.events.send(CoreEvent::LogsUpdated);
    }

    pub(crate) async fn set_runtime(&self, account_id: &str, status: &str, error: Option<String>) {
        self.0.runtimes.write().await.insert(
            account_id.to_string(),
            BotRuntime {
                account_id: account_id.to_string(),
                status: status.to_string(),
                error,
            },
        );
        self.emit_snapshot().await;
    }

    async fn bot_transition(&self, account_id: &str) -> Arc<Mutex<()>> {
        self.0
            .bot_transitions
            .lock()
            .await
            .entry(account_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    pub(crate) async fn start_bot(&self, account_id: &str) -> Result<(), String> {
        let transition = self.bot_transition(account_id).await;
        let _transition_guard = transition.lock().await;
        self.start_bot_transition(account_id).await
    }

    /// Out-of-band priority signal for campaign Stop. It cancels only the
    /// campaign controller, leaving the account supervisor and games alive.
    pub async fn interrupt_campaign(&self, account_id: &str) {
        if let Some(task) = self.0.campaign_tasks.lock().await.get(account_id) {
            task.cancellation.cancel();
        }
    }

    /// Out-of-band priority signal for account Stop. It never waits for the
    /// normal Start/Stop transition mutex and cancels all account-owned work.
    pub async fn interrupt_account(&self, account_id: &str) {
        self.interrupt_campaign(account_id).await;
        if let Some(task) = self.0.supervisors.lock().await.get(account_id) {
            task.cancellation.cancel();
        }
        let tasks = self.0.game_tasks.lock().await;
        for task in tasks
            .iter()
            .filter(|((task_account, _), _)| task_account == account_id)
            .map(|(_, task)| task)
        {
            task.cancellation.cancel();
        }
    }

    async fn start_bot_transition(&self, account_id: &str) -> Result<(), String> {
        let finished = {
            let mut supervisors = self.0.supervisors.lock().await;
            match supervisors.get(account_id) {
                Some(task) if task.handle.as_ref().is_some_and(JoinHandle::is_finished) => {
                    supervisors.remove(account_id)
                }
                Some(task) if task.handle.is_none() => {
                    return Err(
                        "This account supervisor is still starting; wait for it to finish".into(),
                    )
                }
                Some(_) => return Ok(()),
                None => None,
            }
        };
        if let Some(mut task) = finished {
            if let Some(handle) = task.handle.take() {
                join_owned_task(handle, "prior account supervisor").await?;
            }
        }
        self.reap_finished_game_tasks(account_id).await?;
        if self
            .0
            .game_tasks
            .lock()
            .await
            .keys()
            .any(|(game_account, _)| game_account == account_id)
        {
            return Err(
                "A prior game generation is still exiting; use Stop and wait before restarting"
                    .into(),
            );
        }

        let (account, engine) = {
            let mut config = self.0.config.write().await;
            let account_index = config
                .accounts
                .iter()
                .position(|account| account.id == account_id)
                .ok_or_else(|| "Lichess account not found".to_string())?;
            let account = config.accounts[account_index].clone();
            let engine = config
                .engines
                .iter()
                .find(|engine| engine.id == account.engine_id)
                .cloned()
                .ok_or_else(|| "The account's engine profile no longer exists".to_string())?;
            if !config.accounts[account_index].enabled {
                config.accounts[account_index].enabled = true;
                storage::save(&self.0.config_path, &config)?;
            }
            (account, engine)
        };
        let token = self.token(account_id)?;
        let cancellation = CancellationToken::new();
        let generation = self
            .0
            .supervisor_generation
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        {
            if self.0.quiescing.load(Ordering::Acquire) {
                return Err(
                    "QueenUI is changing runners; the account supervisor was not started".into(),
                );
            }
            let _admission = self.0.ownership_admission.read().await;
            if self.0.quiescing.load(Ordering::Acquire) {
                return Err(
                    "QueenUI is changing runners; the account supervisor was not started".into(),
                );
            }
            let mut supervisors = self.0.supervisors.lock().await;
            if supervisors.contains_key(account_id) {
                return Ok(());
            }
            supervisors.insert(
                account_id.to_string(),
                SupervisorTask {
                    generation,
                    cancellation: cancellation.clone(),
                    handle: None,
                },
            );
        }
        self.set_runtime(account_id, "connecting", None).await;

        // Reconcile before allowing the account event loop or campaign to
        // create new work. A failed authoritative read leaves the generation
        // stopped and visible instead of guessing that there are no games.
        let ongoing = match tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                Err("The account start was interrupted by priority Stop".to_string())
            }
            result = lichess::ongoing_game_ids(&self.0.api_base, &self.0.api_client, &token) => {
                result.map_err(|error| error.to_string())
            }
        } {
            Ok(games) => games,
            Err(error) => {
                self.0.supervisors.lock().await.remove(account_id);
                let detail = format!("Could not reconcile ongoing games before start: {error}");
                self.set_runtime(account_id, "error", Some(detail.clone()))
                    .await;
                return Err(detail);
            }
        };
        if let Err(error) = self.reconcile_active_intents(account_id, &ongoing).await {
            self.0.supervisors.lock().await.remove(account_id);
            self.set_runtime(account_id, "error", Some(error.clone()))
                .await;
            return Err(error);
        }

        let state = self.clone();
        let supervisor_account = account.clone();
        let supervisor_engine = engine.clone();
        let supervisor_token = token.clone();
        let supervisor_cancellation = cancellation.clone();
        let supervisor_account_id = supervisor_account.id.clone();
        let supervisor_future = run_supervisor(
            state.clone(),
            supervisor_account,
            supervisor_engine,
            supervisor_token,
            supervisor_cancellation,
            generation,
        );
        let mut handle = Some(spawn_supervisor_wrapper(
            state,
            supervisor_account_id,
            generation,
            supervisor_future,
        ));

        let installed = {
            let mut supervisors = self.0.supervisors.lock().await;
            if let Some(task) = supervisors
                .get_mut(account_id)
                .filter(|task| task.generation == generation)
            {
                task.handle = handle.take();
                true
            } else {
                false
            }
        };
        if !installed {
            cancellation.cancel();
            if let Some(handle) = handle {
                let _ = join_owned_task(handle, "canceled account supervisor").await;
            }
            return Err("The account start was canceled before its supervisor launched".into());
        }
        for game_id in ongoing {
            if let Err(error) = self
                .spawn_game_task(
                    account.clone(),
                    engine.clone(),
                    token.clone(),
                    game_id,
                    cancellation.child_token(),
                    generation,
                )
                .await
            {
                // A partially restored generation must not survive a failed
                // startup. Cancel and join everything already installed.
                let _ = self.stop_bot_transition(account_id, false, false).await;
                return Err(error);
            }
        }
        Ok(())
    }

    /// Restores the desired account state after a headless service restart.
    /// Failures stay visible on the account runtime instead of preventing the
    /// runner from serving its control API.
    pub async fn resume_enabled_accounts(&self) {
        let accounts: Vec<(String, bool)> = self
            .0
            .config
            .read()
            .await
            .accounts
            .iter()
            .map(|account| (account.id.clone(), account.enabled))
            .collect();
        for (account_id, _) in accounts.iter().filter(|(_, enabled)| *enabled) {
            if let Err(error) = self.start_bot(account_id).await {
                self.set_runtime(account_id, "error", Some(error)).await;
            }
        }

        // Disabled accounts are never resumed, but a crash or an unresolved
        // operator Stop may have left durable game intent. Reconcile those
        // records too and surface any still-live games rather than silently
        // discarding the recovery state or automating against the operator's
        // disabled choice.
        let disabled_with_intent: HashSet<String> = {
            let intents = self.0.active_intents.lock().await;
            accounts
                .iter()
                .filter(|(_, enabled)| !enabled)
                .filter(|(account_id, _)| {
                    intents
                        .iter()
                        .any(|intent| intent.account_id == *account_id)
                })
                .map(|(account_id, _)| account_id.clone())
                .collect()
        };
        for account_id in disabled_with_intent {
            let result = async {
                let token = self.token(&account_id)?;
                let ongoing =
                    lichess::ongoing_game_ids(&self.0.api_base, &self.0.api_client, &token)
                        .await
                        .map_err(|error| error.to_string())?;
                self.reconcile_active_intents(&account_id, &ongoing).await?;
                Ok::<_, String>(ongoing)
            }
            .await;
            match result {
                Ok(ongoing) if ongoing.is_empty() => {}
                Ok(ongoing) => {
                    let detail = format!(
                        "Automation remains disabled, but Lichess still lists ongoing game(s) requiring operator resolution: {}",
                        ongoing.join(", ")
                    );
                    self.set_runtime(&account_id, "error", Some(detail.clone()))
                        .await;
                    diagnostics::record(
                        DiagnosticEntry::error(
                            "lichess",
                            "Disabled account has reconciled ongoing games",
                        )
                        .with_account(&account_id)
                        .with_detail(detail),
                    );
                }
                Err(error) => {
                    self.set_runtime(
                        &account_id,
                        "error",
                        Some(format!(
                            "Could not reconcile persisted game intent: {error}"
                        )),
                    )
                    .await;
                }
            }
        }
    }

    async fn reconcile_enabled_accounts_after_quiesce(&self) {
        let accounts: Vec<_> = self
            .0
            .config
            .read()
            .await
            .accounts
            .iter()
            .filter(|account| account.enabled)
            .cloned()
            .collect();
        for account in accounts {
            if let Err(error) = self.reconcile_running_account_games(&account).await {
                let detail = format!(
                    "Could not reconcile ongoing games after the runner switch was refused: {error}"
                );
                self.set_runtime(&account.id, "error", Some(detail.clone()))
                    .await;
                diagnostics::record(
                    DiagnosticEntry::error(
                        "lichess",
                        "Could not reconcile a restored embedded account",
                    )
                    .with_account(&account.id)
                    .with_detail(detail),
                );
            }
        }
    }

    async fn reconcile_running_account_games(
        &self,
        account: &AccountProfile,
    ) -> Result<(), String> {
        let supervisor = {
            let supervisors = self.0.supervisors.lock().await;
            supervisors
                .get(&account.id)
                .map(|supervisor| (supervisor.generation, supervisor.cancellation.clone()))
        };
        let Some((generation, cancellation)) = supervisor else {
            return self.start_bot(&account.id).await;
        };
        let engine = self
            .0
            .config
            .read()
            .await
            .engines
            .iter()
            .find(|engine| engine.id == account.engine_id)
            .cloned()
            .ok_or_else(|| "The account's engine profile no longer exists".to_string())?;
        let token = self.token(&account.id)?;
        let ongoing = lichess::ongoing_game_ids(&self.0.api_base, &self.0.api_client, &token)
            .await
            .map_err(|error| error.to_string())?;
        // This rollback path is narrower than process startup: every persisted
        // intent was already reconciled when the running generation started or
        // was just written by a quiesced gameStart. Union it with nowPlaying so
        // a second straddled read cannot consume the one-shot event after the
        // switch has already been refused.
        let mut games: HashSet<_> = ongoing.into_iter().collect();
        games.extend(
            self.0
                .active_intents
                .lock()
                .await
                .iter()
                .filter(|intent| intent.account_id == account.id)
                .map(|intent| intent.game_id.clone()),
        );
        let mut games: Vec<_> = games.into_iter().collect();
        games.sort();
        self.reconcile_active_intents(&account.id, &games).await?;
        for game_id in games {
            self.spawn_game_task(
                account.clone(),
                engine.clone(),
                token.clone(),
                game_id,
                cancellation.child_token(),
                generation,
            )
            .await?;
        }
        Ok(())
    }

    async fn stop_bot(&self, account_id: &str) -> Result<(), String> {
        self.stop_bot_owned(account_id, true, true).await
    }

    async fn stop_bot_owned(
        &self,
        account_id: &str,
        persist_disabled: bool,
        resign_games: bool,
    ) -> Result<(), String> {
        self.interrupt_account(account_id).await;
        let transition = self.bot_transition(account_id).await;
        let _transition_guard = transition.lock().await;
        self.stop_bot_transition(account_id, persist_disabled, resign_games)
            .await
    }

    async fn stop_bot_transition(
        &self,
        account_id: &str,
        persist_disabled: bool,
        resign_games: bool,
    ) -> Result<(), String> {
        self.set_runtime(account_id, "stopping", None).await;
        {
            let mut config = self.0.config.write().await;
            if let Some(account) = config
                .accounts
                .iter_mut()
                .find(|account| account.id == account_id)
            {
                if persist_disabled && account.enabled {
                    account.enabled = false;
                    storage::save(&self.0.config_path, &config)?;
                }
            }
        }
        // Signal every producer first. Cleanup and joins happen below, but no
        // campaign, stream, engine, or HTTP submission gets the head start to
        // create new work while Stop is waiting on another component.
        if let Some(task) = self.0.campaign_tasks.lock().await.get(account_id) {
            task.cancellation.cancel();
        }
        let mut errors = Vec::new();

        let mut handles = Vec::new();
        let supervisor_generation =
            if let Some(task) = self.0.supervisors.lock().await.get_mut(account_id) {
                task.cancellation.cancel();
                if let Some(handle) = task.handle.take() {
                    handles.push((handle, "account supervisor"));
                }
                Some(task.generation)
            } else {
                None
            };

        let game_generations: Vec<(GameKey, u64)> = {
            let mut tasks = self.0.game_tasks.lock().await;
            tasks
                .iter_mut()
                .filter(|((game_account, _), _)| game_account == account_id)
                .map(|(key, task)| {
                    task.cancellation.cancel();
                    if let Some(handle) = task.handle.take() {
                        handles.push((handle, "game task"));
                    }
                    (key.clone(), task.generation)
                })
                .collect()
        };
        let task_joins = futures_util::future::join_all(
            handles
                .into_iter()
                .map(|(handle, label)| join_owned_task(handle, label)),
        );
        let (campaign_result, task_results) =
            tokio::join!(campaign::stop(self, account_id), task_joins);
        if let Err(error) = campaign_result {
            errors.push(error);
        }
        for result in task_results {
            if let Err(error) = result {
                errors.push(error);
            }
        }
        // This is the L-04 fence: the old generation stays reserved until
        // every owned join has completed (or timed out and been aborted).
        // The transition mutex prevents a concurrent Stop from clearing the
        // reservation while this owner is still awaiting those joins.
        if let Some(generation) = supervisor_generation {
            let mut supervisors = self.0.supervisors.lock().await;
            if supervisors
                .get(account_id)
                .is_some_and(|task| task.generation == generation)
            {
                supervisors.remove(account_id);
            }
        }
        let mut tasks = self.0.game_tasks.lock().await;
        for (key, generation) in game_generations {
            if tasks
                .get(&key)
                .is_some_and(|task| task.generation == generation)
            {
                tasks.remove(&key);
            }
        }
        drop(tasks);
        self.0
            .active_games
            .lock()
            .await
            .retain(|(game_account, _)| game_account != account_id);

        // Collect only after the generation fence is released. A game that
        // ended naturally during shutdown has now removed its durable intent.
        let mut live_game_ids: HashSet<String> = self
            .0
            .active_games
            .lock()
            .await
            .iter()
            .filter(|(game_account_id, _)| game_account_id == account_id)
            .map(|(_, game_id)| game_id.clone())
            .collect();
        live_game_ids.extend(
            self.0
                .active_intents
                .lock()
                .await
                .iter()
                .filter(|intent| intent.account_id == account_id)
                .map(|intent| intent.game_id.clone()),
        );

        if resign_games && !live_game_ids.is_empty() {
            let token = match self.token(account_id) {
                Ok(token) => Some(token),
                Err(error) => {
                    errors.push(error);
                    None
                }
            };
            let resignations = token.as_deref().map(|token| {
                futures_util::future::join_all(live_game_ids.iter().map(|game_id| {
                    let client = &self.0.api_client;
                    async move {
                        let result = tokio::time::timeout(
                            Duration::from_secs(5),
                            lichess::resign(&self.0.api_base, client, token, game_id),
                        )
                        .await;
                        (game_id, result)
                    }
                }))
            });
            for (game_id, result) in match resignations {
                Some(resignations) => resignations.await,
                None => Vec::new(),
            } {
                match result {
                    Ok(Ok(())) => {
                        if let Err(error) = self.remove_active_intent(account_id, game_id).await {
                            errors.push(error);
                        }
                    }
                    Ok(Err(error)) => {
                        let detail = format!("Could not resign game {game_id}: {error}");
                        diagnostics::record(
                            DiagnosticEntry::error("lichess", "Bot stop could not resign a game")
                                .with_account(account_id)
                                .with_game(game_id)
                                .with_detail(detail.clone()),
                        );
                        errors.push(detail);
                    }
                    Err(_) => errors.push(format!("Resigning game {game_id} timed out")),
                }
            }
        }
        if errors.is_empty() {
            self.set_runtime(account_id, "stopped", None).await;
            Ok(())
        } else {
            let detail = errors.join("; ");
            self.set_runtime(account_id, "error", Some(detail.clone()))
                .await;
            Err(detail)
        }
    }

    async fn take_shutdown_task_handles(&self) -> Vec<(JoinHandle<()>, &'static str)> {
        for task in self.0.campaign_tasks.lock().await.values() {
            task.cancellation.cancel();
        }
        let mut handles = Vec::new();
        for task in self.0.supervisors.lock().await.values_mut() {
            task.cancellation.cancel();
            if let Some(handle) = task.handle.take() {
                handles.push((handle, "account supervisor"));
            }
        }
        for task in self.0.game_tasks.lock().await.values_mut() {
            task.cancellation.cancel();
            if let Some(handle) = task.handle.take() {
                handles.push((handle, "game task"));
            }
        }
        handles
    }

    /// Runner/application graceful shutdown preserves desired account state and
    /// active-game intents, but drains every campaign, submitter, game, engine,
    /// and supervisor within the same bounded joins used by operator Stop.
    pub async fn shutdown(&self) -> Result<(), String> {
        self.quiesce().await.shutdown().await
    }

    async fn shutdown_after_admission_closed(&self) -> Result<(), String> {
        let mut accounts: HashSet<String> =
            self.0.campaign_tasks.lock().await.keys().cloned().collect();
        accounts.extend(self.0.supervisors.lock().await.keys().cloned());
        accounts.extend(
            self.0
                .game_tasks
                .lock()
                .await
                .keys()
                .map(|(account_id, _)| account_id.clone()),
        );
        // Accounts are independent ownership domains. Draining them in
        // parallel keeps the process-wide shutdown bound equal to one owned
        // join budget rather than N accounts multiplied by that budget.
        let errors: Vec<String> =
            futures_util::future::join_all(accounts.iter().map(|account_id| async move {
                self.stop_bot_owned(account_id, false, false).await.err()
            }))
            .await
            .into_iter()
            .flatten()
            .collect();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }

    async fn supervisor_is_current(&self, account_id: &str, generation: u64) -> bool {
        self.0
            .supervisors
            .lock()
            .await
            .get(account_id)
            .is_some_and(|task| task.generation == generation)
    }

    async fn reconcile_active_intents(
        &self,
        account_id: &str,
        authoritative: &[String],
    ) -> Result<(), String> {
        let mut intents = self.0.active_intents.lock().await;
        let mut next = intents.clone();
        next.retain(|intent| intent.account_id != account_id);
        next.extend(
            authoritative
                .iter()
                .cloned()
                .map(|game_id| storage::ActiveGameIntent {
                    account_id: account_id.to_string(),
                    game_id,
                }),
        );
        let values: Vec<_> = next.iter().cloned().collect();
        persist_intents(self.0.active_intents_path.clone(), values).await?;
        *intents = next;
        Ok(())
    }

    async fn add_active_intent(&self, account_id: &str, game_id: &str) -> Result<(), String> {
        let mut intents = self.0.active_intents.lock().await;
        let mut next = intents.clone();
        next.insert(storage::ActiveGameIntent {
            account_id: account_id.to_string(),
            game_id: game_id.to_string(),
        });
        let values: Vec<_> = next.iter().cloned().collect();
        persist_intents(self.0.active_intents_path.clone(), values).await?;
        *intents = next;
        Ok(())
    }

    async fn remove_active_intent(&self, account_id: &str, game_id: &str) -> Result<(), String> {
        let mut intents = self.0.active_intents.lock().await;
        let mut next = intents.clone();
        next.remove(&storage::ActiveGameIntent {
            account_id: account_id.to_string(),
            game_id: game_id.to_string(),
        });
        let values: Vec<_> = next.iter().cloned().collect();
        persist_intents(self.0.active_intents_path.clone(), values).await?;
        *intents = next;
        Ok(())
    }

    pub(crate) async fn remember_uncertain_challenge_creation(
        &self,
        account_id: &str,
        opponent: &str,
    ) -> Result<(), String> {
        let mut creations = self.0.uncertain_challenge_creations.lock().await;
        let mut next = creations.clone();
        next.insert(account_id.to_string(), opponent.to_string());
        let values = next
            .iter()
            .map(
                |(account_id, opponent)| storage::UncertainChallengeCreation {
                    account_id: account_id.clone(),
                    opponent: opponent.clone(),
                },
            )
            .collect();
        let result = persist_uncertain_challenge_creations(
            self.0.uncertain_challenge_creations_path.clone(),
            values,
        )
        .await;
        *creations = next;
        result
    }

    pub(crate) async fn clear_uncertain_challenge_creation(
        &self,
        account_id: &str,
    ) -> Result<(), String> {
        let mut creations = self.0.uncertain_challenge_creations.lock().await;
        if !creations.contains_key(account_id) {
            return Ok(());
        }
        let mut next = creations.clone();
        next.remove(account_id);
        let values = next
            .iter()
            .map(
                |(account_id, opponent)| storage::UncertainChallengeCreation {
                    account_id: account_id.clone(),
                    opponent: opponent.clone(),
                },
            )
            .collect();
        persist_uncertain_challenge_creations(
            self.0.uncertain_challenge_creations_path.clone(),
            values,
        )
        .await?;
        *creations = next;
        Ok(())
    }

    pub(crate) async fn outgoing_challenge_admission(
        &self,
    ) -> Result<tokio::sync::RwLockReadGuard<'_, ()>, String> {
        const ERROR: &str = "QueenUI is changing runners; the outgoing challenge was not created";
        if self.0.quiescing.load(Ordering::Acquire) {
            return Err(ERROR.into());
        }
        let admission = self.0.ownership_admission.read().await;
        if self.0.quiescing.load(Ordering::Acquire) {
            return Err(ERROR.into());
        }
        Ok(admission)
    }

    pub(crate) async fn remember_known_outgoing_challenge(
        &self,
        account_id: &str,
        challenge_id: &str,
        opponent: &str,
    ) {
        self.0.known_outgoing_challenges.lock().await.insert(
            (account_id.to_string(), challenge_id.to_string()),
            opponent.to_string(),
        );
    }

    /// Installs challenge ownership before the POST. The empty id is safe as a
    /// per-account provisional key because the account-wide matchmaking gate
    /// and durable uncertainty barrier admit only one challenge POST at once.
    pub(crate) async fn remember_pending_outgoing_challenge(
        &self,
        account_id: &str,
        opponent: &str,
    ) {
        self.remember_known_outgoing_challenge(account_id, "", opponent)
            .await;
    }

    /// Atomically promotes the write-ahead entry. If a resolution event already
    /// removed it, the definitive response must not resurrect a phantom.
    pub(crate) async fn finalize_pending_outgoing_challenge(
        &self,
        account_id: &str,
        challenge_id: &str,
        opponent: &str,
    ) {
        let mut known = self.0.known_outgoing_challenges.lock().await;
        if known
            .remove(&(account_id.to_string(), String::new()))
            .is_some()
        {
            known.insert(
                (account_id.to_string(), challenge_id.to_string()),
                opponent.to_string(),
            );
        }
    }

    pub(crate) async fn forget_known_outgoing_challenge(
        &self,
        account_id: &str,
        challenge_id: &str,
    ) {
        self.0
            .known_outgoing_challenges
            .lock()
            .await
            .remove(&(account_id.to_string(), challenge_id.to_string()));
    }

    async fn forget_resolved_outgoing_challenge(
        &self,
        account_id: &str,
        challenge_id: &str,
        opponent: Option<&str>,
    ) {
        let mut known = self.0.known_outgoing_challenges.lock().await;
        known.remove(&(account_id.to_string(), challenge_id.to_string()));
        if let Some(opponent) = opponent {
            known.retain(|(known_account, known_id), known_opponent| {
                known_account != account_id
                    || !known_id.is_empty()
                    || !known_opponent.eq_ignore_ascii_case(opponent)
            });
        }
    }

    pub(crate) async fn reconcile_known_outgoing_challenges(
        &self,
        account_id: &str,
        authoritative: &[lichess::OutgoingChallenge],
        ongoing_game_ids: &[String],
    ) {
        let outgoing_ids: HashSet<_> = authoritative
            .iter()
            .map(|challenge| challenge.id.as_str())
            .collect();
        let ongoing_ids: HashSet<_> = ongoing_game_ids.iter().map(String::as_str).collect();
        let local_intent_ids: HashSet<_> = self
            .0
            .active_intents
            .lock()
            .await
            .iter()
            .filter(|intent| intent.account_id == account_id)
            .map(|intent| intent.game_id.clone())
            .collect();
        let mut known = self.0.known_outgoing_challenges.lock().await;
        known.retain(|(known_account, challenge_id), _| {
            known_account != account_id
                || outgoing_ids.contains(challenge_id.as_str())
                || ongoing_ids.contains(challenge_id.as_str())
                || local_intent_ids.contains(challenge_id)
        });
        for challenge in authoritative {
            known.insert(
                (account_id.to_string(), challenge.id.clone()),
                challenge.opponent.clone(),
            );
        }
    }

    async fn clear_account_challenge_ownership(&self, account_id: &str) -> Result<(), String> {
        self.0
            .known_outgoing_challenges
            .lock()
            .await
            .retain(|(known_account, _), _| known_account != account_id);
        self.clear_uncertain_challenge_creation(account_id).await
    }

    async fn reap_finished_game_tasks(&self, account_id: &str) -> Result<(), String> {
        let finished: Vec<GameTask> = {
            let mut tasks = self.0.game_tasks.lock().await;
            let keys: Vec<_> = tasks
                .iter()
                .filter(|((game_account, _), task)| {
                    game_account == account_id
                        && task.handle.as_ref().is_some_and(JoinHandle::is_finished)
                })
                .map(|(key, _)| key.clone())
                .collect();
            keys.into_iter()
                .filter_map(|key| tasks.remove(&key))
                .collect()
        };
        for mut task in finished {
            if let Some(handle) = task.handle.take() {
                join_owned_task(handle, "finished game task").await?;
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn spawn_game_task(
        &self,
        account: AccountProfile,
        engine: EngineProfile,
        token: String,
        game_id: String,
        cancellation: CancellationToken,
        generation: u64,
    ) -> Result<(), String> {
        let game_key: GameKey = (account.id.clone(), game_id.clone());
        if self.0.quiescing.load(Ordering::Acquire) {
            return self.defer_quiesced_game_start(&account.id, &game_id).await;
        }
        let _admission = self.0.ownership_admission.read().await;
        if self.0.quiescing.load(Ordering::Acquire) {
            return self.defer_quiesced_game_start(&account.id, &game_id).await;
        }
        self.reap_finished_game_tasks(&account.id).await?;
        {
            let supervisors = self.0.supervisors.lock().await;
            if cancellation.is_cancelled()
                || supervisors
                    .get(&account.id)
                    .is_none_or(|task| task.generation != generation)
            {
                return Err(
                    "The account generation ended before the game task was reserved".into(),
                );
            }
            let mut tasks = self.0.game_tasks.lock().await;
            if let Some(existing_generation) = tasks.get(&game_key).map(|task| task.generation) {
                return if existing_generation == generation {
                    Ok(())
                } else {
                    Err("A prior generation still owns this game task".into())
                };
            }
            tasks.insert(
                game_key.clone(),
                GameTask {
                    generation,
                    cancellation: cancellation.clone(),
                    handle: None,
                },
            );
        }
        if !self.0.active_games.lock().await.insert(game_key.clone()) {
            self.0.game_tasks.lock().await.remove(&game_key);
            return Ok(());
        }
        if let Err(error) = self.add_active_intent(&account.id, &game_id).await {
            self.0.active_games.lock().await.remove(&game_key);
            self.0.game_tasks.lock().await.remove(&game_key);
            return Err(error);
        }

        let state = self.clone();
        let task_account = account.clone();
        let task_key = game_key.clone();
        let task_cancellation = cancellation.clone();
        let game_future = run_game(
            state.clone(),
            task_account.clone(),
            engine,
            token,
            game_id,
            task_cancellation.clone(),
        );
        let mut handle = Some(spawn_game_wrapper(
            state,
            task_account,
            task_key,
            task_cancellation,
            generation,
            game_future,
        ));

        let installed = {
            let mut tasks = self.0.game_tasks.lock().await;
            if let Some(task) = tasks
                .get_mut(&game_key)
                .filter(|task| task.generation == generation)
            {
                task.handle = handle.take();
                true
            } else {
                false
            }
        };
        if !installed {
            cancellation.cancel();
            if let Some(handle) = handle {
                let _ = join_owned_task(handle, "canceled game task").await;
            }
            return Err("The game task was canceled before its handle was registered".into());
        }
        Ok(())
    }

    async fn defer_quiesced_game_start(
        &self,
        account_id: &str,
        game_id: &str,
    ) -> Result<(), String> {
        // This disk write is the event acknowledgement boundary: the one-shot
        // gameStart is never refused until its game identity survives a crash.
        self.add_active_intent(account_id, game_id).await?;
        Err(self.record_quiesced_game_start(account_id, game_id))
    }

    fn record_quiesced_game_start(&self, account_id: &str, game_id: &str) -> String {
        let detail =
            "QueenUI is changing runners; gameStart was deferred for startup reconciliation"
                .to_string();
        diagnostics::record(
            DiagnosticEntry::warn("app", "Game start refused while changing runners")
                .with_account(account_id)
                .with_game(game_id)
                .with_detail(detail.clone()),
        );
        detail
    }

    /// Opens the flight recorder for one game. The opponent, our colour, and
    /// the clock are unknown until Lichess sends `gameFull`; they are patched
    /// in later so the engine's handshake still lands in the recording.
    fn open_game_log(
        &self,
        account: &AccountProfile,
        engine_profile: &EngineProfile,
        game_id: &str,
    ) -> Option<enginelog::LogWriter> {
        let options = engine_profile
            .options
            .iter()
            .filter_map(|option| {
                option
                    .value
                    .as_ref()
                    .map(|value| (option.name.clone(), value.clone()))
            })
            .collect();
        self.0.logs.open_session(enginelog::SessionMeta {
            kind: "game".into(),
            game_id: Some(game_id.to_string()),
            account_id: account.id.clone(),
            bot_username: account.username.clone(),
            opponent: None,
            opponent_rating: None,
            engine_id: engine_profile.id.clone(),
            engine_name: engine_profile.name.clone(),
            engine_path: engine_profile.path.clone(),
            color: None,
            clock: None,
            initial_fen: None,
            options,
            book: engine_profile.opening_book.as_ref().map(|book| {
                format!(
                    "{} ({}, max {} plies, top {}% pool)",
                    book.name, book.format, book.max_plies, book.top_move_percent
                )
            }),
            app_version: env!("CARGO_PKG_VERSION").to_string(),
        })
    }

    async fn opening_book(
        &self,
        config: Option<&OpeningBookConfig>,
    ) -> Option<Arc<opening_book::OpeningBook>> {
        let config = config.filter(|book| book.enabled)?.clone();
        if let Some(book) = self.0.opening_books.lock().await.get(&config.path).cloned() {
            return Some(book);
        }
        let cache_path = config.path.clone();
        let loaded = tokio::task::spawn_blocking(move || {
            opening_book::OpeningBook::load(&config).map(Arc::new)
        })
        .await
        .ok()?
        .ok()?;
        self.0
            .opening_books
            .lock()
            .await
            .insert(cache_path, loaded.clone());
        Some(loaded)
    }
}

pub async fn get_snapshot(state: CoreStateRef<'_>) -> Result<AppSnapshot, String> {
    Ok(state.snapshot().await)
}

pub fn write_pgn_file(path: String, contents: String) -> Result<(), String> {
    if !path.to_ascii_lowercase().ends_with(".pgn") {
        return Err("Choose a file with the .pgn extension.".into());
    }
    let requested = PathBuf::from(&path);
    if !requested.is_absolute()
        || requested
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err("Choose an absolute PGN path without \"..\" segments.".into());
    }
    let file_name = requested
        .file_name()
        .ok_or_else(|| "Choose a PGN file name.".to_string())?
        .to_owned();
    let parent = requested
        .parent()
        .ok_or_else(|| "Choose a PGN destination folder.".to_string())?
        .canonicalize()
        .map_err(|error| format!("Could not open the PGN destination folder: {error}"))?;
    let target = parent.join(file_name);
    std::fs::write(&target, contents)
        .map_err(|error| format!("Could not save PGN to {}: {error}", target.display()))
}

pub async fn add_engine(path: String, state: CoreStateRef<'_>) -> Result<EngineProfile, String> {
    let governor = state.engine_governor();
    let requested_path = path.trim();
    let profile = match uci::probe_with_governor(requested_path, &governor).await {
        Ok(profile) => profile,
        Err(probe_error) => {
            let mut config = state.0.config.write().await;
            let Some(engine) = config
                .engines
                .iter_mut()
                .find(|engine| same_engine_executable(&engine.path, requested_path))
            else {
                return Err(probe_error);
            };
            engine.last_probed_at_ms = Some(uci::unix_time_ms());
            engine.probe_ok = Some(false);
            storage::save(&state.0.config_path, &config)?;
            drop(config);
            state.emit_snapshot().await;
            return Err(probe_error);
        }
    };
    let mut config = state.0.config.write().await;
    if let Some(engine) = config
        .engines
        .iter_mut()
        .find(|engine| same_engine_executable(&engine.path, &profile.path))
    {
        engine.last_probed_at_ms = profile.last_probed_at_ms;
        engine.probe_ok = profile.probe_ok;
        storage::save(&state.0.config_path, &config)?;
        drop(config);
        state.emit_snapshot().await;
        return Err("That engine executable is already registered.".into());
    }
    config.engines.push(profile.clone());
    storage::save(&state.0.config_path, &config)?;
    drop(config);
    state.emit_snapshot().await;
    Ok(profile)
}

pub async fn remove_engine(engine_id: String, state: CoreStateRef<'_>) -> Result<(), String> {
    let mut config = state.0.config.write().await;
    if config
        .accounts
        .iter()
        .any(|account| account.engine_id == engine_id)
    {
        return Err(
            "Assign another engine to every account using this profile before removing it.".into(),
        );
    }
    let before = config.engines.len();
    let book_path = config
        .engines
        .iter()
        .find(|engine| engine.id == engine_id)
        .and_then(|engine| engine.opening_book.as_ref())
        .map(|book| book.path.clone());
    config.engines.retain(|engine| engine.id != engine_id);
    if config.engines.len() == before {
        return Err("Engine profile not found".into());
    }
    storage::save(&state.0.config_path, &config)?;
    drop(config);
    if let Some(book_path) = book_path {
        state.0.opening_books.lock().await.remove(&book_path);
        storage::remove_imported_opening_book(
            &state.0.config_path,
            PathBuf::from(book_path).as_path(),
        )?;
    }
    state.emit_snapshot().await;
    Ok(())
}

async fn ensure_engine_idle(state: &AppState, engine_id: &str) -> Result<(), String> {
    let assigned_accounts: Vec<_> = state
        .0
        .config
        .read()
        .await
        .accounts
        .iter()
        .filter(|account| account.engine_id == engine_id)
        .map(|account| account.id.clone())
        .collect();
    let supervisors = state.0.supervisors.lock().await;
    if assigned_accounts
        .iter()
        .any(|account_id| supervisors.contains_key(account_id))
    {
        return Err("Stop every bot using this engine before changing its configuration.".into());
    }
    Ok(())
}

pub async fn update_engine_options(
    engine_id: String,
    options: Vec<EngineOptionUpdate>,
    state: CoreStateRef<'_>,
) -> Result<(), String> {
    ensure_engine_idle(state.inner(), &engine_id).await?;
    let mut config = state.0.config.write().await;
    let engine = config
        .engines
        .iter_mut()
        .find(|engine| engine.id == engine_id)
        .ok_or_else(|| "Engine profile not found".to_string())?;
    for update in options {
        let option = engine
            .options
            .iter_mut()
            .find(|option| option.name == update.name)
            .ok_or_else(|| format!("The engine no longer reports option {}.", update.name))?;
        uci::validate_option_value(option, update.value.as_deref())?;
        option.value = update.value;
    }
    storage::save(&state.0.config_path, &config)?;
    drop(config);
    state.emit_snapshot().await;
    Ok(())
}

pub async fn refresh_engine_options(
    engine_id: String,
    state: CoreStateRef<'_>,
) -> Result<(), String> {
    ensure_engine_idle(state.inner(), &engine_id).await?;
    let path = state
        .0
        .config
        .read()
        .await
        .engines
        .iter()
        .find(|engine| engine.id == engine_id)
        .map(|engine| engine.path.clone())
        .ok_or_else(|| "Engine profile not found".to_string())?;
    let governor = state.engine_governor();
    let probed = match uci::probe_with_governor(&path, &governor).await {
        Ok(probed) => probed,
        Err(probe_error) => {
            let mut config = state.0.config.write().await;
            let engine = config
                .engines
                .iter_mut()
                .find(|engine| engine.id == engine_id)
                .ok_or_else(|| "Engine profile not found".to_string())?;
            engine.last_probed_at_ms = Some(uci::unix_time_ms());
            engine.probe_ok = Some(false);
            storage::save(&state.0.config_path, &config)?;
            drop(config);
            state.emit_snapshot().await;
            return Err(probe_error);
        }
    };
    let mut config = state.0.config.write().await;
    let engine = config
        .engines
        .iter_mut()
        .find(|engine| engine.id == engine_id)
        .ok_or_else(|| "Engine profile not found".to_string())?;
    let existing_values: HashMap<_, _> = engine
        .options
        .iter()
        .filter_map(|option| {
            option
                .value
                .clone()
                .map(|value| (option.name.clone(), value))
        })
        .collect();
    engine.name = probed.name;
    engine.author = probed.author;
    engine.option_count = probed.option_count;
    engine.last_probed_at_ms = probed.last_probed_at_ms;
    engine.probe_ok = probed.probe_ok;
    engine.options = probed
        .options
        .into_iter()
        .map(|mut option| {
            if let Some(value) = existing_values.get(&option.name) {
                if uci::validate_option_value(&option, Some(value)).is_ok() {
                    option.value = Some(value.clone());
                }
            }
            option
        })
        .collect();
    storage::save(&state.0.config_path, &config)?;
    drop(config);
    state.emit_snapshot().await;
    Ok(())
}

#[cfg(test)]
mod engine_probe_truth_tests {
    #[cfg(not(windows))]
    use super::add_engine;
    use super::{refresh_engine_options, AppState, CoreStateRef};
    #[cfg(not(windows))]
    use crate::models::AppConfig;
    use crate::{storage, test_support};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unix_time_ms() -> u64 {
        u64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis(),
        )
        .unwrap()
    }

    fn assert_current_timestamp(timestamp: Option<u64>, before: u64, after: u64) {
        let timestamp = timestamp.expect("probe timestamp");
        assert!(timestamp >= before, "{timestamp} was before {before}");
        assert!(timestamp <= after, "{timestamp} was after {after}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn adding_a_uci_engine_records_a_successful_probe_with_a_current_timestamp() {
        use std::os::unix::fs::PermissionsExt;

        let root = test_support::temp_root("successful-engine-probe");
        let engine_path = root.join("probe-uci.sh");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            &engine_path,
            r#"#!/bin/sh
while IFS= read -r command; do
  case "$command" in
    uci) printf '%s\n' 'id name Probe truth UCI' 'id author QueenUI tests' 'uciok' ;;
    isready) printf '%s\n' 'readyok' ;;
    quit) exit 0 ;;
  esac
done
"#,
        )
        .unwrap();
        std::fs::set_permissions(&engine_path, std::fs::Permissions::from_mode(0o700)).unwrap();
        let state = AppState::new(root.clone(), AppConfig::default()).unwrap();

        let before = unix_time_ms();
        let profile = add_engine(
            engine_path.to_string_lossy().to_string(),
            CoreStateRef::new(&state),
        )
        .await
        .expect("add UCI engine");
        let after = unix_time_ms();

        assert_eq!(profile.probe_ok, Some(true));
        assert_current_timestamp(profile.last_probed_at_ms, before, after);
        let loaded = storage::load(&storage::config_path(&root)).expect("load saved profile");
        assert_eq!(loaded.engines[0].probe_ok, Some(true));
        assert_eq!(
            loaded.engines[0].last_probed_at_ms,
            profile.last_probed_at_ms
        );
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn refreshing_a_missing_engine_records_a_failed_probe_and_keeps_the_profile() {
        let root = test_support::temp_root("failed-engine-probe");
        let missing_path = root.join("missing-uci");
        let state = AppState::new(
            root.clone(),
            test_support::app_config(missing_path.to_str().unwrap(), false),
        )
        .unwrap();

        let before = unix_time_ms();
        let error = refresh_engine_options("engine".into(), CoreStateRef::new(&state))
            .await
            .expect_err("missing engine probe should fail");
        let after = unix_time_ms();

        assert!(error.contains("does not exist"), "{error}");
        let snapshot = state.snapshot().await;
        assert_eq!(snapshot.engines.len(), 1);
        assert_eq!(snapshot.engines[0].name, "Fake UCI");
        assert_eq!(snapshot.engines[0].probe_ok, Some(false));
        assert_current_timestamp(snapshot.engines[0].last_probed_at_ms, before, after);
        let loaded = storage::load(&storage::config_path(&root)).expect("load failed probe");
        assert_eq!(loaded.engines[0].probe_ok, Some(false));
        assert_eq!(
            loaded.engines[0].last_probed_at_ms,
            snapshot.engines[0].last_probed_at_ms
        );
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }
}

pub async fn configure_opening_book(
    request: OpeningBookUpdate,
    state: CoreStateRef<'_>,
) -> Result<OpeningBookConfig, String> {
    if !(1..=200).contains(&request.max_plies) {
        return Err("Opening-book depth must be between 1 and 200 plies.".into());
    }
    if !(1..=100).contains(&request.top_move_percent) {
        return Err("Top-move selection must be between 1% and 100%.".into());
    }
    ensure_engine_idle(state.inner(), &request.engine_id).await?;
    let (existing_name, previous_path) = state
        .0
        .config
        .read()
        .await
        .engines
        .iter()
        .find(|engine| engine.id == request.engine_id)
        .and_then(|engine| engine.opening_book.as_ref())
        .map(|book| {
            (
                (book.path == request.path).then(|| book.name.clone()),
                Some(book.path.clone()),
            )
        })
        .unwrap_or((None, None));
    let config_path = state.0.config_path.clone();
    let source_path = PathBuf::from(request.path.trim());
    let requested_engine_id = request.engine_id.clone();
    let engine_id = requested_engine_id.clone();
    let enabled = request.enabled;
    let max_plies = request.max_plies;
    let top_move_percent = request.top_move_percent;
    let (book, loaded_book) = tokio::task::spawn_blocking(move || {
        let source = source_path.to_string_lossy().to_string();
        let prepared = opening_book::prepare(&source)?;
        let managed_path = storage::import_opening_book(&config_path, &engine_id, &source_path)?;
        let inspection = prepared.inspection.clone();
        let book = OpeningBookConfig {
            enabled,
            path: managed_path.to_string_lossy().to_string(),
            name: existing_name.unwrap_or(inspection.name),
            format: inspection.format,
            max_plies,
            top_move_percent,
            entry_count: inspection.entry_count,
        };
        let loaded = Arc::new(prepared.finish(book.clone()));
        Ok::<_, String>((book, loaded))
    })
    .await
    .map_err(|error| format!("Opening-book import worker failed: {error}"))??;
    let mut config = state.0.config.write().await;
    let engine = config
        .engines
        .iter_mut()
        .find(|engine| engine.id == requested_engine_id)
        .ok_or_else(|| "Engine profile not found".to_string())?;
    engine.opening_book = Some(book.clone());
    storage::save(&state.0.config_path, &config)?;
    drop(config);
    let mut cache = state.0.opening_books.lock().await;
    if let Some(previous_path) = previous_path.as_ref() {
        cache.remove(previous_path);
    }
    cache.insert(book.path.clone(), loaded_book);
    drop(cache);
    if let Some(previous_path) = previous_path.filter(|path| path != &book.path) {
        storage::remove_imported_opening_book(
            &state.0.config_path,
            PathBuf::from(previous_path).as_path(),
        )?;
    }
    state.emit_snapshot().await;
    Ok(book)
}

pub async fn clear_engine_opening_book(
    engine_id: String,
    state: CoreStateRef<'_>,
) -> Result<(), String> {
    ensure_engine_idle(state.inner(), &engine_id).await?;
    let mut config = state.0.config.write().await;
    let engine = config
        .engines
        .iter_mut()
        .find(|engine| engine.id == engine_id)
        .ok_or_else(|| "Engine profile not found".to_string())?;
    let book_path = engine.opening_book.as_ref().map(|book| book.path.clone());
    engine.opening_book = None;
    storage::save(&state.0.config_path, &config)?;
    drop(config);
    if let Some(book_path) = book_path {
        state.0.opening_books.lock().await.remove(&book_path);
        storage::remove_imported_opening_book(
            &state.0.config_path,
            PathBuf::from(book_path).as_path(),
        )?;
    }
    state.emit_snapshot().await;
    Ok(())
}

async fn validate_lichess_token(
    token: &str,
    state: &AppState,
) -> Result<lichess::ValidatedAccount, String> {
    if token.is_empty() {
        return Err("Enter a Lichess API token.".into());
    }
    let validation = lichess::account(&state.0.api_base, &state.0.api_client, token)
        .await
        .map_err(|error| error.to_string())?;
    if validation.account.title.as_deref() != Some("BOT") {
        return Err(format!(
            "@{} is not a Lichess BOT account. QueenUI will not automate moves on a human account.",
            validation.account.username
        ));
    }
    Ok(validation)
}

fn account_scope_result(account: AccountProfile, scopes: Vec<String>) -> AddAccountResult {
    let missing_for_matchmaking = lichess::missing_matchmaking_scopes(&scopes);
    AddAccountResult {
        account,
        can_play_games: !missing_for_matchmaking
            .iter()
            .any(|scope| scope == "bot:play"),
        scopes,
        missing_for_matchmaking,
    }
}

pub async fn add_lichess_account(
    request: AddAccountRequest,
    state: CoreStateRef<'_>,
) -> Result<AddAccountResult, String> {
    let token = request.token.trim();
    if token.is_empty() {
        return Err("Enter a Lichess API token.".into());
    }
    {
        let config = state.0.config.read().await;
        if !config
            .engines
            .iter()
            .any(|engine| engine.id == request.engine_id)
        {
            return Err("Select a registered engine profile.".into());
        }
    }
    let validation = validate_lichess_token(token, state.inner()).await?;
    let lichess_account = validation.account;
    let rating = lichess_account.rating();
    let mut account = AccountProfile {
        id: lichess_account.id,
        username: lichess_account.username,
        engine_id: request.engine_id,
        rating,
        enabled: false,
    };
    state.0.secrets.store(&account.id, token)?;
    let mut config = state.0.config.write().await;
    if let Some(existing) = config
        .accounts
        .iter_mut()
        .find(|item| item.id == account.id)
    {
        account.enabled = existing.enabled;
        *existing = account.clone();
    } else {
        config.accounts.push(account.clone());
    }
    storage::save(&state.0.config_path, &config)?;
    drop(config);
    state
        .0
        .runtimes
        .write()
        .await
        .entry(account.id.clone())
        .or_insert(BotRuntime {
            account_id: account.id.clone(),
            status: "stopped".into(),
            error: None,
        });
    state.emit_snapshot().await;
    Ok(account_scope_result(account, validation.scopes))
}

pub async fn update_lichess_account_token(
    account_id: String,
    token: String,
    state: CoreStateRef<'_>,
) -> Result<AddAccountResult, String> {
    let token = token.trim();
    let account = state
        .0
        .config
        .read()
        .await
        .accounts
        .iter()
        .find(|account| account.id == account_id)
        .cloned()
        .ok_or_else(|| "Lichess account not found".to_string())?;
    let validation = validate_lichess_token(token, state.inner()).await?;
    if !validation.account.id.eq_ignore_ascii_case(&account.id) {
        return Err(format!(
            "The Lichess token belongs to @{} ({}), but the selected account is @{} ({}).",
            validation.account.username, validation.account.id, account.username, account.id
        ));
    }
    state.0.secrets.store(&account.id, token)?;
    Ok(account_scope_result(account, validation.scopes))
}

pub async fn update_account_engine(
    account_id: String,
    engine_id: String,
    state: CoreStateRef<'_>,
) -> Result<(), String> {
    if state.0.supervisors.lock().await.contains_key(&account_id) {
        return Err("Stop the bot before changing its engine.".into());
    }
    let mut config = state.0.config.write().await;
    if !config.engines.iter().any(|engine| engine.id == engine_id) {
        return Err("Engine profile not found".into());
    }
    let account = config
        .accounts
        .iter_mut()
        .find(|account| account.id == account_id)
        .ok_or_else(|| "Lichess account not found".to_string())?;
    account.engine_id = engine_id;
    storage::save(&state.0.config_path, &config)?;
    drop(config);
    state.emit_snapshot().await;
    Ok(())
}

pub async fn remove_lichess_account(
    account_id: String,
    state: CoreStateRef<'_>,
) -> Result<(), String> {
    state.stop_bot(&account_id).await?;
    state.0.secrets.delete(&account_id)?;
    state.clear_account_challenge_ownership(&account_id).await?;
    let mut config = state.0.config.write().await;
    config.accounts.retain(|account| account.id != account_id);
    config
        .campaigns
        .retain(|campaign| campaign.account_id != account_id);
    storage::save(&state.0.config_path, &config)?;
    drop(config);
    state.0.runtimes.write().await.remove(&account_id);
    state.0.campaign_runtimes.write().await.remove(&account_id);
    state
        .0
        .games
        .write()
        .await
        .retain(|_, game| game.account_id != account_id);
    state.emit_snapshot().await;
    Ok(())
}

pub async fn dismiss_game_error(game_id: String, state: CoreStateRef<'_>) -> Result<(), String> {
    let mut games = state.0.games.write().await;
    let key = games
        .iter()
        .find(|(_, game)| game.id == game_id && game.status == "error")
        .map(|(key, _)| key.clone())
        .ok_or_else(|| format!("No retained game error was found for {game_id}."))?;
    games.remove(&key);
    drop(games);
    state.emit_snapshot().await;
    Ok(())
}

pub async fn start_bot(account_id: String, state: CoreStateRef<'_>) -> Result<(), String> {
    state.start_bot(&account_id).await
}

pub async fn stop_bot(account_id: String, state: CoreStateRef<'_>) -> Result<(), String> {
    state.stop_bot(&account_id).await
}

pub async fn start_campaign(
    settings: CampaignSettings,
    state: CoreStateRef<'_>,
) -> Result<(), String> {
    campaign::start(state.inner().clone(), settings).await
}

pub async fn stop_campaign(account_id: String, state: CoreStateRef<'_>) -> Result<(), String> {
    campaign::stop(state.inner(), &account_id).await
}

pub async fn create_challenge(
    request: ChallengeRequest,
    state: CoreStateRef<'_>,
) -> Result<ChallengeResult, String> {
    if request.opponent.trim().is_empty() {
        return Err("Enter a Lichess opponent.".into());
    }
    lichess::validate_username(&request.opponent)?;
    campaign::validate_clock(request.clock_limit, request.clock_increment)?;
    if request.variant != "standard" {
        return Err("QueenUI currently supports automated Standard chess games only.".into());
    }
    state.start_bot(&request.account_id).await?;
    for _ in 0..50 {
        let connected = state
            .0
            .runtimes
            .read()
            .await
            .get(&request.account_id)
            .map(|runtime| runtime.status == "online" || runtime.status == "playing")
            .unwrap_or(false);
        if connected {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let connected = state
        .0
        .runtimes
        .read()
        .await
        .get(&request.account_id)
        .map(|runtime| runtime.status == "online" || runtime.status == "playing")
        .unwrap_or(false);
    if !connected {
        return Err("The bot could not connect to its Lichess event stream. Check the account error and try again.".into());
    }
    let token = state.token(&request.account_id)?;
    let _ownership = state.outgoing_challenge_admission().await?;
    // Acquire the shared API gate outside the timeout so time spent queued
    // behind other operations does not count against the request itself.
    let _gate = state.0.matchmaking_api_gate.lock().await;
    if state
        .0
        .uncertain_challenge_creations
        .lock()
        .await
        .contains_key(&request.account_id)
    {
        let outgoing = lichess::outgoing_challenges(
            &state.0.api_base,
            &state.0.api_client,
            &token,
        )
            .await
            .map_err(|error| {
                lichess::actionable_missing_scope_message(&error).unwrap_or_else(|| {
                    format!(
                        "Challenge creation remains paused until the prior unknown POST is reconciled: {error}"
                    )
                })
            })?;
        state
            .reconcile_known_outgoing_challenges(&request.account_id, &outgoing, &[])
            .await;
        state
            .clear_uncertain_challenge_creation(&request.account_id)
            .await
            .map_err(|error| {
                format!(
                    "Challenge creation remains paused because the reconciled safety barrier could not be updated: {error}"
                )
            })?;
        if let Some(challenge) = outgoing.into_iter().find(|challenge| {
            challenge
                .opponent
                .eq_ignore_ascii_case(request.opponent.trim())
        }) {
            state
                .remember_known_outgoing_challenge(
                    &request.account_id,
                    &challenge.id,
                    &challenge.opponent,
                )
                .await;
            return Ok(ChallengeResult {
                id: challenge.id.clone(),
                status: challenge.status,
                url: lichess::site_url(&challenge.id).map_err(|error| error.to_string())?,
            });
        }
    }
    state
        .remember_uncertain_challenge_creation(&request.account_id, &request.opponent)
        .await
        .map_err(|error| {
            format!(
                "Challenge creation was not sent because its durable safety barrier could not be saved: {error}"
            )
        })?;
    state
        .remember_pending_outgoing_challenge(&request.account_id, &request.opponent)
        .await;
    match lichess::create_challenge(&state.0.api_base, &state.0.api_client, &token, &request).await
    {
        Ok(challenge) => {
            state
                .finalize_pending_outgoing_challenge(
                    &request.account_id,
                    &challenge.id,
                    &request.opponent,
                )
                .await;
            if let Err(error) = state
                .clear_uncertain_challenge_creation(&request.account_id)
                .await
            {
                diagnostics::record(
                    DiagnosticEntry::error(
                        "storage",
                        "Could not clear a definitive challenge-creation barrier",
                    )
                    .with_account(&request.account_id)
                    .with_detail(error),
                );
            }
            Ok(challenge)
        }
        Err(error) if error.ambiguous_write => Err(format!(
            "Challenge POST outcome is unknown; creation is paused until authoritative outgoing challenges are reconciled: {error}"
        )),
        Err(error) => {
            state
                .forget_known_outgoing_challenge(&request.account_id, "")
                .await;
            let persistence_error = state
                .clear_uncertain_challenge_creation(&request.account_id)
                .await
                .err();
            let error = lichess::actionable_missing_scope_message(&error)
                .unwrap_or_else(|| error.to_string());
            Err(format!(
                "{error}{}",
                persistence_error
                    .map(|detail| format!(
                        "; the definitive response was received, but the durable safety barrier could not be cleared: {detail}"
                    ))
                    .unwrap_or_default()
            ))
        }
    }
}

async fn run_supervisor(
    state: AppState,
    account: AccountProfile,
    engine: EngineProfile,
    token: String,
    cancellation: CancellationToken,
    generation: u64,
) {
    let mut retry_delay = 1u64;
    while !cancellation.is_cancelled() {
        let mut rate_limited = false;
        match lichess::event_stream(&state.0.api_base, &state.0.client, &token).await {
            Ok(response) => {
                let connected_at = Instant::now();
                state.set_runtime(&account.id, "online", None).await;
                let mut runtime_errored = false;
                let mut stream = response.bytes_stream();
                let mut buffer = Vec::new();
                let mut received = 0usize;
                loop {
                    tokio::select! {
                        _ = cancellation.cancelled() => return,
                        chunk = stream.next() => {
                            match chunk {
                                Some(Ok(chunk)) => {
                                    let lines = match lichess::append_ndjson_chunk(
                                        &mut buffer,
                                        &chunk,
                                        &mut received,
                                        64 * 1024 * 1024,
                                        "read account event stream",
                                    ) {
                                        Ok(lines) => lines,
                                        Err(error) => {
                                            state.set_runtime(&account.id, "error", Some(error.to_string())).await;
                                            break;
                                        }
                                    };
                                    for line in lines {
                                        match handle_account_event(
                                            &state,
                                            &account,
                                            &engine,
                                            &token,
                                            &cancellation,
                                            generation,
                                            &line,
                                        ).await {
                                            Ok(()) => {
                                                if runtime_errored {
                                                    // A later event succeeded, so clear the
                                                    // sticky "error" status.
                                                    runtime_errored = false;
                                                    let playing = state
                                                        .0
                                                        .active_games
                                                        .lock()
                                                        .await
                                                        .iter()
                                                        .any(|(game_account_id, _)| {
                                                            game_account_id == &account.id
                                                        });
                                                    state
                                                        .set_runtime(
                                                            &account.id,
                                                            if playing { "playing" } else { "online" },
                                                            None,
                                                        )
                                                        .await;
                                                }
                                            }
                                            Err(error) => {
                                                runtime_errored = true;
                                                state.set_runtime(&account.id, "error", Some(error)).await;
                                            }
                                        }
                                    }
                                }
                                Some(Err(error)) => {
                                    state.set_runtime(
                                        &account.id,
                                        "reconnecting",
                                        Some(format!("Lichess event stream interrupted: {error}")),
                                    ).await;
                                    break;
                                }
                                None => break,
                            }
                        }
                    }
                }
                // Only reset backoff when the stream stayed healthy for a while;
                // an instantly dying stream must keep backing off.
                if connected_at.elapsed() > Duration::from_secs(30) {
                    retry_delay = 1;
                }
            }
            Err(error) => {
                rate_limited = error.is_rate_limited();
                state
                    .set_runtime(&account.id, "reconnecting", Some(error.to_string()))
                    .await;
            }
        }

        let delay = if rate_limited {
            retry_delay.max(60)
        } else {
            retry_delay
        };
        tokio::select! {
            _ = cancellation.cancelled() => return,
            _ = tokio::time::sleep(Duration::from_secs(delay)) => {}
        }
        retry_delay = (retry_delay * 2).min(20);
    }
}

async fn handle_account_event(
    state: &AppState,
    account: &AccountProfile,
    engine: &EngineProfile,
    token: &str,
    cancellation: &CancellationToken,
    generation: u64,
    line: &str,
) -> Result<(), String> {
    if line.trim().is_empty() {
        return Ok(());
    }
    let event: Value = serde_json::from_str(line)
        .map_err(|error| format!("Could not read a Lichess event: {error}"))?;
    let event_type = event
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if matches!(
        event_type,
        "challengeDeclined" | "challengeCanceled" | "gameStart" | "gameFinish"
    ) {
        let resolved_challenge_id =
            if event_type == "challengeDeclined" || event_type == "challengeCanceled" {
                event.pointer("/challenge/id").and_then(Value::as_str)
            } else {
                event
                    .pointer("/game/gameId")
                    .or_else(|| event.pointer("/game/id"))
                    .and_then(Value::as_str)
            };
        let resolved_opponent =
            if event_type == "challengeDeclined" || event_type == "challengeCanceled" {
                event
                    .pointer("/challenge/destUser/name")
                    .or_else(|| event.pointer("/challenge/destUser/id"))
                    .and_then(Value::as_str)
            } else {
                event
                    .pointer("/game/opponent/username")
                    .or_else(|| event.pointer("/game/opponent/id"))
                    .and_then(Value::as_str)
            };
        if event_type != "gameStart" {
            if let Some(challenge_id) = resolved_challenge_id {
                state
                    .forget_resolved_outgoing_challenge(
                        &account.id,
                        challenge_id,
                        resolved_opponent,
                    )
                    .await;
            }
        }
        campaign::record_account_event(state, &account.id, event_type, &event).await;
    }
    if event_type == "challengeDeclined"
        || event_type == "challengeCanceled"
        || event_type == "gameFinish"
    {
        return Ok(());
    }
    if event_type == "challenge" {
        decline_incoming_challenge(state, account, token, &event).await;
        return Ok(());
    }
    if event_type != "gameStart" {
        return Ok(());
    }
    let game_id = event
        .pointer("/game/gameId")
        .or_else(|| event.pointer("/game/id"))
        .and_then(Value::as_str)
        .ok_or_else(|| "Lichess sent a game start without a game id".to_string())?
        .to_string();
    let result = state
        .spawn_game_task(
            account.clone(),
            engine.clone(),
            token.to_string(),
            game_id.clone(),
            cancellation.child_token(),
            generation,
        )
        .await;
    if result.is_ok() {
        let opponent = event
            .pointer("/game/opponent/username")
            .or_else(|| event.pointer("/game/opponent/id"))
            .and_then(Value::as_str);
        state
            .forget_resolved_outgoing_challenge(&account.id, &game_id, opponent)
            .await;
    }
    result
}

/// QueenUI only plays games it initiated, so incoming challenges are declined
/// explicitly instead of being left to expire on the challenger's side.
async fn decline_incoming_challenge(
    state: &AppState,
    account: &AccountProfile,
    token: &str,
    event: &Value,
) {
    let Some(challenge_id) = event.pointer("/challenge/id").and_then(Value::as_str) else {
        return;
    };
    let challenger = event
        .pointer("/challenge/challenger/id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    // Our own outgoing challenges are also echoed on the event stream; never
    // decline those.
    if challenger.eq_ignore_ascii_case(&account.id)
        || challenger.eq_ignore_ascii_case(&account.username)
    {
        return;
    }
    if let Err(error) = lichess::decline_challenge(
        &state.0.api_base,
        &state.0.api_client,
        token,
        challenge_id,
        "generic",
    )
    .await
    {
        diagnostics::record(
            DiagnosticEntry::warn("lichess", "Could not decline an incoming challenge")
                .with_detail(format!("challenge {challenge_id}: {error}")),
        );
    }
}

/// Records one game's complete UCI conversation while it is played.
///
/// The recording is closed on every exit path — including the failures, which
/// are exactly the sessions worth reading afterwards — so the session
/// lifecycle lives here rather than inside the many early returns of
/// `run_game_session`.
async fn run_game(
    state: AppState,
    account: AccountProfile,
    engine_profile: EngineProfile,
    token: String,
    game_id: String,
    cancellation: CancellationToken,
) -> Result<(), String> {
    let log = state.open_game_log(&account, &engine_profile, &game_id);
    if log.is_some() {
        state.emit_logs_updated();
    }
    let outcome = run_game_session(
        state.clone(),
        account.clone(),
        engine_profile,
        token,
        game_id.clone(),
        cancellation,
        log.clone(),
    )
    .await;
    if let Some(log) = log {
        let (status, result) = state
            .0
            .games
            .read()
            .await
            .get(&(account.id.clone(), game_id))
            .map(|game| (game.status.clone(), game.result.clone()))
            .unwrap_or_else(|| ("unknown".to_string(), None));
        // Reaching here with the game still running means QueenUI stopped
        // watching it — the bot was stopped or the task was cancelled — so
        // recording it as "started" would misreport why the log ends.
        let status = match status.as_str() {
            "started" | "created" => "stopped".to_string(),
            _ => status,
        };
        if let Err(error) = &outcome {
            log.note(&format!("game-error detail={error}"));
        }
        log.finish(&status, result.as_deref()).await;
        state.emit_logs_updated();
    }
    outcome
}

async fn run_game_session(
    state: AppState,
    account: AccountProfile,
    engine_profile: EngineProfile,
    token: String,
    game_id: String,
    cancellation: CancellationToken,
    log: Option<enginelog::LogWriter>,
) -> Result<(), String> {
    let governor = state.engine_governor();
    let mut engine = uci::UciEngine::start_governed(
        &engine_profile.path,
        &engine_profile.options,
        log.clone(),
        &governor,
    )
    .await?;
    let opening_book = state
        .opening_book(engine_profile.opening_book.as_ref())
        .await;
    let game_key: GameKey = (account.id.clone(), game_id.clone());
    let mut context = GameContext {
        log,
        ..GameContext::default()
    };
    let transport: Arc<dyn MoveTransport> = Arc::new(LichessMoveTransport {
        base: state.0.api_base.clone(),
        client: state.0.api_client.clone(),
        token: token.clone(),
    });
    let mut submissions = SubmissionCoordinator::start(
        state.clone(),
        game_key.clone(),
        transport,
        context.telemetry.submission_retries.clone(),
        &cancellation,
    );
    let outcome = run_game_stream_loop(
        &state,
        &account,
        &engine_profile,
        &token,
        &game_id,
        &cancellation,
        opening_book.as_deref(),
        &mut engine,
        &mut submissions,
        &mut context,
    )
    .await;
    let submission_shutdown = submissions.shutdown().await;
    engine.shutdown().await;
    match (outcome, submission_shutdown) {
        (Err(error), Err(shutdown)) => Err(format!("{error}; {shutdown}")),
        (Err(error), _) => Err(error),
        (Ok(()), Err(shutdown)) => Err(shutdown),
        (Ok(()), Ok(())) => Ok(()),
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_game_stream_loop(
    state: &AppState,
    account: &AccountProfile,
    engine_profile: &EngineProfile,
    token: &str,
    game_id: &str,
    cancellation: &CancellationToken,
    opening_book: Option<&opening_book::OpeningBook>,
    engine: &mut uci::UciEngine,
    submissions: &mut SubmissionCoordinator,
    context: &mut GameContext,
) -> Result<(), String> {
    let game_key: GameKey = (account.id.clone(), game_id.to_string());
    let mut reconnect_delay = Duration::from_millis(250);
    // Reconnecting is only worthwhile for transient failures; cap the total
    // time spent in a consecutive-failure window so a dead game can never leak
    // its engine process forever.
    const MAX_RECONNECT_WINDOW: Duration = Duration::from_secs(600);
    let mut failing_since: Option<Instant> = None;

    loop {
        let response = tokio::select! {
            _ = cancellation.cancelled() => return Ok(()),
            response = lichess::game_stream(
                &state.0.api_base,
                &state.0.client,
                token,
                game_id,
            ) => response,
        };
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                if error.is_auth_failure() || error.is_not_found() {
                    // Permanent rejections will never succeed on retry.
                    return Err(format!(
                        "Lichess rejected the game stream permanently: {error}"
                    ));
                }
                if failing_since.get_or_insert_with(Instant::now).elapsed() >= MAX_RECONNECT_WINDOW
                {
                    return Err(format!(
                        "Gave up reconnecting to game {game_id} after {} minutes of failures without auto-resigning an unreconciled game: {error}",
                        MAX_RECONNECT_WINDOW.as_secs() / 60
                    ));
                }
                context.telemetry.stream_reconnects += 1;
                show_stream_reconnect(state, &game_key, &error.to_string(), reconnect_delay).await;
                tokio::select! {
                    _ = cancellation.cancelled() => return Ok(()),
                    _ = tokio::time::sleep(reconnect_delay) => {}
                }
                reconnect_delay = (reconnect_delay * 2).min(Duration::from_secs(5));
                continue;
            }
        };

        let mut stream = response.bytes_stream();
        let mut buffer = Vec::new();
        let mut received = 0usize;
        // A proxy or captive portal answering with HTML yields one unreadable
        // line per newline in the body. Counting them and reporting once keeps
        // the 1000-entry diagnostics ring from evicting everything that
        // explains why the bot actually stopped.
        let mut skipped_lines = 0u64;
        let mut first_skip_detail: Option<String> = None;
        let interruption = loop {
            let chunk = tokio::select! {
                _ = cancellation.cancelled() => return Ok(()),
                chunk = stream.next() => chunk,
            };
            match chunk {
                Some(Ok(chunk)) => {
                    reconnect_delay = Duration::from_millis(250);
                    failing_since = None;
                    let lines = lichess::append_ndjson_chunk(
                        &mut buffer,
                        &chunk,
                        &mut received,
                        64 * 1024 * 1024,
                        "read game stream",
                    )
                    .map_err(|error| error.to_string())?;
                    for line in lines {
                        if line.trim().is_empty() {
                            continue;
                        }
                        // One malformed line must not abort the whole game task.
                        let event: Value = match serde_json::from_str(&line) {
                            Ok(event) => event,
                            Err(error) => {
                                if skipped_lines == 0 {
                                    first_skip_detail = Some(error.to_string());
                                }
                                skipped_lines += 1;
                                continue;
                            }
                        };
                        let completed = process_game_event(
                            state,
                            account,
                            game_id,
                            token,
                            engine,
                            engine_profile,
                            opening_book,
                            submissions,
                            context,
                            event,
                        )
                        .await?;
                        if completed {
                            report_skipped_lines(
                                skipped_lines,
                                first_skip_detail.take(),
                                &account.id,
                                game_id,
                            );
                            return Ok(());
                        }
                    }
                }
                Some(Err(error)) => break format!("Game stream interrupted: {error}"),
                None => break "Lichess closed the game stream unexpectedly".to_string(),
            }
        };

        report_skipped_lines(
            skipped_lines,
            first_skip_detail.take(),
            &account.id,
            game_id,
        );
        if failing_since.get_or_insert_with(Instant::now).elapsed() >= MAX_RECONNECT_WINDOW {
            return Err(format!(
                "Gave up reconnecting to game {game_id} after {} minutes of failures without auto-resigning an unreconciled game: {interruption}",
                MAX_RECONNECT_WINDOW.as_secs() / 60
            ));
        }
        context.telemetry.stream_reconnects += 1;
        show_stream_reconnect(state, &game_key, &interruption, reconnect_delay).await;
        tokio::select! {
            _ = cancellation.cancelled() => return Ok(()),
            _ = tokio::time::sleep(reconnect_delay) => {}
        }
        reconnect_delay = (reconnect_delay * 2).min(Duration::from_secs(5));
    }
}

/// Reports a game stream's unreadable lines as a single diagnostic.
fn report_skipped_lines(count: u64, detail: Option<String>, account_id: &str, game_id: &str) {
    if count == 0 {
        return;
    }
    let mut entry = DiagnosticEntry::warn(
        "lichess",
        format!(
            "Skipped {count} unreadable line{} on a game stream",
            if count == 1 { "" } else { "s" }
        ),
    )
    .with_account(account_id)
    .with_game(game_id);
    if let Some(detail) = detail {
        entry = entry.with_detail(detail);
    }
    diagnostics::record(entry);
}

async fn show_stream_reconnect(
    state: &AppState,
    game_key: &GameKey,
    error: &str,
    retry_in: Duration,
) {
    if let Some(game) = state.0.games.write().await.get_mut(game_key) {
        game.error = Some(format!(
            "{error}. Reconnecting without leaving the game in {:.2}s…",
            retry_in.as_secs_f32()
        ));
    }
    state.emit_snapshot().await;
}

trait MoveTransport: Send + Sync {
    fn submit<'a>(
        &'a self,
        game_id: &'a str,
        chess_move: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), lichess::LichessError>> + Send + 'a>>;
}

struct LichessMoveTransport {
    base: Url,
    client: Client,
    token: String,
}

impl MoveTransport for LichessMoveTransport {
    fn submit<'a>(
        &'a self,
        game_id: &'a str,
        chess_move: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), lichess::LichessError>> + Send + 'a>> {
        Box::pin(lichess::play_move(
            &self.base,
            &self.client,
            &self.token,
            game_id,
            chess_move,
        ))
    }
}

#[derive(Clone, Debug, Default)]
struct SubmissionObservation {
    moves: String,
    terminal: bool,
}

struct SubmissionRequest {
    position_moves: String,
    selected_move: String,
}

#[derive(Clone, Copy)]
struct SubmissionPolicy {
    budget: Duration,
    max_attempts: u32,
}

impl Default for SubmissionPolicy {
    fn default() -> Self {
        Self {
            budget: Duration::from_secs(60),
            max_attempts: 8,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum SubmissionOutcome {
    Submitted,
    Reconciled,
    Rejected(String),
    Exhausted(String),
    Canceled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AutoResignReason {
    EngineUnavailable,
}

/// QueenUI never auto-resigns because an HTTP status/body "looks bad". The
/// sole automated policy is a twice-failed engine search after the current
/// authoritative stream position has parsed and replayed legally.
fn should_auto_resign(reason: AutoResignReason, position_reconciled: bool) -> bool {
    position_reconciled && reason == AutoResignReason::EngineUnavailable
}

struct SubmissionCoordinator {
    commands: mpsc::Sender<SubmissionRequest>,
    observations: watch::Sender<SubmissionObservation>,
    cancellation: CancellationToken,
    handle: Option<JoinHandle<()>>,
}

struct SubmissionWorkerChannels {
    commands: mpsc::Receiver<SubmissionRequest>,
    observations: watch::Receiver<SubmissionObservation>,
    outcomes: Option<mpsc::UnboundedSender<SubmissionOutcome>>,
}

impl SubmissionCoordinator {
    fn start(
        app: AppState,
        game_key: GameKey,
        transport: Arc<dyn MoveTransport>,
        retries: Arc<AtomicI64>,
        parent_cancellation: &CancellationToken,
    ) -> Self {
        Self::start_with_policy(
            app,
            game_key,
            transport,
            retries,
            parent_cancellation,
            SubmissionPolicy::default(),
            None,
        )
    }

    #[cfg(test)]
    fn start_for_test(
        app: AppState,
        game_key: GameKey,
        transport: Arc<dyn MoveTransport>,
        retries: Arc<AtomicI64>,
        parent_cancellation: &CancellationToken,
        policy: SubmissionPolicy,
    ) -> (Self, mpsc::UnboundedReceiver<SubmissionOutcome>) {
        let (outcomes, receiver) = mpsc::unbounded_channel();
        (
            Self::start_with_policy(
                app,
                game_key,
                transport,
                retries,
                parent_cancellation,
                policy,
                Some(outcomes),
            ),
            receiver,
        )
    }

    fn start_with_policy(
        app: AppState,
        game_key: GameKey,
        transport: Arc<dyn MoveTransport>,
        retries: Arc<AtomicI64>,
        parent_cancellation: &CancellationToken,
        policy: SubmissionPolicy,
        outcomes: Option<mpsc::UnboundedSender<SubmissionOutcome>>,
    ) -> Self {
        let (commands, receiver) = mpsc::channel(1);
        let (observations, observation_receiver) = watch::channel(SubmissionObservation::default());
        let cancellation = parent_cancellation.child_token();
        let worker_cancellation = cancellation.clone();
        let worker_key = game_key.clone();
        let handle = tokio::spawn(async move {
            submission_worker(
                app,
                worker_key,
                transport,
                retries,
                SubmissionWorkerChannels {
                    commands: receiver,
                    observations: observation_receiver,
                    outcomes,
                },
                worker_cancellation,
                policy,
            )
            .await;
        });
        Self {
            commands,
            observations,
            cancellation,
            handle: Some(handle),
        }
    }

    fn observe(&self, moves: String, terminal: bool) {
        self.observations
            .send_replace(SubmissionObservation { moves, terminal });
    }

    async fn submit(&self, position_moves: String, selected_move: String) -> Result<(), String> {
        self.commands
            .send(SubmissionRequest {
                position_moves,
                selected_move,
            })
            .await
            .map_err(|_| "The game's move submission coordinator has stopped".to_string())
    }

    async fn shutdown(&mut self) -> Result<(), String> {
        self.cancellation.cancel();
        if let Some(handle) = self.handle.take() {
            join_owned_task(handle, "move submission coordinator").await
        } else {
            Ok(())
        }
    }
}

impl Drop for SubmissionCoordinator {
    fn drop(&mut self) {
        self.cancellation.cancel();
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}

async fn submission_worker(
    app: AppState,
    game_key: GameKey,
    transport: Arc<dyn MoveTransport>,
    retries: Arc<AtomicI64>,
    mut channels: SubmissionWorkerChannels,
    cancellation: CancellationToken,
    policy: SubmissionPolicy,
) {
    loop {
        let request = tokio::select! {
            _ = cancellation.cancelled() => return,
            request = channels.commands.recv() => match request {
                Some(request) => request,
                None => return,
            },
        };
        let outcome = run_submission(
            transport.as_ref(),
            &game_key.1,
            &request,
            &mut channels.observations,
            &cancellation,
            policy,
            &retries,
        )
        .await;
        if let Some(outcomes) = &channels.outcomes {
            let _ = outcomes.send(outcome.clone());
        }
        let message = match outcome {
            SubmissionOutcome::Submitted | SubmissionOutcome::Reconciled => None,
            SubmissionOutcome::Rejected(error) => Some(format!(
                "Lichess rejected the validated move; QueenUI will not auto-resign without a reconciled engine failure: {error}"
            )),
            SubmissionOutcome::Exhausted(error) => Some(format!(
                "Move submission remained uncertain after the bounded retry budget: {error}"
            )),
            SubmissionOutcome::Canceled => return,
        };
        if let Some(game) = app.0.games.write().await.get_mut(&game_key) {
            game.error = message;
        }
        app.emit_snapshot().await;
    }
}

async fn run_submission(
    transport: &dyn MoveTransport,
    game_id: &str,
    request: &SubmissionRequest,
    observations: &mut watch::Receiver<SubmissionObservation>,
    cancellation: &CancellationToken,
    policy: SubmissionPolicy,
    retries: &AtomicI64,
) -> SubmissionOutcome {
    let started = tokio::time::Instant::now();
    let mut attempt = 0u32;
    loop {
        if cancellation.is_cancelled() {
            return SubmissionOutcome::Canceled;
        }
        attempt += 1;
        let response = tokio::select! {
            _ = cancellation.cancelled() => return SubmissionOutcome::Canceled,
            response = transport.submit(game_id, &request.selected_move) => response,
        };
        match response {
            Ok(()) => return SubmissionOutcome::Submitted,
            Err(error) => {
                // Only the authoritative stream can turn an ambiguous HTTP
                // outcome into success. Error text and status alone cannot.
                let observed = observations.borrow().clone();
                if observed.terminal || observed.moves != request.position_moves {
                    return SubmissionOutcome::Reconciled;
                }
                if error.is_auth_failure()
                    || error.is_not_found()
                    || error
                        .status
                        .is_some_and(|status| status.is_client_error() && !error.is_rate_limited())
                {
                    return SubmissionOutcome::Rejected(error.to_string());
                }
                if attempt >= policy.max_attempts || started.elapsed() >= policy.budget {
                    return SubmissionOutcome::Exhausted(error.to_string());
                }
                retries.fetch_add(1, Ordering::Relaxed);
                let remaining = policy.budget.saturating_sub(started.elapsed());
                let delay = submission_retry_delay(&error, attempt, true).min(remaining);
                let observed = observations.borrow().clone();
                if observed.terminal || observed.moves != request.position_moves {
                    return SubmissionOutcome::Reconciled;
                }
                tokio::select! {
                    _ = cancellation.cancelled() => return SubmissionOutcome::Canceled,
                    changed = observations.changed() => {
                        if changed.is_err() {
                            return SubmissionOutcome::Canceled;
                        }
                        let observed = observations.borrow().clone();
                        if observed.terminal || observed.moves != request.position_moves {
                            return SubmissionOutcome::Reconciled;
                        }
                    }
                    _ = tokio::time::sleep(delay) => {}
                }
            }
        }
    }
}

fn submission_retry_delay(
    error: &lichess::LichessError,
    attempt: u32,
    with_jitter: bool,
) -> Duration {
    let exponential = Duration::from_millis(
        250u64
            .saturating_mul(1u64 << attempt.saturating_sub(1).min(5))
            .min(5_000),
    );
    let base = error
        .retry_after
        .unwrap_or(exponential)
        .min(Duration::from_secs(30));
    if !with_jitter || base.is_zero() {
        return base;
    }
    let maximum_jitter = (base.as_millis() as u64 / 5).clamp(1, 500);
    base.saturating_add(Duration::from_millis(rand::random_range(
        0..=maximum_jitter,
    )))
}

/// "180+2" when Lichess sent a clock, otherwise its speed key ("correspondence").
fn context_clock_label(context: &GameContext) -> String {
    match (context.clock_limit, context.clock_increment) {
        (Some(limit), Some(increment)) => format!("{limit}+{increment}"),
        _ => context
            .speed
            .clone()
            .unwrap_or_else(|| "unknown".to_string()),
    }
}

#[derive(Default)]
struct GameContext {
    initial_fen: String,
    color: String,
    opponent: String,
    bot_rating: Option<i64>,
    opponent_rating: Option<i64>,
    rated: bool,
    /// Seconds, from the gameFull clock (Lichess sends milliseconds).
    clock_limit: Option<i64>,
    clock_increment: Option<i64>,
    /// Lichess speed key from gameFull, used when no clock is available.
    speed: Option<String>,
    last_searched_moves: Option<String>,
    automation_halted: bool,
    telemetry: TelemetryCapture,
    /// Flight recorder for this game, carried here so every handler that sees
    /// the context can annotate the recording without a wider signature.
    log: Option<enginelog::LogWriter>,
}

/// Engine-side counters accumulated while one game runs; flattened into a
/// history::GameTelemetry when the game finishes. Lichess never sees this.
#[derive(Default)]
struct TelemetryCapture {
    /// One clamped eval snapshot per submitted move (see eval_entry).
    eval_series_cp: Vec<i32>,
    /// Search depth reported at each engine-searched submission.
    depths: Vec<i64>,
    /// Elapsed milliseconds from detecting our turn to submitting the move.
    move_times_ms: Vec<i64>,
    book_plies: i64,
    engine_restarts: i64,
    /// Shared with the owned submission coordinator, which is joined before
    /// the game task can exit.
    submission_retries: Arc<AtomicI64>,
    stream_reconnects: i64,
    failure_resign: bool,
    end_clock_ms: Option<i64>,
}

impl TelemetryCapture {
    /// Records one submitted move: eval snapshot, search depth (when the move
    /// came from an engine search), and the time from turn detection to
    /// submission. Book moves pass `info: None` and count toward book_plies.
    fn record_submission(
        &mut self,
        info: Option<&models::EngineTelemetry>,
        move_time_ms: i64,
        from_book: bool,
    ) {
        let previous = self.eval_series_cp.last().copied();
        self.eval_series_cp.push(eval_entry(info, previous));
        if let Some(depth) = info.and_then(|info| info.depth) {
            self.depths.push(i64::from(depth));
        }
        self.move_times_ms.push(move_time_ms.max(0));
        if from_book {
            self.book_plies += 1;
        }
    }
}

/// Evals are clamped so one mate line cannot dwarf every chart.
const EVAL_CLAMP_CP: i32 = 1000;

/// Maps the final search telemetry of one of our moves to an eval snapshot in
/// our perspective (the engine searched on our turn, so UCI's side-to-move
/// scores already are our-perspective). Mate scores map to +/-1000, centipawn
/// scores clamp to [-1000, 1000]. Book moves and searches without any score
/// repeat the previous snapshot, or 0 when none exists yet (book openings are
/// treated as balanced), keeping the series aligned one entry per move.
fn eval_entry(info: Option<&models::EngineTelemetry>, previous: Option<i32>) -> i32 {
    if let Some(info) = info {
        if let Some(mate) = info.mate_in {
            return if mate >= 0 {
                EVAL_CLAMP_CP
            } else {
                -EVAL_CLAMP_CP
            };
        }
        if let Some(cp) = info.score_cp {
            return cp.clamp(-EVAL_CLAMP_CP, EVAL_CLAMP_CP);
        }
    }
    previous.unwrap_or(0)
}

/// Deterministic FNV-1a over the canonical configuration string.
fn fnv1a_64(input: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in input.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Stable 12-hex-char fingerprint of an engine setup: engine id, every UCI
/// option that has a value (sorted by name), and the opening-book
/// configuration. Games played under the same fingerprint form one cohort.
fn config_fingerprint(engine: &EngineProfile) -> String {
    let mut options: Vec<String> = engine
        .options
        .iter()
        .filter_map(|option| {
            option
                .value
                .as_ref()
                .map(|value| format!("{}={}", option.name, value))
        })
        .collect();
    options.sort();
    let book = match engine.opening_book.as_ref() {
        Some(book) => format!(
            "book={}|enabled={}|maxPlies={}|percent={}",
            book.path, book.enabled, book.max_plies, book.top_move_percent
        ),
        None => "book=none".to_string(),
    };
    let canonical = format!("engine={}\n{}\n{book}", engine.id, options.join("\n"));
    let hex = format!("{:016x}", fnv1a_64(&canonical));
    hex[..12].to_string()
}

/// Flattens the per-game counters into the persisted telemetry record.
fn build_game_telemetry(
    capture: &TelemetryCapture,
    config_fingerprint: String,
) -> history::GameTelemetry {
    fn average(values: &[i64]) -> Option<f64> {
        (!values.is_empty()).then(|| values.iter().sum::<i64>() as f64 / values.len() as f64)
    }
    history::GameTelemetry {
        eval_series_cp: capture.eval_series_cp.clone(),
        avg_depth: average(&capture.depths),
        min_depth: capture.depths.iter().copied().min(),
        avg_move_time_ms: average(&capture.move_times_ms),
        max_move_time_ms: capture.move_times_ms.iter().copied().max(),
        end_clock_ms: capture.end_clock_ms,
        book_plies: capture.book_plies,
        engine_restarts: capture.engine_restarts,
        submission_retries: capture.submission_retries.load(Ordering::Relaxed),
        stream_reconnects: capture.stream_reconnects,
        failure_resign: capture.failure_resign,
        max_eval_cp: capture.eval_series_cp.iter().copied().max(),
        min_eval_cp: capture.eval_series_cp.iter().copied().min(),
        blunders: history::count_blunders(&capture.eval_series_cp),
        config_fingerprint: Some(config_fingerprint),
    }
}

#[allow(clippy::too_many_arguments)]
async fn process_game_event(
    app: &AppState,
    account: &AccountProfile,
    game_id: &str,
    token: &str,
    engine: &mut uci::UciEngine,
    engine_profile: &EngineProfile,
    opening_book: Option<&opening_book::OpeningBook>,
    submissions: &mut SubmissionCoordinator,
    context: &mut GameContext,
    event: Value,
) -> Result<bool, String> {
    let game_key: GameKey = (account.id.clone(), game_id.to_string());
    let event_type = event
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let state = if event_type == "gameFull" {
        context.initial_fen = event
            .get("initialFen")
            .and_then(Value::as_str)
            .unwrap_or("startpos")
            .to_string();
        context.rated = event.get("rated").and_then(Value::as_bool).unwrap_or(false);
        context.speed = event
            .get("speed")
            .and_then(Value::as_str)
            .map(str::to_string);
        context.clock_limit = event
            .pointer("/clock/initial")
            .and_then(Value::as_i64)
            .map(|milliseconds| milliseconds / 1000);
        context.clock_increment = event
            .pointer("/clock/increment")
            .and_then(Value::as_i64)
            .map(|milliseconds| milliseconds / 1000);
        let white_id = event
            .pointer("/white/id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let black_id = event
            .pointer("/black/id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if white_id.eq_ignore_ascii_case(&account.id)
            || white_id.eq_ignore_ascii_case(&account.username)
        {
            context.color = "white".into();
            context.opponent = player_name(event.get("black"));
            context.bot_rating = player_rating(event.get("white"));
            context.opponent_rating = player_rating(event.get("black"));
        } else if black_id.eq_ignore_ascii_case(&account.id)
            || black_id.eq_ignore_ascii_case(&account.username)
        {
            context.color = "black".into();
            context.opponent = player_name(event.get("white"));
            context.bot_rating = player_rating(event.get("black"));
            context.opponent_rating = player_rating(event.get("white"));
        } else {
            return Err("Could not determine the bot's color in the game".into());
        }
        if let Some(log) = &context.log {
            log.describe(enginelog::SessionDescription {
                opponent: Some(context.opponent.clone()),
                opponent_rating: context.opponent_rating,
                color: Some(context.color.clone()),
                clock: Some(context_clock_label(context)),
                initial_fen: Some(context.initial_fen.clone()),
            });
        }
        event.get("state").cloned().unwrap_or(Value::Null)
    } else if event_type == "gameState" {
        event
    } else {
        return Ok(false);
    };

    let status = state
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("started")
        .to_string();
    let raw_moves = state
        .get("moves")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let live_position = crate::position::LivePosition::parse(&context.initial_fen, &raw_moves)?;
    context.initial_fen = live_position.initial_fen().to_string();
    let moves = live_position.moves();
    let white_time = state.get("wtime").and_then(Value::as_i64).unwrap_or(0);
    let black_time = state.get("btime").and_then(Value::as_i64).unwrap_or(0);
    let white_increment = state.get("winc").and_then(Value::as_i64).unwrap_or(0);
    let black_increment = state.get("binc").and_then(Value::as_i64).unwrap_or(0);
    let clock_updated_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let completed = status != "started" && status != "created";
    submissions.observe(moves.clone(), completed);
    let result = game_result(&status, state.get("winner").and_then(Value::as_str));

    let (existing_line, existing_info, existing_thinking) = app
        .0
        .games
        .read()
        .await
        .get(&game_key)
        .map(|game| {
            (
                game.engine_line.clone(),
                game.engine_info.clone(),
                game.engine_thinking,
            )
        })
        .unwrap_or((None, None, false));
    let game = LiveGame {
        id: game_id.to_string(),
        account_id: account.id.clone(),
        bot_username: account.username.clone(),
        opponent: context.opponent.clone(),
        bot_rating: context.bot_rating.or(account.rating),
        opponent_rating: context.opponent_rating,
        color: context.color.clone(),
        initial_fen: context.initial_fen.clone(),
        moves: moves.clone(),
        status: status.clone(),
        white_time,
        black_time,
        white_increment,
        black_increment,
        clock_updated_at,
        result: result.clone(),
        engine_line: existing_line,
        engine_info: existing_info,
        engine_thinking: !completed && existing_thinking,
        error: None,
    };
    {
        let mut games = app.0.games.write().await;
        games.insert(game_key.clone(), game);
        prune_finished_games(&mut games);
    }
    app.emit_snapshot().await;
    if completed {
        app.remove_active_intent(&account.id, game_id).await?;
        context.telemetry.end_clock_ms = Some(if context.color == "white" {
            white_time
        } else {
            black_time
        });
        record_finished_game(
            app,
            account,
            engine_profile,
            game_id,
            context,
            &status,
            result.as_deref(),
            &moves,
        )
        .await;
        return Ok(true);
    }

    let ply_count = live_position.ply_count();
    let white_to_move = live_position.is_white_to_move();
    let our_turn = (context.color == "white") == white_to_move;
    if our_turn
        && !context.automation_halted
        && context.last_searched_moves.as_deref() != Some(&moves)
    {
        context.last_searched_moves = Some(moves.clone());
        // Move time runs from here (our turn detected on the stream) until the
        // first submission attempt for the selected move.
        let turn_detected_at = Instant::now();
        if let Some(book_move) =
            opening_book.and_then(|book| book.choose_move(&context.initial_fen, &moves))
        {
            if let Some(log) = &context.log {
                log.note(&format!(
                    "book move={book_move} ply={ply_count} number={}",
                    ply_count / 2 + 1
                ));
            }
            if let Some(game) = app.0.games.write().await.get_mut(&game_key) {
                game.engine_thinking = false;
                game.engine_info = None;
                game.engine_line = engine_profile.opening_book.as_ref().map(|book| {
                    format!(
                        "book {} · {} · top {}% candidate pool",
                        book.name, book_move, book.top_move_percent
                    )
                });
                game.error = None;
            }
            app.emit_snapshot().await;
            context.telemetry.record_submission(
                None,
                turn_detected_at.elapsed().as_millis().min(i64::MAX as u128) as i64,
                true,
            );
            submissions.submit(moves.clone(), book_move).await?;
            return Ok(false);
        }
        if let Some(game) = app.0.games.write().await.get_mut(&game_key) {
            game.engine_thinking = true;
            game.engine_info = None;
            game.engine_line = None;
        }
        app.emit_snapshot().await;

        let (info_sender, mut info_receiver) =
            tokio::sync::mpsc::unbounded_channel::<models::EngineTelemetry>();
        let telemetry_app = app.clone();
        let telemetry_game_key = game_key.clone();
        let telemetry_position = moves.clone();
        let telemetry_pump = tokio::spawn(async move {
            while let Some(info) = info_receiver.recv().await {
                let updated = {
                    let mut games = telemetry_app.0.games.write().await;
                    if let Some(game) = games.get_mut(&telemetry_game_key) {
                        if game.engine_thinking && game.moves == telemetry_position {
                            game.engine_line = Some(info.raw.clone());
                            game.engine_info = Some(info);
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                };
                if updated {
                    telemetry_app.emit_snapshot().await;
                }
            }
        });
        if let Some(log) = &context.log {
            // The outline the Logs page builds is cut on these notes, so they
            // carry everything needed to label one search: which move it is,
            // for which side, under which clock.
            log.note(&format!(
                "search ply={ply_count} move={} color={} wtime={white_time} btime={black_time} winc={white_increment} binc={black_increment}",
                ply_count / 2 + 1,
                if white_to_move { "w" } else { "b" },
            ));
        }
        let search_started = Instant::now();
        let mut search_result = engine
            .best_move(
                &context.initial_fen,
                &moves,
                white_time,
                black_time,
                white_increment,
                black_increment,
                |info| {
                    let _ = info_sender.send(info);
                },
            )
            .await;
        if let Err(first_error) = &search_result {
            let first_error = first_error.clone();
            if let Some(game) = app.0.games.write().await.get_mut(&game_key) {
                game.error = Some(format!(
                    "Engine search stalled; restarting it safely: {first_error}"
                ));
            }
            app.emit_snapshot().await;

            engine.shutdown().await;
            context.telemetry.engine_restarts += 1;
            if let Some(log) = &context.log {
                log.note(&format!("engine-restart reason={first_error}"));
            }
            diagnostics::record(
                DiagnosticEntry::warn("engine", "Restarting the engine after a failed search")
                    .with_account(&account.id)
                    .with_game(game_id)
                    .with_detail(&first_error),
            );
            let governor = app.engine_governor();
            search_result = match uci::UciEngine::start_governed(
                &engine_profile.path,
                &engine_profile.options,
                context.log.clone(),
                &governor,
            )
            .await
            {
                Ok(replacement) => {
                    *engine = replacement;
                    let elapsed = search_started.elapsed().as_millis().min(i64::MAX as u128) as i64;
                    let retry_white_time = if context.color == "white" {
                        white_time.saturating_sub(elapsed)
                    } else {
                        white_time
                    };
                    let retry_black_time = if context.color == "black" {
                        black_time.saturating_sub(elapsed)
                    } else {
                        black_time
                    };
                    engine
                        .best_move(
                            &context.initial_fen,
                            &moves,
                            retry_white_time,
                            retry_black_time,
                            white_increment,
                            black_increment,
                            |info| {
                                let _ = info_sender.send(info);
                            },
                        )
                        .await
                        .map_err(|retry_error| {
                            format!(
                                "Initial search failed ({first_error}); the restarted engine also failed ({retry_error})"
                            )
                        })
                }
                Err(restart_error) => Err(format!(
                    "Initial search failed ({first_error}); the engine could not be restarted ({restart_error})"
                )),
            };
        }
        drop(info_sender);
        let _ = telemetry_pump.await;
        let search = match search_result {
            Ok(search) => search,
            Err(error) => {
                // Narrow auto-resign policy: only an unrecoverable engine
                // failure, while processing a freshly parsed authoritative
                // game state, may trigger it. HTTP move rejections and stream
                // uncertainty never do.
                context.telemetry.failure_resign = true;
                context.automation_halted = true;
                if let Some(log) = &context.log {
                    log.note(&format!("search-failed detail={error}"));
                }
                diagnostics::record(
                    DiagnosticEntry::error("engine", "Engine recovery failed")
                        .with_account(&account.id)
                        .with_game(game_id)
                        .with_detail(&error),
                );
                submissions.shutdown().await?;
                let reconciliation =
                    lichess::ongoing_game_ids(&app.0.api_base, &app.0.api_client, token).await;
                let resignation = match reconciliation {
                    Ok(ongoing)
                        if ongoing.iter().any(|ongoing_id| ongoing_id == game_id)
                            && should_auto_resign(
                                AutoResignReason::EngineUnavailable,
                                true,
                            ) =>
                    {
                        lichess::resign(&app.0.api_base, &app.0.api_client, token, game_id)
                            .await
                            .map_err(|error| error.to_string())
                    }
                    Ok(_) => Err(
                        "Authoritative account state no longer lists this game as ongoing".into(),
                    ),
                    Err(error) => Err(format!(
                        "Could not reconcile authoritative ongoing games before auto-resign: {error}"
                    )),
                };
                let message = match resignation {
                    Ok(()) => format!(
                        "Engine recovery failed, so QueenUI resigned cleanly instead of disconnecting: {error}"
                    ),
                    Err(resign_error) => format!(
                        "Engine recovery failed. QueenUI kept the game stream connected, but could not submit a resignation: {error}; {resign_error}"
                    ),
                };
                if let Some(game) = app.0.games.write().await.get_mut(&game_key) {
                    game.engine_thinking = false;
                    game.error = Some(message);
                }
                app.emit_snapshot().await;
                // Keep consuming Lichess' authoritative game stream. Returning an
                // error here used to drop the connection and produced "left the
                // game" losses after an engine timeout.
                return Ok(false);
            }
        };
        if let Some(log) = &context.log {
            log.note(&format!(
                "bestmove uci={} elapsed={}",
                search.best_move,
                search_started.elapsed().as_millis()
            ));
        }
        if let Some(game) = app.0.games.write().await.get_mut(&game_key) {
            game.engine_line = search.last_info.clone();
            game.engine_info = search.telemetry.clone();
            game.engine_thinking = false;
            game.error = None;
        }
        app.emit_snapshot().await;
        context.telemetry.record_submission(
            search.telemetry.as_ref(),
            turn_detected_at.elapsed().as_millis().min(i64::MAX as u128) as i64,
            false,
        );
        submissions
            .submit(moves.clone(), search.best_move.clone())
            .await?;
    }
    Ok(false)
}

/// Persists one finished game to the Scorebook history. Aborted/noStart games
/// (result "*") and games without a result are never recorded.
#[allow(clippy::too_many_arguments)]
async fn record_finished_game(
    app: &AppState,
    account: &AccountProfile,
    engine_profile: &EngineProfile,
    game_id: &str,
    context: &GameContext,
    status: &str,
    result: Option<&str>,
    moves: &str,
) {
    let Some(result) = result else { return };
    let Some(outcome) = history::result_from_pgn(result, &context.color) else {
        return;
    };
    let perf = match context.clock_limit {
        Some(limit) => history::perf_key_for_clock(
            limit.clamp(0, u32::MAX as i64) as u32,
            context
                .clock_increment
                .unwrap_or(0)
                .clamp(0, u32::MAX as i64) as u32,
        )
        .to_string(),
        None => context
            .speed
            .clone()
            .unwrap_or_else(|| "classical".to_string()),
    };
    let finished_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    let record = history::GameRecord {
        id: game_id.to_string(),
        account_id: account.id.clone(),
        account_username: account.username.clone(),
        engine_id: Some(engine_profile.id.clone()),
        engine_name: Some(engine_profile.name.clone()),
        opponent: context.opponent.clone(),
        opponent_rating: context.opponent_rating,
        our_rating: context.bot_rating.or(account.rating),
        color: context.color.clone(),
        result: outcome.to_string(),
        status: status.to_string(),
        rated: context.rated,
        clock_limit: context.clock_limit,
        clock_increment: context.clock_increment,
        perf,
        moves_count: moves.split_whitespace().count() as i64,
        finished_at_ms,
        source: "queenui".into(),
        opening: None,
        telemetry: Some(build_game_telemetry(
            &context.telemetry,
            config_fingerprint(engine_profile),
        )),
    };
    match app.0.history.append(record).await {
        Ok(true) => {
            let _ = app.0.events.send(CoreEvent::HistoryUpdated);
        }
        Ok(false) => {}
        Err(error) => {
            diagnostics::record(
                DiagnosticEntry::error("storage", "Could not record a finished game")
                    .with_account(&account.id)
                    .with_game(game_id)
                    .with_detail(error),
            );
        }
    }
}

pub async fn get_scorebook_stats(
    filter: history::ScorebookFilter,
    state: CoreStateRef<'_>,
) -> Result<history::ScorebookStats, String> {
    let (accounts, engines) = {
        let config = state.0.config.read().await;
        (
            config
                .accounts
                .iter()
                .map(|account| history::AccountRef {
                    id: account.id.clone(),
                    username: account.username.clone(),
                })
                .collect(),
            config
                .engines
                .iter()
                .map(|engine| history::EngineRef {
                    id: engine.id.clone(),
                    name: engine.name.clone(),
                })
                .collect(),
        )
    };
    let records = state.0.history.records().await;
    Ok(history::compute_stats(&records, &filter, accounts, engines))
}

pub async fn import_lichess_history(
    account_id: String,
    max: Option<u32>,
    state: CoreStateRef<'_>,
) -> Result<history::ImportReport, String> {
    let account = state
        .0
        .config
        .read()
        .await
        .accounts
        .iter()
        .find(|account| account.id == account_id)
        .cloned()
        .ok_or_else(|| "Lichess account not found".to_string())?;
    let token = state.token(&account_id)?;
    let max = max.unwrap_or(1000).clamp(1, 3000);
    let games = lichess::export_games(
        &state.0.api_base,
        &state.0.client,
        &token,
        &account.username,
        max,
    )
    .await
    .map_err(|error| error.to_string())?;
    let scanned = games.len() as i64;
    let mut imported = 0i64;
    let mut skipped = 0i64;
    for game in &games {
        match history::record_from_lichess_export(game, &account) {
            Some(record) => {
                if state.0.history.append(record).await? {
                    imported += 1;
                } else {
                    skipped += 1;
                }
            }
            None => skipped += 1,
        }
    }
    let _ = state.0.events.send(CoreEvent::HistoryUpdated);
    Ok(history::ImportReport {
        imported,
        skipped,
        scanned,
    })
}

/// Runs a log-store operation off the async runtime.
///
/// Decoding a session is megabytes of gzip plus a full-file parse, and a
/// cross-session search multiplies that by the archive. A command body with no
/// await point holds its runtime worker for the whole duration, competing with
/// the very game streams and engine pipes this feature exists to observe, so
/// every one of them runs on a blocking thread instead.
async fn on_log_store<T, F>(state: &AppState, task: F) -> Result<T, String>
where
    F: FnOnce(&enginelog::EngineLogStore) -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    let logs = state.0.logs.clone();
    let workers = state
        .0
        .blocking_workers
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let permit = workers
        .acquire_owned()
        .await
        .map_err(|_| "The bounded blocking worker pool is unavailable".to_string())?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        task(&logs)
    })
    .await
    .map_err(|error| format!("The log query did not finish: {error}"))?
}

#[cfg(test)]
mod blocking_worker_tests {
    use super::{on_log_store, AppState};
    use crate::test_support::{app_config, temp_root, MemorySecretStore};
    use std::{sync::Arc, time::Duration};
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn abandoned_query_keeps_its_blocking_permit_until_the_worker_really_exits() {
        let root = temp_root("blocking-worker-ceiling");
        let state = AppState::new_with_secret_store(
            root.clone(),
            app_config("unused-engine", false),
            Arc::new(MemorySecretStore::default()),
        )
        .unwrap();
        state.configure_blocking_workers(1).unwrap();

        let (first_started_tx, first_started_rx) = oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let first_state = state.clone();
        let first = tokio::spawn(async move {
            on_log_store(&first_state, move |_| {
                let _ = first_started_tx.send(());
                release_rx.recv().unwrap();
                Ok(())
            })
            .await
        });
        tokio::time::timeout(Duration::from_secs(1), first_started_rx)
            .await
            .unwrap()
            .unwrap();
        first.abort();
        let _ = first.await;

        let (second_started_tx, mut second_started_rx) = oneshot::channel();
        let second_state = state.clone();
        let second = tokio::spawn(async move {
            on_log_store(&second_state, move |_| {
                let _ = second_started_tx.send(());
                Ok(())
            })
            .await
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut second_started_rx)
                .await
                .is_err(),
            "an abandoned JoinHandle must not release a still-running blocking worker"
        );

        release_tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(1), second_started_rx)
            .await
            .unwrap()
            .unwrap();
        second.await.unwrap().unwrap();
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }
}

pub async fn list_log_sessions(
    filter: enginelog::LogFilter,
    state: CoreStateRef<'_>,
) -> Result<Vec<enginelog::LogSessionSummary>, String> {
    on_log_store(&state, move |logs| Ok(logs.list(&filter))).await
}

pub async fn get_log_page(
    session_id: String,
    offset: u64,
    limit: u64,
    state: CoreStateRef<'_>,
) -> Result<enginelog::LogPage, String> {
    on_log_store(&state, move |logs| logs.page(&session_id, offset, limit)).await
}

pub async fn get_log_outline(
    session_id: String,
    state: CoreStateRef<'_>,
) -> Result<Vec<enginelog::LogSearchBlock>, String> {
    on_log_store(&state, move |logs| logs.outline(&session_id)).await
}

pub async fn search_log_session(
    session_id: String,
    query: enginelog::LogQuery,
    state: CoreStateRef<'_>,
) -> Result<Vec<enginelog::LogMatch>, String> {
    on_log_store(&state, move |logs| logs.search(&session_id, &query)).await
}

pub async fn search_log_sessions(
    filter: enginelog::LogFilter,
    query: enginelog::LogQuery,
    state: CoreStateRef<'_>,
) -> Result<Vec<enginelog::LogSessionMatches>, String> {
    on_log_store(&state, move |logs| logs.search_all(&filter, &query)).await
}

pub async fn export_log_session(
    session_id: String,
    path: String,
    mode: String,
    state: CoreStateRef<'_>,
) -> Result<(), String> {
    let mode = enginelog::ExportMode::parse(&mode)?;
    on_log_store(&state, move |logs| {
        logs.export(&session_id, std::path::Path::new(&path), mode)
    })
    .await
}

pub async fn export_log_session_bytes(
    session_id: String,
    mode: String,
    state: CoreStateRef<'_>,
) -> Result<Vec<u8>, String> {
    let mode = enginelog::ExportMode::parse(&mode)?;
    on_log_store(&state, move |logs| logs.export_bytes(&session_id, mode)).await
}

pub async fn export_log_session_bytes_bounded(
    session_id: String,
    mode: enginelog::ExportMode,
    max_bytes: usize,
    state: CoreStateRef<'_>,
) -> Result<Vec<u8>, String> {
    on_log_store(&state, move |logs| {
        logs.export_bytes_bounded(&session_id, mode, max_bytes)
    })
    .await
}

pub async fn delete_log_session(session_id: String, state: CoreStateRef<'_>) -> Result<(), String> {
    on_log_store(&state, move |logs| logs.delete(&session_id)).await?;
    state.emit_logs_updated();
    Ok(())
}

pub async fn clear_log_sessions(state: CoreStateRef<'_>) -> Result<u64, String> {
    let removed = on_log_store(&state, |logs| logs.clear()).await?;
    state.emit_logs_updated();
    Ok(removed)
}

pub async fn get_logs_overview(state: CoreStateRef<'_>) -> Result<enginelog::LogsOverview, String> {
    Ok(state.0.logs.overview())
}

pub async fn set_log_retention(
    retention: enginelog::LogRetention,
    state: CoreStateRef<'_>,
) -> Result<(), String> {
    {
        let mut config = state.0.config.write().await;
        config.log_retention = retention.clone();
        storage::save(&state.0.config_path, &config)?;
    }
    // Applying a policy prunes, which deletes files — off the runtime.
    on_log_store(&state, move |logs| {
        logs.set_retention(retention);
        Ok(())
    })
    .await?;
    state.emit_logs_updated();
    Ok(())
}

pub async fn get_diagnostics(
    filter: diagnostics::DiagnosticFilter,
    state: CoreStateRef<'_>,
) -> Result<Vec<diagnostics::DiagnosticEntry>, String> {
    Ok(state.0.diagnostics.recent(&filter))
}

pub async fn clear_diagnostics(state: CoreStateRef<'_>) -> Result<(), String> {
    state.0.diagnostics.clear()
}

fn player_name(player: Option<&Value>) -> String {
    player
        .and_then(|player| player.get("name").or_else(|| player.get("id")))
        .and_then(Value::as_str)
        .unwrap_or("Lichess opponent")
        .to_string()
}

fn player_rating(player: Option<&Value>) -> Option<i64> {
    player?.get("rating")?.as_i64()
}

fn game_result(status: &str, winner: Option<&str>) -> Option<String> {
    if status == "started" || status == "created" {
        return None;
    }
    match winner {
        Some("white") => Some("1-0".into()),
        Some("black") => Some("0-1".into()),
        _ if status == "aborted" || status == "noStart" => Some("*".into()),
        _ => Some("1/2-1/2".into()),
    }
}

#[cfg(test)]
mod telemetry_capture_tests {
    use super::{build_game_telemetry, config_fingerprint, eval_entry, fnv1a_64, TelemetryCapture};
    use crate::models::{EngineProfile, EngineTelemetry, OpeningBookConfig, UciOption};

    fn info(score_cp: Option<i32>, mate_in: Option<i32>, depth: Option<u32>) -> EngineTelemetry {
        EngineTelemetry {
            score_cp,
            mate_in,
            depth,
            ..EngineTelemetry::default()
        }
    }

    fn option(name: &str, value: Option<&str>) -> UciOption {
        UciOption {
            name: name.into(),
            option_type: "spin".into(),
            default_value: None,
            value: value.map(str::to_string),
            min: None,
            max: None,
            choices: Vec::new(),
        }
    }

    fn profile(options: Vec<UciOption>, book: Option<OpeningBookConfig>) -> EngineProfile {
        EngineProfile {
            id: "engine-1".into(),
            name: "Stockfish".into(),
            path: "/engines/stockfish".into(),
            author: None,
            option_count: options.len(),
            last_probed_at_ms: None,
            probe_ok: None,
            options,
            opening_book: book,
        }
    }

    fn book() -> OpeningBookConfig {
        OpeningBookConfig {
            enabled: true,
            path: "/books/main.bin".into(),
            name: "Main".into(),
            format: "polyglot".into(),
            max_plies: 12,
            top_move_percent: 80,
            entry_count: 100,
        }
    }

    #[test]
    fn maps_scores_to_clamped_our_perspective_evals() {
        assert_eq!(eval_entry(Some(&info(Some(85), None, None)), None), 85);
        assert_eq!(eval_entry(Some(&info(Some(2500), None, None)), None), 1000);
        assert_eq!(
            eval_entry(Some(&info(Some(-4000), None, None)), None),
            -1000
        );
        // Mate scores map to +/-1000 regardless of any cp value present.
        assert_eq!(eval_entry(Some(&info(None, Some(4), None)), None), 1000);
        assert_eq!(
            eval_entry(Some(&info(None, Some(-2), None)), Some(300)),
            -1000
        );
        // No score at all (book move or scoreless search): repeat the previous
        // snapshot, or 0 when there is none yet.
        assert_eq!(eval_entry(None, Some(140)), 140);
        assert_eq!(eval_entry(None, None), 0);
        assert_eq!(
            eval_entry(Some(&info(None, None, Some(20))), Some(-60)),
            -60
        );
    }

    #[test]
    fn capture_flattens_into_a_game_telemetry_record() {
        let mut capture = TelemetryCapture::default();
        // Two book moves before any search: evals default to 0.
        capture.record_submission(None, 5, true);
        capture.record_submission(None, 7, true);
        capture.record_submission(Some(&info(Some(120), None, Some(22))), 900, false);
        capture.record_submission(Some(&info(Some(-150), None, Some(18))), 1_400, false);
        capture.record_submission(Some(&info(None, Some(-1), Some(30))), 700, false);
        capture.end_clock_ms = Some(12_345);
        capture.engine_restarts = 1;
        capture.stream_reconnects = 2;
        capture
            .submission_retries
            .fetch_add(4, std::sync::atomic::Ordering::Relaxed);

        let telemetry = build_game_telemetry(&capture, "abcdefabcdef".into());
        assert_eq!(telemetry.eval_series_cp, vec![0, 0, 120, -150, -1000]);
        assert_eq!(telemetry.book_plies, 2);
        // Depths only come from searched moves: 22, 18, 30.
        assert!((telemetry.avg_depth.expect("avg depth") - 70.0 / 3.0).abs() < 1e-9);
        assert_eq!(telemetry.min_depth, Some(18));
        // Move times cover all five submissions.
        assert!((telemetry.avg_move_time_ms.expect("avg move time") - 3_012.0 / 5.0).abs() < 1e-9);
        assert_eq!(telemetry.max_move_time_ms, Some(1_400));
        assert_eq!(telemetry.end_clock_ms, Some(12_345));
        assert_eq!(telemetry.engine_restarts, 1);
        assert_eq!(telemetry.stream_reconnects, 2);
        assert_eq!(telemetry.submission_retries, 4);
        assert_eq!(telemetry.max_eval_cp, Some(120));
        assert_eq!(telemetry.min_eval_cp, Some(-1000));
        // Drops: 120 -> -150 (270) and -150 -> -1000 (850).
        assert_eq!(telemetry.blunders, 2);
        assert_eq!(
            telemetry.config_fingerprint.as_deref(),
            Some("abcdefabcdef")
        );
        assert!(!telemetry.failure_resign);
    }

    #[test]
    fn empty_capture_produces_empty_aggregates() {
        let telemetry = build_game_telemetry(&TelemetryCapture::default(), "aaaaaaaaaaaa".into());
        assert!(telemetry.eval_series_cp.is_empty());
        assert!(telemetry.avg_depth.is_none());
        assert!(telemetry.min_depth.is_none());
        assert!(telemetry.avg_move_time_ms.is_none());
        assert!(telemetry.max_move_time_ms.is_none());
        assert!(telemetry.max_eval_cp.is_none());
        assert!(telemetry.min_eval_cp.is_none());
        assert_eq!(telemetry.blunders, 0);
    }

    #[test]
    fn fingerprint_is_stable_and_sensitive_to_configuration() {
        let base = profile(
            vec![option("Hash", Some("256")), option("Threads", Some("4"))],
            Some(book()),
        );
        let fingerprint = config_fingerprint(&base);
        assert_eq!(fingerprint.len(), 12);
        assert!(fingerprint.chars().all(|c| c.is_ascii_hexdigit()));
        // Same inputs, same hash.
        assert_eq!(fingerprint, config_fingerprint(&base));
        // Option order does not matter: the canonical form is sorted.
        let reordered = profile(
            vec![option("Threads", Some("4")), option("Hash", Some("256"))],
            Some(book()),
        );
        assert_eq!(fingerprint, config_fingerprint(&reordered));
        // A changed option value changes the hash.
        let retuned = profile(
            vec![option("Hash", Some("512")), option("Threads", Some("4"))],
            Some(book()),
        );
        assert_ne!(fingerprint, config_fingerprint(&retuned));
        // Valueless options are ignored.
        let with_unset = profile(
            vec![
                option("Hash", Some("256")),
                option("Threads", Some("4")),
                option("SyzygyPath", None),
            ],
            Some(book()),
        );
        assert_eq!(fingerprint, config_fingerprint(&with_unset));
        // Book configuration participates.
        let no_book = profile(
            vec![option("Hash", Some("256")), option("Threads", Some("4"))],
            None,
        );
        assert_ne!(fingerprint, config_fingerprint(&no_book));
        let mut shallow_book = book();
        shallow_book.max_plies = 6;
        let with_shallow_book = profile(
            vec![option("Hash", Some("256")), option("Threads", Some("4"))],
            Some(shallow_book),
        );
        assert_ne!(fingerprint, config_fingerprint(&with_shallow_book));
    }

    #[test]
    fn fnv1a_matches_known_vectors() {
        // Published FNV-1a 64-bit test vectors.
        assert_eq!(fnv1a_64(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a_64("a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a_64("foobar"), 0x85944171f73967e8);
    }
}

#[cfg(test)]
mod game_lifecycle_tests {
    use super::game_result;

    #[test]
    fn maps_terminal_lichess_states_to_pgn_results() {
        assert_eq!(game_result("started", None), None);
        assert_eq!(game_result("mate", Some("white")).as_deref(), Some("1-0"));
        assert_eq!(game_result("resign", Some("black")).as_deref(), Some("0-1"));
        assert_eq!(game_result("draw", None).as_deref(), Some("1/2-1/2"));
        assert_eq!(game_result("aborted", None).as_deref(), Some("*"));
    }
}

#[cfg(test)]
mod submission_safety_tests {
    use super::{
        run_submission, should_auto_resign, submission_retry_delay, AutoResignReason,
        LichessMoveTransport, MoveTransport, SubmissionCoordinator, SubmissionObservation,
        SubmissionOutcome, SubmissionPolicy, SubmissionRequest,
    };
    use crate::lichess::{LichessError, LichessErrorKind};
    use reqwest::StatusCode;
    use std::{
        collections::VecDeque,
        future::Future,
        pin::Pin,
        sync::{
            atomic::{AtomicI64, AtomicUsize, Ordering},
            Arc, Mutex,
        },
        time::Duration,
    };
    use tokio::sync::{watch, Notify};
    use tokio_util::sync::CancellationToken;

    struct AcceptanceMoveTransport {
        http: LichessMoveTransport,
        inject_transport_ambiguity: std::sync::atomic::AtomicBool,
        active: AtomicUsize,
        maximum_active: AtomicUsize,
    }

    impl MoveTransport for AcceptanceMoveTransport {
        fn submit<'a>(
            &'a self,
            game_id: &'a str,
            chess_move: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), LichessError>> + Send + 'a>> {
            Box::pin(async move {
                let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
                self.maximum_active.fetch_max(active, Ordering::SeqCst);
                let result = if chess_move == "transport"
                    && !self.inject_transport_ambiguity.swap(true, Ordering::SeqCst)
                {
                    Err(failure(None, Some(Duration::from_millis(1))))
                } else {
                    self.http.submit(game_id, chess_move).await
                };
                self.active.fetch_sub(1, Ordering::SeqCst);
                result
            })
        }
    }

    struct MockMoveTransport {
        results: Mutex<VecDeque<Result<(), LichessError>>>,
        calls: AtomicUsize,
    }

    impl MoveTransport for MockMoveTransport {
        fn submit<'a>(
            &'a self,
            _game_id: &'a str,
            _chess_move: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), LichessError>> + Send + 'a>> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            let result = self.results.lock().unwrap().pop_front().unwrap_or(Ok(()));
            Box::pin(async move { result })
        }
    }

    struct BlockingMoveTransport {
        entered: Arc<Notify>,
    }

    impl MoveTransport for BlockingMoveTransport {
        fn submit<'a>(
            &'a self,
            _game_id: &'a str,
            _chess_move: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), LichessError>> + Send + 'a>> {
            Box::pin(async move {
                self.entered.notify_one();
                std::future::pending().await
            })
        }
    }

    fn failure(status: Option<StatusCode>, retry_after: Option<Duration>) -> LichessError {
        LichessError {
            kind: if status.is_some() {
                LichessErrorKind::Http
            } else {
                LichessErrorKind::Transport
            },
            status,
            code: None,
            retry_after,
            body: "mock failure".into(),
            operation: "mock submit",
            ambiguous_write: true,
        }
    }

    #[tokio::test]
    async fn mocked_submitter_honors_retry_after_and_stays_single_worker() {
        let transport = MockMoveTransport {
            results: Mutex::new(VecDeque::from([
                Err(failure(
                    Some(StatusCode::TOO_MANY_REQUESTS),
                    Some(Duration::from_millis(2)),
                )),
                Ok(()),
            ])),
            calls: AtomicUsize::new(0),
        };
        let (_observations, mut receiver) = watch::channel(SubmissionObservation {
            moves: "e2e4".into(),
            terminal: false,
        });
        let retries = AtomicI64::new(0);
        let outcome = run_submission(
            &transport,
            "game",
            &SubmissionRequest {
                position_moves: "e2e4".into(),
                selected_move: "e7e5".into(),
            },
            &mut receiver,
            &CancellationToken::new(),
            SubmissionPolicy {
                budget: Duration::from_secs(1),
                max_attempts: 3,
            },
            &retries,
        )
        .await;
        assert_eq!(outcome, SubmissionOutcome::Submitted);
        assert_eq!(transport.calls.load(Ordering::Relaxed), 2);
        assert_eq!(retries.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn authoritative_progress_cancels_an_ambiguous_retry() {
        let transport = Arc::new(MockMoveTransport {
            results: Mutex::new(VecDeque::from([Err(failure(
                None,
                Some(Duration::from_secs(1)),
            ))])),
            calls: AtomicUsize::new(0),
        });
        let (observations, mut receiver) = watch::channel(SubmissionObservation {
            moves: "e2e4".into(),
            terminal: false,
        });
        let update = tokio::spawn(async move {
            tokio::task::yield_now().await;
            observations.send_replace(SubmissionObservation {
                moves: "e2e4 e7e5".into(),
                terminal: false,
            });
        });
        let outcome = run_submission(
            transport.as_ref(),
            "game",
            &SubmissionRequest {
                position_moves: "e2e4".into(),
                selected_move: "e7e5".into(),
            },
            &mut receiver,
            &CancellationToken::new(),
            SubmissionPolicy {
                budget: Duration::from_secs(2),
                max_attempts: 3,
            },
            &AtomicI64::new(0),
        )
        .await;
        update.await.unwrap();
        assert_eq!(outcome, SubmissionOutcome::Reconciled);
        assert_eq!(transport.calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn cancellation_interrupts_an_in_flight_transport() {
        let entered = Arc::new(Notify::new());
        let transport = BlockingMoveTransport {
            entered: entered.clone(),
        };
        let (_observations, mut receiver) = watch::channel(SubmissionObservation::default());
        let cancellation = CancellationToken::new();
        let stop = cancellation.clone();
        let canceler = tokio::spawn(async move {
            entered.notified().await;
            stop.cancel();
        });
        let outcome = tokio::time::timeout(
            Duration::from_secs(1),
            run_submission(
                &transport,
                "game",
                &SubmissionRequest {
                    position_moves: String::new(),
                    selected_move: "e2e4".into(),
                },
                &mut receiver,
                &cancellation,
                SubmissionPolicy {
                    budget: Duration::from_secs(1),
                    max_attempts: 3,
                },
                &AtomicI64::new(0),
            ),
        )
        .await
        .expect("submission cancellation bound");
        canceler.await.unwrap();
        assert_eq!(outcome, SubmissionOutcome::Canceled);
    }

    #[test]
    fn retry_delay_uses_typed_retry_after_and_policy_is_narrow() {
        let error = failure(
            Some(StatusCode::TOO_MANY_REQUESTS),
            Some(Duration::from_secs(7)),
        );
        assert_eq!(
            submission_retry_delay(&error, 1, false),
            Duration::from_secs(7)
        );
        assert!(should_auto_resign(
            AutoResignReason::EngineUnavailable,
            true
        ));
        assert!(!should_auto_resign(
            AutoResignReason::EngineUnavailable,
            false
        ));
    }

    async fn one_submission(
        results: Vec<Result<(), LichessError>>,
        observation: SubmissionObservation,
        policy: SubmissionPolicy,
    ) -> (SubmissionOutcome, usize) {
        let transport = MockMoveTransport {
            results: Mutex::new(results.into()),
            calls: AtomicUsize::new(0),
        };
        let (_observations, mut receiver) = watch::channel(observation);
        let outcome = run_submission(
            &transport,
            "game",
            &SubmissionRequest {
                position_moves: "e2e4".into(),
                selected_move: "e7e5".into(),
            },
            &mut receiver,
            &CancellationToken::new(),
            policy,
            &AtomicI64::new(0),
        )
        .await;
        (outcome, transport.calls.load(Ordering::Relaxed))
    }

    #[tokio::test]
    async fn typed_http_matrix_rejects_or_retries_only_the_intended_classes() {
        let policy = SubmissionPolicy {
            budget: Duration::from_secs(1),
            max_attempts: 2,
        };
        for status in [
            StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN,
            StatusCode::NOT_FOUND,
            StatusCode::CONFLICT,
            StatusCode::UNPROCESSABLE_ENTITY,
        ] {
            let (outcome, calls) = one_submission(
                vec![Err(failure(Some(status), None))],
                SubmissionObservation {
                    moves: "e2e4".into(),
                    terminal: false,
                },
                policy,
            )
            .await;
            assert!(
                matches!(outcome, SubmissionOutcome::Rejected(_)),
                "{status}"
            );
            assert_eq!(calls, 1, "{status}");
        }

        for error in [
            failure(
                Some(StatusCode::INTERNAL_SERVER_ERROR),
                Some(Duration::from_millis(1)),
            ),
            failure(None, Some(Duration::from_millis(1))),
        ] {
            let (outcome, calls) = one_submission(
                vec![Err(error), Ok(())],
                SubmissionObservation {
                    moves: "e2e4".into(),
                    terminal: false,
                },
                policy,
            )
            .await;
            assert_eq!(outcome, SubmissionOutcome::Submitted);
            assert_eq!(calls, 2);
        }

        let (terminal, calls) = one_submission(
            vec![Err(failure(None, Some(Duration::from_secs(1))))],
            SubmissionObservation {
                moves: "e2e4".into(),
                terminal: true,
            },
            policy,
        )
        .await;
        assert_eq!(terminal, SubmissionOutcome::Reconciled);
        assert_eq!(calls, 1);

        let (exhausted, calls) = one_submission(
            vec![
                Err(failure(
                    Some(StatusCode::SERVICE_UNAVAILABLE),
                    Some(Duration::from_millis(1)),
                )),
                Err(failure(Some(StatusCode::SERVICE_UNAVAILABLE), None)),
            ],
            SubmissionObservation {
                moves: "e2e4".into(),
                terminal: false,
            },
            policy,
        )
        .await;
        assert!(matches!(exhausted, SubmissionOutcome::Exhausted(_)));
        assert_eq!(calls, 2);

        let base = failure(
            Some(StatusCode::TOO_MANY_REQUESTS),
            Some(Duration::from_millis(100)),
        );
        for _ in 0..32 {
            let delay = submission_retry_delay(&base, 1, true);
            assert!((Duration::from_millis(100)..=Duration::from_millis(120)).contains(&delay));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn elapsed_budget_exhausts_ambiguity_before_the_attempt_limit() {
        let policy = SubmissionPolicy {
            budget: Duration::from_millis(100),
            max_attempts: 10,
        };
        let failures = (0..policy.max_attempts)
            .map(|_| Err(failure(Some(StatusCode::INTERNAL_SERVER_ERROR), None)))
            .collect();
        let (outcome, calls) = one_submission(
            failures,
            SubmissionObservation {
                moves: "e2e4".into(),
                terminal: false,
            },
            policy,
        )
        .await;
        assert!(matches!(outcome, SubmissionOutcome::Exhausted(_)));
        assert!(calls < policy.max_attempts as usize, "made {calls} calls");
        assert_eq!(calls, 2);
    }

    #[tokio::test]
    async fn one_coordinator_drives_the_loopback_http_matrix_and_joins_before_stop_returns() {
        use crate::test_support::{
            app_config, temp_root, MemorySecretStore, ScriptReply, ScriptedHttp,
        };

        let http = ScriptedHttp::start().await;
        for (chess_move, status) in [
            ("auth401", StatusCode::UNAUTHORIZED),
            ("auth403", StatusCode::FORBIDDEN),
            ("missing404", StatusCode::NOT_FOUND),
            ("client409", StatusCode::CONFLICT),
            ("client422", StatusCode::UNPROCESSABLE_ENTITY),
        ] {
            http.push(
                "POST",
                &format!("/api/bot/game/game/move/{chess_move}"),
                ScriptReply::Json(status, r#"{"error":"rejected"}"#.into()),
            );
        }
        for chess_move in ["server500", "date429", "malformed429"] {
            let (status, retry_after) = if chess_move == "server500" {
                (StatusCode::INTERNAL_SERVER_ERROR, "0")
            } else if chess_move == "date429" {
                (
                    StatusCode::TOO_MANY_REQUESTS,
                    "Sun, 06 Nov 1994 08:49:37 GMT",
                )
            } else {
                (StatusCode::TOO_MANY_REQUESTS, "not-a-delay")
            };
            http.push(
                "POST",
                &format!("/api/bot/game/game/move/{chess_move}"),
                ScriptReply::JsonWithHeaders(
                    status,
                    r#"{"error":"retry"}"#.into(),
                    vec![("retry-after", retry_after)],
                ),
            );
            http.push(
                "POST",
                &format!("/api/bot/game/game/move/{chess_move}"),
                ScriptReply::Json(StatusCode::OK, "{}".into()),
            );
        }
        http.push(
            "POST",
            "/api/bot/game/game/move/transport",
            ScriptReply::Json(StatusCode::OK, "{}".into()),
        );
        http.push(
            "POST",
            "/api/bot/game/game/move/terminal",
            ScriptReply::JsonWithHeaders(
                StatusCode::SERVICE_UNAVAILABLE,
                r#"{"error":"uncertain"}"#.into(),
                vec![("retry-after", "0")],
            ),
        );
        for _ in 0..2 {
            http.push(
                "POST",
                "/api/bot/game/game/move/budget",
                ScriptReply::JsonWithHeaders(
                    StatusCode::SERVICE_UNAVAILABLE,
                    r#"{"error":"still uncertain"}"#.into(),
                    vec![("retry-after", "0")],
                ),
            );
        }
        for index in 0..5 {
            http.push(
                "POST",
                &format!("/api/bot/game/game/move/concurrent{index}"),
                ScriptReply::Json(StatusCode::OK, "{}".into()),
            );
        }
        http.push(
            "POST",
            "/api/bot/game/game/move/blocked",
            ScriptReply::Delay(
                Duration::from_secs(60),
                Box::new(ScriptReply::Json(StatusCode::OK, "{}".into())),
            ),
        );

        let root = temp_root("coordinator-http-matrix");
        let app = super::AppState::new_with_test_api(
            root.clone(),
            app_config("unused-engine", false),
            Arc::new(MemorySecretStore::with("bot", "token")),
            http.base(),
        )
        .unwrap();
        let transport = Arc::new(AcceptanceMoveTransport {
            http: LichessMoveTransport {
                base: http.base(),
                client: app.0.api_client.clone(),
                token: "token".into(),
            },
            inject_transport_ambiguity: std::sync::atomic::AtomicBool::new(false),
            active: AtomicUsize::new(0),
            maximum_active: AtomicUsize::new(0),
        });
        let retries = Arc::new(AtomicI64::new(0));
        let parent = CancellationToken::new();
        let (mut coordinator, mut outcomes) = SubmissionCoordinator::start_for_test(
            app,
            ("bot".into(), "game".into()),
            transport.clone(),
            retries.clone(),
            &parent,
            SubmissionPolicy {
                budget: Duration::from_secs(2),
                max_attempts: 2,
            },
        );

        for chess_move in ["auth401", "auth403", "missing404", "client409", "client422"] {
            coordinator.observe(chess_move.into(), false);
            coordinator
                .submit(chess_move.into(), chess_move.into())
                .await
                .unwrap();
            let outcome = tokio::time::timeout(Duration::from_secs(2), outcomes.recv())
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(outcome, SubmissionOutcome::Rejected(_)));
            assert_eq!(
                http.count("POST", &format!("/api/bot/game/game/move/{chess_move}")),
                1
            );
        }
        for chess_move in ["server500", "transport", "date429", "malformed429"] {
            coordinator.observe(chess_move.into(), false);
            coordinator
                .submit(chess_move.into(), chess_move.into())
                .await
                .unwrap();
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(2), outcomes.recv())
                    .await
                    .unwrap()
                    .unwrap(),
                SubmissionOutcome::Submitted
            );
        }
        assert_eq!(http.count("POST", "/api/bot/game/game/move/server500"), 2);
        assert_eq!(
            http.count("POST", "/api/bot/game/game/move/transport"),
            1,
            "the first transport-ambiguous attempt happens before an HTTP response"
        );
        assert_eq!(http.count("POST", "/api/bot/game/game/move/date429"), 2);
        assert_eq!(
            http.count("POST", "/api/bot/game/game/move/malformed429"),
            2
        );

        coordinator.observe("terminal".into(), true);
        coordinator
            .submit("terminal".into(), "terminal".into())
            .await
            .unwrap();
        assert_eq!(
            outcomes.recv().await.unwrap(),
            SubmissionOutcome::Reconciled
        );
        assert_eq!(http.count("POST", "/api/bot/game/game/move/terminal"), 1);

        coordinator.observe("budget".into(), false);
        coordinator
            .submit("budget".into(), "budget".into())
            .await
            .unwrap();
        assert!(matches!(
            outcomes.recv().await.unwrap(),
            SubmissionOutcome::Exhausted(_)
        ));
        assert_eq!(http.count("POST", "/api/bot/game/game/move/budget"), 2);

        coordinator.observe("concurrent".into(), false);
        futures_util::future::join_all(
            (0..5)
                .map(|index| coordinator.submit("concurrent".into(), format!("concurrent{index}"))),
        )
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
        for _ in 0..5 {
            assert_eq!(outcomes.recv().await.unwrap(), SubmissionOutcome::Submitted);
        }
        assert_eq!(transport.maximum_active.load(Ordering::SeqCst), 1);

        coordinator.observe("blocked".into(), false);
        coordinator
            .submit("blocked".into(), "blocked".into())
            .await
            .unwrap();
        http.wait_for_count("POST", "/api/bot/game/game/move/blocked", 1)
            .await;
        coordinator.shutdown().await.unwrap();
        assert_eq!(outcomes.recv().await.unwrap(), SubmissionOutcome::Canceled);
        let calls_after_join = http.requests().len();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(http.requests().len(), calls_after_join);
        assert!(retries.load(Ordering::Relaxed) >= 5);
        let _ = std::fs::remove_dir_all(root);
    }

    struct SerialMoveTransport {
        calls: AtomicUsize,
        active: AtomicUsize,
        maximum_active: AtomicUsize,
        changed: Notify,
    }

    impl MoveTransport for SerialMoveTransport {
        fn submit<'a>(
            &'a self,
            _game_id: &'a str,
            _chess_move: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), LichessError>> + Send + 'a>> {
            Box::pin(async move {
                let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
                self.maximum_active.fetch_max(active, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(2)).await;
                self.active.fetch_sub(1, Ordering::SeqCst);
                self.calls.fetch_add(1, Ordering::SeqCst);
                self.changed.notify_waiters();
                Ok(())
            })
        }
    }

    #[tokio::test]
    async fn one_game_coordinator_serializes_concurrent_submissions_and_joins_on_stop() {
        let root = crate::test_support::temp_root("submission-owner");
        let app = super::AppState::new_with_secret_store(
            root.clone(),
            crate::test_support::app_config("unused-engine", false),
            Arc::new(crate::test_support::MemorySecretStore::with("bot", "token")),
        )
        .unwrap();
        let transport = Arc::new(SerialMoveTransport {
            calls: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            maximum_active: AtomicUsize::new(0),
            changed: Notify::new(),
        });
        let parent = CancellationToken::new();
        let mut coordinator = SubmissionCoordinator::start(
            app,
            ("bot".into(), "game".into()),
            transport.clone(),
            Arc::new(AtomicI64::new(0)),
            &parent,
        );
        futures_util::future::join_all(
            (0..5).map(|index| {
                coordinator.submit(format!("position-{index}"), format!("move-{index}"))
            }),
        )
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while transport.calls.load(Ordering::SeqCst) < 5 {
                transport.changed.notified().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(transport.maximum_active.load(Ordering::SeqCst), 1);
        coordinator.shutdown().await.unwrap();
        let calls_after_join = transport.calls.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(transport.calls.load(Ordering::SeqCst), calls_after_join);
        let _ = std::fs::remove_dir_all(root);
    }
}

#[cfg(test)]
mod owned_lifecycle_acceptance_tests {
    use super::{
        add_lichess_account, create_challenge, handle_account_event, remove_lichess_account,
        spawn_game_wrapper, spawn_supervisor_wrapper, update_lichess_account_token, AppState,
        CoreStateRef, GameTask, SupervisorTask, TASK_JOIN_TIMEOUT,
    };
    #[cfg(not(windows))]
    use super::{process_game_event, GameContext, MoveTransport, SubmissionCoordinator};
    use crate::models::{AddAccountRequest, CampaignSettings, ChallengeRequest};
    use crate::storage::{self, FileSecretStore, SecretStore};
    use crate::test_support::{
        app_config, temp_root, MemorySecretStore, ScriptReply, ScriptedHttp,
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::Semaphore;
    use tokio_util::sync::CancellationToken;

    fn standard_challenge_request() -> ChallengeRequest {
        ChallengeRequest {
            account_id: "bot".into(),
            opponent: "Opponent".into(),
            clock_limit: 180,
            clock_increment: 2,
            rated: false,
            color: "random".into(),
            variant: "standard".into(),
        }
    }

    async fn install_online_supervisor(state: &AppState, generation: u64) {
        let cancellation = CancellationToken::new();
        let task_cancellation = cancellation.clone();
        state.0.supervisors.lock().await.insert(
            "bot".into(),
            SupervisorTask {
                generation,
                cancellation,
                handle: Some(tokio::spawn(async move {
                    task_cancellation.cancelled().await;
                })),
            },
        );
        state.set_runtime("bot", "online", None).await;
    }

    #[tokio::test]
    async fn embedded_account_validation_result_distinguishes_missing_bot_play() {
        let http = ScriptedHttp::start().await;
        http.push(
            "GET",
            "/api/account",
            ScriptReply::JsonWithHeaders(
                axum::http::StatusCode::OK,
                r#"{"id":"bot","username":"Bot","title":"BOT","perfs":{"blitz":{"rating":2100}}}"#
                    .into(),
                vec![("x-oauth-scopes", "challenge:read challenge:write")],
            ),
        );
        let root = temp_root("account-scope-result");
        let mut config = app_config("unused-engine", false);
        config.accounts.clear();
        let state = AppState::new_with_test_api(
            root.clone(),
            config,
            Arc::new(MemorySecretStore::default()),
            http.base(),
        )
        .unwrap();

        let result = add_lichess_account(
            AddAccountRequest {
                token: "token".into(),
                engine_id: "engine".into(),
            },
            CoreStateRef::new(&state),
        )
        .await
        .unwrap();

        assert_eq!(result.account.id, "bot");
        assert_eq!(result.scopes, ["challenge:read", "challenge:write"]);
        assert_eq!(result.missing_for_matchmaking, ["bot:play"]);
        assert!(!result.can_play_games);
        assert_eq!(state.snapshot().await.accounts.len(), 1);
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn token_swap_preserves_campaign_profile_and_runtime_and_returns_scopes() {
        let http = ScriptedHttp::start().await;
        http.push(
            "GET",
            "/api/account",
            ScriptReply::JsonWithHeaders(
                axum::http::StatusCode::OK,
                r#"{"id":"bot","username":"Bot","title":"BOT","perfs":{"blitz":{"rating":2200}}}"#
                    .into(),
                vec![("x-oauth-scopes", "bot:play challenge:write")],
            ),
        );
        let root = temp_root("token-swap-preserves-state");
        let mut config = app_config("unused-engine", true);
        config.campaigns.push(CampaignSettings {
            account_id: "bot".into(),
            min_rating: 1900,
            max_rating: 2300,
            concurrency: 3,
            clock_limit: 180,
            clock_increment: 2,
            rated: true,
            color: "random".into(),
        });
        let secrets = Arc::new(FileSecretStore::new(root.join("secrets")));
        secrets.store("bot", "old-token").unwrap();
        let state = AppState::new_with_test_api(root.clone(), config, secrets.clone(), http.base())
            .unwrap();
        state
            .set_runtime(
                "bot",
                "reconnecting",
                Some("the old token was revoked".into()),
            )
            .await;
        let before = state.snapshot().await;

        let result = update_lichess_account_token(
            "bot".into(),
            " replacement-token ".into(),
            CoreStateRef::new(&state),
        )
        .await
        .unwrap();
        let after = state.snapshot().await;

        assert_eq!(secrets.get("bot").unwrap(), "replacement-token");
        assert_eq!(
            serde_json::to_value(&after.accounts).unwrap(),
            serde_json::to_value(&before.accounts).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&after.campaigns).unwrap(),
            serde_json::to_value(&before.campaigns).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&after.runtimes).unwrap(),
            serde_json::to_value(&before.runtimes).unwrap()
        );
        assert_eq!(result.account.username, "Bot");
        assert_eq!(result.account.rating, Some(2000));
        assert_eq!(result.scopes, ["bot:play", "challenge:write"]);
        assert_eq!(result.missing_for_matchmaking, ["challenge:read"]);
        assert!(result.can_play_games);
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn token_swap_refuses_a_token_for_a_different_username() {
        let http = ScriptedHttp::start().await;
        http.push(
            "GET",
            "/api/account",
            ScriptReply::Json(
                axum::http::StatusCode::OK,
                r#"{"id":"otherbot","username":"OtherBot","title":"BOT","perfs":{}}"#.into(),
            ),
        );
        let root = temp_root("token-swap-wrong-account");
        let secrets = Arc::new(MemorySecretStore::with("bot", "old-token"));
        let state = AppState::new_with_test_api(
            root.clone(),
            app_config("unused-engine", false),
            secrets.clone(),
            http.base(),
        )
        .unwrap();

        let error = update_lichess_account_token(
            "bot".into(),
            "wrong-token".into(),
            CoreStateRef::new(&state),
        )
        .await
        .unwrap_err();

        assert_eq!(
            error,
            "The Lichess token belongs to @OtherBot (otherbot), but the selected account is @Bot (bot)."
        );
        assert_eq!(secrets.get("bot").unwrap(), "old-token");
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn quiesced_game_start_survives_refusal_and_reconciliation_spawns_the_game() {
        let http = ScriptedHttp::start().await;
        http.push(
            "GET",
            "/api/challenge",
            ScriptReply::Json(axum::http::StatusCode::OK, r#"{"out":[]}"#.into()),
        );
        http.push(
            "GET",
            "/api/account/playing",
            ScriptReply::Json(
                axum::http::StatusCode::OK,
                r#"{"nowPlaying":[{"gameId":"challenge"}]}"#.into(),
            ),
        );
        http.push(
            "GET",
            "/api/account/playing",
            ScriptReply::Json(axum::http::StatusCode::OK, r#"{"nowPlaying":[]}"#.into()),
        );
        let game_stream = CancellationToken::new();
        http.push(
            "GET",
            "/api/bot/game/stream/challenge",
            ScriptReply::NdjsonHold(String::new(), game_stream.clone()),
        );
        let root = temp_root("quiesce-game-start");
        let state = AppState::new_with_test_api(
            root.clone(),
            app_config("unused-engine", true),
            Arc::new(MemorySecretStore::with("bot", "token")),
            http.base(),
        )
        .unwrap();
        install_online_supervisor(&state, 1).await;
        let config = state.0.config.read().await;
        let account = config.accounts[0].clone();
        let engine = config.engines[0].clone();
        drop(config);
        state
            .remember_known_outgoing_challenge("bot", "challenge", "Opponent")
            .await;
        let quiesce = state.quiesce().await;

        let error = handle_account_event(
            &state,
            &account,
            &engine,
            "token",
            &CancellationToken::new(),
            1,
            r#"{"type":"gameStart","game":{"gameId":"challenge","opponent":{"username":"Opponent"}}}"#,
        )
        .await
        .unwrap_err();

        assert_eq!(
            error,
            "QueenUI is changing runners; gameStart was deferred for startup reconciliation"
        );
        assert!(state
            .0
            .known_outgoing_challenges
            .lock()
            .await
            .contains_key(&("bot".into(), "challenge".into())));
        assert_eq!(
            storage::load_active_game_intents(&storage::active_game_intents_path(&root)).unwrap(),
            vec![storage::ActiveGameIntent {
                account_id: "bot".into(),
                game_id: "challenge".into(),
            }]
        );
        assert_eq!(quiesce.live_game_count().await, 1);
        assert_eq!(
            quiesce.verify_authoritative_handover().await.unwrap_err(),
            "Lichess account Bot still has 1 live game (challenge); finish or resign them before switching to a runner."
        );

        quiesce.restore().await;
        assert!(state
            .0
            .game_tasks
            .lock()
            .await
            .contains_key(&("bot".into(), "challenge".into())));
        assert_eq!(http.count("GET", "/api/account/playing"), 2);
        assert_eq!(http.count("GET", "/api/stream/event"), 0);
        assert!(state
            .0
            .diagnostics
            .recent(&crate::diagnostics::DiagnosticFilter {
                account_id: Some("bot".into()),
                query: Some("game start refused".into()),
                ..Default::default()
            })
            .iter()
            .any(|entry| entry.message == "Game start refused while changing runners"));

        state.stop_bot_owned("bot", false, false).await.unwrap();
        game_stream.cancel();
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn quiesced_game_start_intent_persists_when_the_switch_succeeds() {
        let root = temp_root("quiesce-game-start-success");
        let state = AppState::new_with_secret_store(
            root.clone(),
            app_config("unused-engine", true),
            Arc::new(MemorySecretStore::with("bot", "token")),
        )
        .unwrap();
        install_online_supervisor(&state, 1).await;
        let config = state.0.config.read().await;
        let account = config.accounts[0].clone();
        let engine = config.engines[0].clone();
        drop(config);
        state
            .remember_known_outgoing_challenge("bot", "challenge", "Opponent")
            .await;
        let quiesce = state.quiesce().await;

        handle_account_event(
            &state,
            &account,
            &engine,
            "token",
            &CancellationToken::new(),
            1,
            r#"{"type":"gameStart","game":{"gameId":"challenge","opponent":{"username":"Opponent"}}}"#,
        )
        .await
        .unwrap_err();
        quiesce.shutdown().await.unwrap();

        assert_eq!(
            storage::load_active_game_intents(&storage::active_game_intents_path(&root)).unwrap(),
            vec![storage::ActiveGameIntent {
                account_id: "bot".into(),
                game_id: "challenge".into(),
            }]
        );
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn canceled_quiesce_acquisition_does_not_poison_supervisor_or_game_admission() {
        let http = ScriptedHttp::start().await;
        http.push(
            "GET",
            "/api/account/playing",
            ScriptReply::Json(axum::http::StatusCode::OK, r#"{"nowPlaying":[]}"#.into()),
        );
        let stream_release = CancellationToken::new();
        http.push(
            "GET",
            "/api/stream/event",
            ScriptReply::NdjsonHold(String::new(), stream_release.clone()),
        );
        let root = temp_root("canceled-quiesce");
        let state = AppState::new_with_test_api(
            root.clone(),
            app_config("unused-engine", false),
            Arc::new(MemorySecretStore::with("bot", "token")),
            http.base(),
        )
        .unwrap();
        let existing_reader = state.0.ownership_admission.clone().read_owned().await;
        let quiescing_state = state.clone();
        let acquisition = tokio::spawn(async move { quiescing_state.quiesce().await });
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert!(!acquisition.is_finished());
        acquisition.abort();
        let _ = acquisition.await;
        assert!(!state.0.quiescing.load(Ordering::Acquire));
        drop(existing_reader);

        state.start_bot("bot").await.unwrap();
        let config = state.0.config.read().await;
        let account = config.accounts[0].clone();
        let engine = config.engines[0].clone();
        drop(config);
        let (generation, game_cancellation) = {
            let supervisors = state.0.supervisors.lock().await;
            let supervisor = &supervisors["bot"];
            (supervisor.generation, supervisor.cancellation.child_token())
        };
        state
            .spawn_game_task(
                account,
                engine,
                "token".into(),
                "admitted-after-cancel".into(),
                game_cancellation,
                generation,
            )
            .await
            .unwrap();

        state.stop_bot_owned("bot", false, false).await.unwrap();
        stream_release.cancel();
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn finished_game_task_does_not_count_as_live_ownership() {
        let root = temp_root("finished-game-count");
        let state = AppState::new_with_secret_store(
            root.clone(),
            app_config("unused-engine", false),
            Arc::new(MemorySecretStore::with("bot", "token")),
        )
        .unwrap();
        state
            .install_finished_game_task_for_test("bot", "finished")
            .await;

        let quiesce = state.quiesce().await;
        assert_eq!(quiesce.live_game_count().await, 0);
        quiesce.shutdown().await.unwrap();
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn handover_inventory_counts_authoritative_ownership_unions() {
        let root = temp_root("handover-inventory");
        let state = AppState::new_with_secret_store(
            root.clone(),
            app_config("unused-engine", false),
            Arc::new(MemorySecretStore::with("bot", "token")),
        )
        .unwrap();
        state.reserve_game_for_test("bot", "reserved").await;
        state.add_active_intent("bot", "reserved").await.unwrap();
        state.add_active_intent("bot", "intent-only").await.unwrap();
        state
            .remember_known_outgoing_challenge("bot", "one", "First")
            .await;
        state
            .remember_known_outgoing_challenge("bot", "two", "Second")
            .await;
        state
            .remember_uncertain_challenge_creation("bot", "Second")
            .await
            .unwrap();
        let mut campaign = crate::models::CampaignRuntime::stopped("bot".into());
        campaign.pending_challenges = 1;
        state
            .0
            .campaign_runtimes
            .write()
            .await
            .insert("bot".into(), campaign);

        assert_eq!(state.live_game_ownership_count().await, 2);
        assert_eq!(state.outstanding_outgoing_challenge_count().await, 2);

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn quiesced_shutdown_releases_ownership_before_joining_tasks() {
        let root = temp_root("shutdown-lock-order");
        let state = AppState::new_with_secret_store(
            root.clone(),
            app_config("unused-engine", false),
            Arc::new(MemorySecretStore::with("bot", "token")),
        )
        .unwrap();
        let quiesce = state.quiesce().await;
        let admission = state.0.ownership_admission.clone();
        state.0.supervisors.lock().await.insert(
            "bot".into(),
            SupervisorTask {
                generation: 1,
                cancellation: CancellationToken::new(),
                handle: Some(tokio::spawn(async move {
                    let _queued_reservation = admission.read_owned().await;
                })),
            },
        );

        tokio::time::timeout(Duration::from_secs(1), quiesce.shutdown())
            .await
            .expect("shutdown held the ownership writer across its task join")
            .unwrap();
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn quiesce_reports_locally_tracked_and_uncertain_outgoing_challenges() {
        let root = temp_root("local-outgoing-refusal");
        let state = AppState::new_with_secret_store(
            root.clone(),
            app_config("unused-engine", false),
            Arc::new(MemorySecretStore::with("bot", "token")),
        )
        .unwrap();
        state
            .remember_known_outgoing_challenge("bot", "challenge", "Opponent")
            .await;
        let quiesce = state.quiesce().await;
        assert_eq!(
            quiesce.locally_known_outgoing_challenge_error().await,
            Some("An outgoing challenge to Opponent is still unresolved; cancel it or let it resolve before switching to a runner.".into())
        );
        drop(quiesce);
        state
            .forget_known_outgoing_challenge("bot", "challenge")
            .await;
        state
            .remember_uncertain_challenge_creation("bot", "OtherOpponent")
            .await
            .unwrap();
        let quiesce = state.quiesce().await;
        assert_eq!(
            quiesce.locally_known_outgoing_challenge_error().await,
            Some("An outgoing challenge creation for bot against OtherOpponent is still uncertain; let QueenUI reconcile it before switching to a runner.".into())
        );
        drop(quiesce);
        state
            .clear_uncertain_challenge_creation("bot")
            .await
            .unwrap();
        let mut campaign = crate::models::CampaignRuntime::stopped("bot".into());
        campaign.pending_challenges = 2;
        state
            .0
            .campaign_runtimes
            .write()
            .await
            .insert("bot".into(), campaign);
        let quiesce = state.quiesce().await;
        assert_eq!(
            quiesce.locally_known_outgoing_challenge_error().await,
            Some("2 campaign challenges are still unresolved; cancel them or let them resolve before switching to a runner.".into())
        );
        drop(quiesce);
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn authoritative_handover_refuses_remote_game_and_outgoing_challenge() {
        let outgoing_http = ScriptedHttp::start().await;
        outgoing_http.push(
            "GET",
            "/api/challenge",
            ScriptReply::Json(
                axum::http::StatusCode::OK,
                r#"{"out":[{"id":"challenge-1","destUser":{"id":"Opponent"},"status":"created"}]}"#
                    .into(),
            ),
        );
        outgoing_http.push(
            "GET",
            "/api/account/playing",
            ScriptReply::Json(axum::http::StatusCode::OK, r#"{"nowPlaying":[]}"#.into()),
        );
        let outgoing_root = temp_root("authoritative-outgoing");
        let outgoing_state = AppState::new_with_test_api(
            outgoing_root.clone(),
            app_config("unused-engine", true),
            Arc::new(MemorySecretStore::with("bot", "token")),
            outgoing_http.base(),
        )
        .unwrap();
        let quiesce = outgoing_state.quiesce().await;
        assert_eq!(
            quiesce.verify_authoritative_handover().await.unwrap_err(),
            "Lichess account Bot still has 1 outgoing challenge (Opponent); cancel them or let them resolve before switching to a runner."
        );
        assert_eq!(outgoing_http.count("GET", "/api/challenge"), 1);
        assert_eq!(outgoing_http.count("GET", "/api/account/playing"), 1);
        drop(quiesce);
        drop(outgoing_state);
        let _ = std::fs::remove_dir_all(outgoing_root);

        let game_http = ScriptedHttp::start().await;
        game_http.push(
            "GET",
            "/api/challenge",
            ScriptReply::Json(axum::http::StatusCode::OK, r#"{"out":[]}"#.into()),
        );
        game_http.push(
            "GET",
            "/api/account/playing",
            ScriptReply::Json(
                axum::http::StatusCode::OK,
                r#"{"nowPlaying":[{"gameId":"game-1"}]}"#.into(),
            ),
        );
        let game_root = temp_root("authoritative-game");
        let game_state = AppState::new_with_test_api(
            game_root.clone(),
            app_config("unused-engine", true),
            Arc::new(MemorySecretStore::with("bot", "token")),
            game_http.base(),
        )
        .unwrap();
        let quiesce = game_state.quiesce().await;
        assert_eq!(
            quiesce.verify_authoritative_handover().await.unwrap_err(),
            "Lichess account Bot still has 1 live game (game-1); finish or resign them before switching to a runner."
        );
        assert_eq!(game_http.count("GET", "/api/challenge"), 1);
        assert_eq!(game_http.count("GET", "/api/account/playing"), 1);
        drop(quiesce);
        drop(game_state);
        let _ = std::fs::remove_dir_all(game_root);
    }

    #[tokio::test]
    async fn authoritative_handover_fails_closed_when_lichess_cannot_be_verified() {
        let http = ScriptedHttp::start().await;
        http.push(
            "GET",
            "/api/challenge",
            ScriptReply::Json(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                r#"{"error":"offline"}"#.into(),
            ),
        );
        http.push(
            "GET",
            "/api/account/playing",
            ScriptReply::Json(axum::http::StatusCode::OK, r#"{"nowPlaying":[]}"#.into()),
        );
        let root = temp_root("authoritative-fail-closed");
        let state = AppState::new_with_test_api(
            root.clone(),
            app_config("unused-engine", true),
            Arc::new(MemorySecretStore::with("bot", "token")),
            http.base(),
        )
        .unwrap();
        let quiesce = state.quiesce().await;
        assert_eq!(
            quiesce.verify_authoritative_handover().await.unwrap_err(),
            "Could not verify Lichess account Bot before switching runners; live games or outgoing challenges may still exist."
        );
        drop(quiesce);
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn authoritative_handover_prunes_a_stale_known_challenge_and_proceeds() {
        let http = ScriptedHttp::start().await;
        http.push(
            "GET",
            "/api/challenge",
            ScriptReply::Json(axum::http::StatusCode::OK, r#"{"out":[]}"#.into()),
        );
        http.push(
            "GET",
            "/api/account/playing",
            ScriptReply::Json(axum::http::StatusCode::OK, r#"{"nowPlaying":[]}"#.into()),
        );
        let root = temp_root("authoritative-known-prune");
        let state = AppState::new_with_test_api(
            root.clone(),
            app_config("unused-engine", true),
            Arc::new(MemorySecretStore::with("bot", "token")),
            http.base(),
        )
        .unwrap();
        state
            .remember_known_outgoing_challenge("bot", "stale", "Opponent")
            .await;

        let quiesce = state.quiesce().await;
        quiesce.verify_authoritative_handover().await.unwrap();
        assert!(state.0.known_outgoing_challenges.lock().await.is_empty());
        quiesce.shutdown().await.unwrap();
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn authoritative_handover_rechecks_local_maps_after_straddled_reads() {
        let http = ScriptedHttp::start().await;
        for (path, body) in [
            ("/api/challenge", r#"{"out":[]}"#),
            ("/api/account/playing", r#"{"nowPlaying":[]}"#),
        ] {
            http.push(
                "GET",
                path,
                ScriptReply::Delay(
                    Duration::from_millis(50),
                    Box::new(ScriptReply::Json(axum::http::StatusCode::OK, body.into())),
                ),
            );
        }
        let root = temp_root("authoritative-straddle");
        let state = AppState::new_with_test_api(
            root.clone(),
            app_config("unused-engine", true),
            Arc::new(MemorySecretStore::with("bot", "token")),
            http.base(),
        )
        .unwrap();
        install_online_supervisor(&state, 1).await;
        state
            .remember_known_outgoing_challenge("bot", "straddled", "Opponent")
            .await;
        let config = state.0.config.read().await;
        let account = config.accounts[0].clone();
        let engine = config.engines[0].clone();
        drop(config);
        let quiesce = state.quiesce().await;

        let verification = quiesce.verify_authoritative_handover();
        let delivery = async {
            http.wait_for_count("GET", "/api/challenge", 1).await;
            http.wait_for_count("GET", "/api/account/playing", 1).await;
            handle_account_event(
                &state,
                &account,
                &engine,
                "token",
                &CancellationToken::new(),
                1,
                r#"{"type":"gameStart","game":{"gameId":"straddled","opponent":{"username":"Opponent"}}}"#,
            )
            .await
            .unwrap_err()
        };
        let (verification, _) = tokio::join!(verification, delivery);

        assert_eq!(
            verification.unwrap_err(),
            "An outgoing challenge to Opponent is still unresolved; cancel it or let it resolve before switching to a runner."
        );
        assert!(state
            .0
            .known_outgoing_challenges
            .lock()
            .await
            .contains_key(&("bot".into(), "straddled".into())));
        quiesce.shutdown().await.unwrap();
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn stop_start_fence_holds_old_supervisor_and_game_until_both_join() {
        let http = ScriptedHttp::start().await;
        http.push(
            "GET",
            "/api/account/playing",
            ScriptReply::Json(axum::http::StatusCode::OK, r#"{"nowPlaying":[]}"#.into()),
        );
        let stream_release = CancellationToken::new();
        http.push(
            "GET",
            "/api/stream/event",
            ScriptReply::NdjsonHold(String::new(), stream_release.clone()),
        );
        let root = temp_root("stop-start-fence");
        let secrets = Arc::new(MemorySecretStore::with("bot", "token"));
        let state = AppState::new_with_test_api(
            root.clone(),
            app_config("unused-engine", false),
            secrets,
            http.base(),
        )
        .unwrap();

        let release = Arc::new(Semaphore::new(0));
        state.0.supervisor_generation.store(7, Ordering::SeqCst);
        let supervisor_cancellation = CancellationToken::new();
        let supervisor_wait = release.clone();
        let supervisor_handle = tokio::spawn(async move {
            let _permit = supervisor_wait.acquire().await.unwrap();
        });
        state.0.supervisors.lock().await.insert(
            "bot".into(),
            SupervisorTask {
                generation: 7,
                cancellation: supervisor_cancellation.clone(),
                handle: Some(supervisor_handle),
            },
        );
        let game_cancellation = CancellationToken::new();
        let game_wait = release.clone();
        let submission_wait = release.clone();
        let submission_exited = Arc::new(AtomicBool::new(false));
        let submission_finished = submission_exited.clone();
        let game_handle = tokio::spawn(async move {
            let submission = tokio::spawn(async move {
                let _permit = submission_wait.acquire().await.unwrap();
                submission_finished.store(true, Ordering::SeqCst);
            });
            let _permit = game_wait.acquire().await.unwrap();
            submission.await.unwrap();
        });
        state.0.game_tasks.lock().await.insert(
            ("bot".into(), "old-game".into()),
            GameTask {
                generation: 7,
                cancellation: game_cancellation.clone(),
                handle: Some(game_handle),
            },
        );

        let stopping_state = state.clone();
        let stop =
            tokio::spawn(async move { stopping_state.stop_bot_owned("bot", false, false).await });
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while !supervisor_cancellation.is_cancelled() || !game_cancellation.is_cancelled() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        let starting_state = state.clone();
        let start = tokio::spawn(async move { starting_state.start_bot("bot").await });
        tokio::task::yield_now().await;
        assert!(
            !start.is_finished(),
            "Start crossed the in-progress Stop fence"
        );
        assert_eq!(
            state.0.supervisors.lock().await["bot"].generation,
            7,
            "the old supervisor reservation was released before its join"
        );
        assert!(state
            .0
            .game_tasks
            .lock()
            .await
            .contains_key(&("bot".into(), "old-game".into())));
        assert!(
            !submission_exited.load(Ordering::SeqCst),
            "the old game submission owner exited before its release"
        );

        release.add_permits(3);
        stop.await.unwrap().unwrap();
        assert!(submission_exited.load(Ordering::SeqCst));
        start.await.unwrap().unwrap();
        let new_generation = state.0.supervisors.lock().await["bot"].generation;
        assert!(new_generation > 7);
        assert!(!state
            .0
            .game_tasks
            .lock()
            .await
            .contains_key(&("bot".into(), "old-game".into())));
        state.stop_bot_owned("bot", false, false).await.unwrap();
        stream_release.cancel();
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn supervisor_and_game_panics_are_joined_into_durable_error_state() {
        let root = temp_root("task-panics");
        let state = AppState::new_with_secret_store(
            root.clone(),
            app_config("unused-engine", false),
            Arc::new(MemorySecretStore::with("bot", "token")),
        )
        .unwrap();
        state.0.supervisors.lock().await.insert(
            "bot".into(),
            SupervisorTask {
                generation: 1,
                cancellation: CancellationToken::new(),
                handle: Some(tokio::spawn(async { panic!("supervisor exploded") })),
            },
        );
        state.0.game_tasks.lock().await.insert(
            ("bot".into(), "game".into()),
            GameTask {
                generation: 1,
                cancellation: CancellationToken::new(),
                handle: Some(tokio::spawn(async { panic!("game exploded") })),
            },
        );

        let error = state.stop_bot_owned("bot", false, false).await.unwrap_err();
        assert!(
            error.contains("account supervisor failed while joining"),
            "{error}"
        );
        assert!(error.contains("game task failed while joining"), "{error}");
        let runtime = state.0.runtimes.read().await["bot"].clone();
        assert_eq!(runtime.status, "error");
        assert_eq!(runtime.error.as_deref(), Some(error.as_str()));
        assert!(!state.0.supervisors.lock().await.contains_key("bot"));
        assert!(state.0.game_tasks.lock().await.is_empty());
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn supervisor_wrapper_surfaces_current_panics_without_a_stop_call() {
        let root = temp_root("supervisor-wrapper-panic");
        let state = AppState::new_with_secret_store(
            root.clone(),
            app_config("unused-engine", false),
            Arc::new(MemorySecretStore::with("bot", "token")),
        )
        .unwrap();
        state.0.supervisors.lock().await.insert(
            "bot".into(),
            SupervisorTask {
                generation: 7,
                cancellation: CancellationToken::new(),
                handle: None,
            },
        );

        spawn_supervisor_wrapper(state.clone(), "bot".into(), 7, async {
            panic!("current supervisor wrapper panic");
        })
        .await
        .unwrap();
        let runtime = state.0.runtimes.read().await["bot"].clone();
        assert_eq!(runtime.status, "error");
        assert!(runtime
            .error
            .as_deref()
            .is_some_and(|detail| detail.contains("panicked: current supervisor wrapper panic")));
        assert!(state
            .0
            .diagnostics
            .recent(&crate::diagnostics::DiagnosticFilter {
                account_id: Some("bot".into()),
                query: Some("current supervisor wrapper panic".into()),
                ..Default::default()
            })
            .iter()
            .any(|entry| entry.message == "Account supervisor panicked"));

        state.set_runtime("bot", "online", None).await;
        state.0.supervisors.lock().await.insert(
            "bot".into(),
            SupervisorTask {
                generation: 8,
                cancellation: CancellationToken::new(),
                handle: None,
            },
        );
        spawn_supervisor_wrapper(state.clone(), "bot".into(), 7, async {
            panic!("stale supervisor wrapper panic");
        })
        .await
        .unwrap();
        let runtime = state.0.runtimes.read().await["bot"].clone();
        assert_eq!(runtime.status, "online");
        assert!(runtime.error.is_none());
        assert!(state
            .0
            .diagnostics
            .recent(&crate::diagnostics::DiagnosticFilter {
                query: Some("stale supervisor wrapper panic".into()),
                ..Default::default()
            })
            .is_empty());

        state.0.supervisors.lock().await.remove("bot");
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn game_wrapper_surfaces_current_panics_without_a_stop_call() {
        let root = temp_root("game-wrapper-panic");
        let state = AppState::new_with_secret_store(
            root.clone(),
            app_config("unused-engine", false),
            Arc::new(MemorySecretStore::with("bot", "token")),
        )
        .unwrap();
        state.0.supervisors.lock().await.insert(
            "bot".into(),
            SupervisorTask {
                generation: 11,
                cancellation: CancellationToken::new(),
                handle: None,
            },
        );
        let game_key = ("bot".to_string(), "wrapper-game".to_string());
        state.0.games.write().await.insert(
            game_key.clone(),
            crate::models::LiveGame {
                id: "wrapper-game".into(),
                account_id: "bot".into(),
                bot_username: "Bot".into(),
                opponent: "Opponent".into(),
                bot_rating: None,
                opponent_rating: None,
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
            },
        );
        state.0.active_games.lock().await.insert(game_key.clone());
        let account = state.0.config.read().await.accounts[0].clone();

        spawn_game_wrapper(
            state.clone(),
            account,
            game_key.clone(),
            CancellationToken::new(),
            11,
            async {
                panic!("current game wrapper panic");
                #[allow(unreachable_code)]
                Ok::<(), String>(())
            },
        )
        .await
        .unwrap();
        let game = state.0.games.read().await[&game_key].clone();
        assert_eq!(game.status, "error");
        assert!(game.error.as_deref().is_some_and(
            |detail| detail.contains("Game task panicked: current game wrapper panic")
        ));
        assert!(state
            .0
            .diagnostics
            .recent(&crate::diagnostics::DiagnosticFilter {
                account_id: Some("bot".into()),
                query: Some("current game wrapper panic".into()),
                ..Default::default()
            })
            .iter()
            .any(|entry| entry.message == "A supervised game task failed"));

        state.set_runtime("bot", "online", None).await;
        state.0.supervisors.lock().await.insert(
            "bot".into(),
            SupervisorTask {
                generation: 12,
                cancellation: CancellationToken::new(),
                handle: None,
            },
        );
        let account = state.0.config.read().await.accounts[0].clone();
        let stale_key = ("bot".to_string(), "stale-game".to_string());
        state.0.active_games.lock().await.insert(stale_key.clone());
        spawn_game_wrapper(
            state.clone(),
            account,
            stale_key.clone(),
            CancellationToken::new(),
            11,
            async { Err("stale game failure".into()) },
        )
        .await
        .unwrap();
        let runtime = state.0.runtimes.read().await["bot"].clone();
        assert_eq!(runtime.status, "online");
        assert!(runtime.error.is_none());

        assert!(state.0.active_games.lock().await.contains(&stale_key));
        state.stop_bot_owned("bot", false, false).await.unwrap();
        assert!(!state.0.active_games.lock().await.contains(&stale_key));
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(start_paused = true)]
    async fn join_timeout_aborts_task_and_surfaces_durable_error() {
        let root = temp_root("task-timeout");
        let state = AppState::new_with_secret_store(
            root.clone(),
            app_config("unused-engine", false),
            Arc::new(MemorySecretStore::with("bot", "token")),
        )
        .unwrap();
        state.0.supervisors.lock().await.insert(
            "bot".into(),
            SupervisorTask {
                generation: 1,
                cancellation: CancellationToken::new(),
                handle: Some(tokio::spawn(std::future::pending())),
            },
        );
        let stopping_state = state.clone();
        let stop =
            tokio::spawn(async move { stopping_state.stop_bot_owned("bot", false, false).await });
        tokio::task::yield_now().await;
        tokio::time::advance(TASK_JOIN_TIMEOUT + std::time::Duration::from_secs(1)).await;
        let error = stop.await.unwrap().unwrap_err();
        assert!(error.contains("did not exit within 10 seconds"), "{error}");
        let runtime = state.0.runtimes.read().await["bot"].clone();
        assert_eq!(runtime.status, "error");
        assert_eq!(runtime.error.as_deref(), Some(error.as_str()));
        assert!(!state.0.supervisors.lock().await.contains_key("bot"));
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn ambiguous_challenge_post_blocks_every_later_post_until_authoritative_reconciliation() {
        let http = ScriptedHttp::start().await;
        http.push(
            "POST",
            "/api/challenge/Opponent",
            ScriptReply::BodyError(axum::http::StatusCode::OK),
        );
        http.push(
            "GET",
            "/api/challenge",
            ScriptReply::Json(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                r#"{"error":"temporarily unavailable"}"#.into(),
            ),
        );
        http.push(
            "GET",
            "/api/challenge",
            ScriptReply::Json(
                axum::http::StatusCode::OK,
                r#"{"out":[{"id":"known-id","status":"created","destUser":{"id":"Opponent"}}]}"#
                    .into(),
            ),
        );
        let root = temp_root("ambiguous-challenge");
        let state = AppState::new_with_test_api(
            root.clone(),
            app_config("unused-engine", false),
            Arc::new(MemorySecretStore::with("bot", "token")),
            http.base(),
        )
        .unwrap();
        let supervisor_cancellation = CancellationToken::new();
        let task_cancellation = supervisor_cancellation.clone();
        state.0.supervisors.lock().await.insert(
            "bot".into(),
            SupervisorTask {
                generation: 1,
                cancellation: supervisor_cancellation,
                handle: Some(tokio::spawn(async move {
                    task_cancellation.cancelled().await;
                })),
            },
        );
        state.set_runtime("bot", "online", None).await;
        let request = ChallengeRequest {
            account_id: "bot".into(),
            opponent: "Opponent".into(),
            clock_limit: 180,
            clock_increment: 2,
            rated: false,
            color: "random".into(),
            variant: "standard".into(),
        };

        let first = create_challenge(request.clone(), CoreStateRef::new(&state))
            .await
            .unwrap_err();
        assert!(first.contains("outcome is unknown"), "{first}");
        assert_eq!(http.count("POST", "/api/challenge/Opponent"), 1);

        let second = create_challenge(request.clone(), CoreStateRef::new(&state))
            .await
            .unwrap_err();
        assert!(second.contains("remains paused"), "{second}");
        assert_eq!(
            http.count("POST", "/api/challenge/Opponent"),
            1,
            "a failed authoritative read must not permit another POST"
        );
        assert_eq!(
            http.requests()
                .iter()
                .filter(|request| request.method == "POST")
                .count(),
            1,
            "the account-wide ambiguity barrier must block every later POST"
        );

        let reconciled = create_challenge(request, CoreStateRef::new(&state))
            .await
            .unwrap();
        assert_eq!(reconciled.id, "known-id");
        assert_eq!(http.count("POST", "/api/challenge/Opponent"), 1);
        assert!(state
            .0
            .uncertain_challenge_creations
            .lock()
            .await
            .is_empty());
        state.stop_bot_owned("bot", false, false).await.unwrap();
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn manual_challenge_reconciliation_surfaces_missing_scope_remedy() {
        let http = ScriptedHttp::start().await;
        http.push(
            "GET",
            "/api/challenge",
            ScriptReply::Json(
                axum::http::StatusCode::FORBIDDEN,
                r#"{"error":"Missing scope: challenge:read"}"#.into(),
            ),
        );
        let root = temp_root("challenge-missing-scope");
        let state = AppState::new_with_test_api(
            root.clone(),
            app_config("unused-engine", false),
            Arc::new(MemorySecretStore::with("bot", "token")),
            http.base(),
        )
        .unwrap();
        install_online_supervisor(&state, 1).await;
        state
            .remember_uncertain_challenge_creation("bot", "Opponent")
            .await
            .unwrap();

        let error = create_challenge(standard_challenge_request(), CoreStateRef::new(&state))
            .await
            .unwrap_err();

        assert_eq!(
            error,
            "Matchmaking is paused because this Lichess token is missing scope challenge:read; create a new token at lichess.org/account/oauth/token/create with Play-bot, Read-challenges, and Send-challenges ticked—games continue with the current token."
        );
        assert_eq!(http.count("POST", "/api/challenge/Opponent"), 0);
        state.stop_bot_owned("bot", false, false).await.unwrap();
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn challenge_barrier_is_committed_to_disk_before_create_post() {
        let http = ScriptedHttp::start().await;
        http.push(
            "POST",
            "/api/challenge/Opponent",
            ScriptReply::Delay(
                Duration::from_secs(1),
                Box::new(ScriptReply::Json(
                    axum::http::StatusCode::OK,
                    r#"{"challenge":{"id":"write-ahead","status":"created"}}"#.into(),
                )),
            ),
        );
        let root = temp_root("challenge-write-ahead");
        let state = AppState::new_with_test_api(
            root.clone(),
            app_config("unused-engine", false),
            Arc::new(MemorySecretStore::with("bot", "token")),
            http.base(),
        )
        .unwrap();
        install_online_supervisor(&state, 1).await;

        let task_state = state.clone();
        let create = tokio::spawn(async move {
            create_challenge(standard_challenge_request(), CoreStateRef::new(&task_state)).await
        });
        http.wait_for_count("POST", "/api/challenge/Opponent", 1)
            .await;

        let creations = storage::load_uncertain_challenge_creations(
            &storage::uncertain_challenge_creations_path(&root),
        )
        .unwrap();
        assert_eq!(creations.len(), 1);
        assert_eq!(creations[0].account_id, "bot");
        assert_eq!(creations[0].opponent, "Opponent");

        assert_eq!(create.await.unwrap().unwrap().id, "write-ahead");
        state.stop_bot_owned("bot", false, false).await.unwrap();
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn challenge_resolution_before_post_completion_does_not_leave_a_known_phantom() {
        let http = ScriptedHttp::start().await;
        http.push(
            "POST",
            "/api/challenge/Opponent",
            ScriptReply::Delay(
                Duration::from_millis(50),
                Box::new(ScriptReply::Json(
                    axum::http::StatusCode::OK,
                    r#"{"challenge":{"id":"resolved-before-response","status":"created"}}"#.into(),
                )),
            ),
        );
        let root = temp_root("challenge-resolution-race");
        let state = AppState::new_with_test_api(
            root.clone(),
            app_config("unused-engine", false),
            Arc::new(MemorySecretStore::with("bot", "token")),
            http.base(),
        )
        .unwrap();
        install_online_supervisor(&state, 1).await;

        let task_state = state.clone();
        let create = tokio::spawn(async move {
            create_challenge(standard_challenge_request(), CoreStateRef::new(&task_state)).await
        });
        http.wait_for_count("POST", "/api/challenge/Opponent", 1)
            .await;
        let config = state.0.config.read().await;
        let account = config.accounts[0].clone();
        let engine = config.engines[0].clone();
        drop(config);
        handle_account_event(
            &state,
            &account,
            &engine,
            "token",
            &CancellationToken::new(),
            1,
            r#"{"type":"challengeDeclined","challenge":{"id":"resolved-before-response","destUser":{"id":"Opponent"}}}"#,
        )
        .await
        .unwrap();

        assert_eq!(
            create.await.unwrap().unwrap().id,
            "resolved-before-response"
        );
        assert!(state.0.known_outgoing_challenges.lock().await.is_empty());
        state.stop_bot_owned("bot", false, false).await.unwrap();
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn challenge_barrier_persist_failure_blocks_create_post() {
        let http = ScriptedHttp::start().await;
        let root = temp_root("challenge-write-ahead-failure");
        let state = AppState::new_with_test_api(
            root.clone(),
            app_config("unused-engine", false),
            Arc::new(MemorySecretStore::with("bot", "token")),
            http.base(),
        )
        .unwrap();
        install_online_supervisor(&state, 1).await;
        std::fs::create_dir(storage::uncertain_challenge_creations_path(&root)).unwrap();

        let error = create_challenge(standard_challenge_request(), CoreStateRef::new(&state))
            .await
            .unwrap_err();
        assert!(error.contains("barrier"), "{error}");
        assert_eq!(http.count("POST", "/api/challenge/Opponent"), 0);
        assert!(state
            .0
            .uncertain_challenge_creations
            .lock()
            .await
            .contains_key("bot"));

        state.stop_bot_owned("bot", false, false).await.unwrap();
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn definitive_challenge_decline_clears_barrier_without_forcing_reconciliation() {
        let http = ScriptedHttp::start().await;
        http.push(
            "POST",
            "/api/challenge/Opponent",
            ScriptReply::Json(
                axum::http::StatusCode::BAD_REQUEST,
                r#"{"error":"declined"}"#.into(),
            ),
        );
        http.push(
            "POST",
            "/api/challenge/Opponent",
            ScriptReply::Json(
                axum::http::StatusCode::OK,
                r#"{"challenge":{"id":"second-create","status":"created"}}"#.into(),
            ),
        );
        let root = temp_root("challenge-definitive-clear");
        let state = AppState::new_with_test_api(
            root.clone(),
            app_config("unused-engine", false),
            Arc::new(MemorySecretStore::with("bot", "token")),
            http.base(),
        )
        .unwrap();
        install_online_supervisor(&state, 1).await;
        let request = standard_challenge_request();

        let error = create_challenge(request.clone(), CoreStateRef::new(&state))
            .await
            .unwrap_err();
        assert!(error.contains("HTTP 400"), "{error}");
        assert!(storage::load_uncertain_challenge_creations(
            &storage::uncertain_challenge_creations_path(&root)
        )
        .unwrap()
        .is_empty());

        let challenge = create_challenge(request, CoreStateRef::new(&state))
            .await
            .unwrap();
        assert_eq!(challenge.id, "second-create");
        assert_eq!(http.count("POST", "/api/challenge/Opponent"), 2);
        assert_eq!(http.count("GET", "/api/challenge"), 0);

        state.stop_bot_owned("bot", false, false).await.unwrap();
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn idless_challenge_success_is_ambiguous_and_reconciles_before_any_retry() {
        let http = ScriptedHttp::start().await;
        http.push(
            "POST",
            "/api/challenge/Opponent",
            ScriptReply::Json(
                axum::http::StatusCode::OK,
                r#"{"challenge":{"status":"created"}}"#.into(),
            ),
        );
        http.push(
            "GET",
            "/api/challenge",
            ScriptReply::Json(
                axum::http::StatusCode::OK,
                r#"{"out":[{"id":"reconciled-id","status":"created","destUser":{"id":"Opponent"}}]}"#.into(),
            ),
        );
        let root = temp_root("idless-challenge");
        let state = AppState::new_with_test_api(
            root.clone(),
            app_config("unused-engine", false),
            Arc::new(MemorySecretStore::with("bot", "token")),
            http.base(),
        )
        .unwrap();
        let cancellation = CancellationToken::new();
        let task_cancellation = cancellation.clone();
        state.0.supervisors.lock().await.insert(
            "bot".into(),
            SupervisorTask {
                generation: 1,
                cancellation,
                handle: Some(tokio::spawn(async move {
                    task_cancellation.cancelled().await;
                })),
            },
        );
        state.set_runtime("bot", "online", None).await;
        let request = ChallengeRequest {
            account_id: "bot".into(),
            opponent: "Opponent".into(),
            clock_limit: 180,
            clock_increment: 2,
            rated: false,
            color: "random".into(),
            variant: "standard".into(),
        };

        let error = create_challenge(request.clone(), CoreStateRef::new(&state))
            .await
            .unwrap_err();
        assert!(error.contains("outcome is unknown"), "{error}");
        let reconciled = create_challenge(request, CoreStateRef::new(&state))
            .await
            .unwrap();
        assert_eq!(reconciled.id, "reconciled-id");
        assert_eq!(http.count("POST", "/api/challenge/Opponent"), 1);
        assert_eq!(
            http.requests()
                .iter()
                .map(|request| (request.method.as_str(), request.path.as_str()))
                .collect::<Vec<_>>(),
            [
                ("POST", "/api/challenge/Opponent"),
                ("GET", "/api/challenge")
            ]
        );

        state.stop_bot_owned("bot", false, false).await.unwrap();
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn uncertain_challenge_creation_survives_app_state_restart() {
        let http = ScriptedHttp::start().await;
        http.push(
            "POST",
            "/api/challenge/Opponent",
            ScriptReply::BodyError(axum::http::StatusCode::OK),
        );
        http.push(
            "GET",
            "/api/challenge",
            ScriptReply::Json(
                axum::http::StatusCode::OK,
                r#"{"out":[{"id":"survived-restart","status":"created","destUser":{"id":"Opponent"}}]}"#.into(),
            ),
        );
        let root = temp_root("challenge-restart");
        let config = app_config("unused-engine", false);
        let secrets = Arc::new(MemorySecretStore::with("bot", "token"));
        let state =
            AppState::new_with_test_api(root.clone(), config.clone(), secrets.clone(), http.base())
                .unwrap();
        let cancellation = CancellationToken::new();
        let task_cancellation = cancellation.clone();
        state.0.supervisors.lock().await.insert(
            "bot".into(),
            SupervisorTask {
                generation: 1,
                cancellation,
                handle: Some(tokio::spawn(async move {
                    task_cancellation.cancelled().await;
                })),
            },
        );
        state.set_runtime("bot", "online", None).await;
        let request = ChallengeRequest {
            account_id: "bot".into(),
            opponent: "Opponent".into(),
            clock_limit: 180,
            clock_increment: 2,
            rated: false,
            color: "random".into(),
            variant: "standard".into(),
        };

        let error = create_challenge(request.clone(), CoreStateRef::new(&state))
            .await
            .unwrap_err();
        assert!(error.contains("outcome is unknown"), "{error}");
        state.stop_bot_owned("bot", false, false).await.unwrap();
        drop(state);

        let restarted =
            AppState::new_with_test_api(root.clone(), config, secrets, http.base()).unwrap();
        let cancellation = CancellationToken::new();
        let task_cancellation = cancellation.clone();
        restarted.0.supervisors.lock().await.insert(
            "bot".into(),
            SupervisorTask {
                generation: 2,
                cancellation,
                handle: Some(tokio::spawn(async move {
                    task_cancellation.cancelled().await;
                })),
            },
        );
        restarted.set_runtime("bot", "online", None).await;
        let reconciled = create_challenge(request, CoreStateRef::new(&restarted))
            .await
            .unwrap();
        assert_eq!(reconciled.id, "survived-restart");
        assert_eq!(http.count("POST", "/api/challenge/Opponent"), 1);
        let requests = http.requests();
        assert_eq!(requests[0].method, "POST");
        assert_eq!(requests[1].method, "GET");

        restarted.stop_bot_owned("bot", false, false).await.unwrap();
        drop(restarted);
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    fn fake_engine(
        root: &std::path::Path,
    ) -> (
        std::path::PathBuf,
        std::path::PathBuf,
        std::path::PathBuf,
        std::path::PathBuf,
    ) {
        use std::os::unix::fs::PermissionsExt;
        let path = root.join("fake-uci.sh");
        let pid = root.join("engine.pid");
        let quit = root.join("engine.quit");
        let commands = root.join("engine.commands");
        let source = format!(
            r#"#!/bin/sh
printf '%s' "$$" > '{}'
while IFS= read -r command; do
  printf '%s\n' "$command" >> '{}'
  case "$command" in
    uci) printf '%s\n' 'id name Shutdown fake UCI' 'uciok' ;;
    isready) printf '%s\n' 'readyok' ;;
    go*) printf '%s\n' 'bestmove e2e4' ;;
    stop) printf '%s\n' 'bestmove e2e4' ;;
    quit) printf '%s' 'quit' > '{}'; exit 0 ;;
  esac
done
"#,
            pid.display(),
            commands.display(),
            quit.display()
        );
        std::fs::create_dir_all(root).unwrap();
        std::fs::write(&path, source).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        (path, pid, quit, commands)
    }

    #[cfg(unix)]
    fn fake_failing_engine(root: &std::path::Path) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = root.join("fake-failing-uci.sh");
        let source = r#"#!/bin/sh
while IFS= read -r command; do
  case "$command" in
    uci) printf '%s\n' 'id name Failing fake UCI' 'uciok' ;;
    isready) printf '%s\n' 'readyok' ;;
    go*) exit 1 ;;
    quit) exit 0 ;;
  esac
done
"#;
        std::fs::create_dir_all(root).unwrap();
        std::fs::write(&path, source).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn auto_resign_cancels_and_joins_the_game_coordinator_before_resign_post() {
        let http = ScriptedHttp::start().await;
        http.push(
            "POST",
            "/api/bot/game/game/move/e2e4",
            ScriptReply::Delay(
                std::time::Duration::from_secs(60),
                Box::new(ScriptReply::Json(axum::http::StatusCode::OK, "{}".into())),
            ),
        );
        http.push(
            "GET",
            "/api/account/playing",
            ScriptReply::Json(
                axum::http::StatusCode::OK,
                r#"{"nowPlaying":[{"gameId":"game"}]}"#.into(),
            ),
        );
        http.push(
            "POST",
            "/api/bot/game/game/resign",
            ScriptReply::Json(axum::http::StatusCode::OK, "{}".into()),
        );
        let root = temp_root("resign-submission-owner");
        let engine_path = fake_failing_engine(&root);
        let config = app_config(engine_path.to_str().unwrap(), false);
        let state = AppState::new_with_test_api(
            root.clone(),
            config.clone(),
            Arc::new(MemorySecretStore::with("bot", "token")),
            http.base(),
        )
        .unwrap();
        let mut engine = crate::uci::UciEngine::start(engine_path.to_str().unwrap(), &[], None)
            .await
            .unwrap();
        let transport: Arc<dyn MoveTransport> = Arc::new(super::LichessMoveTransport {
            base: http.base(),
            client: state.0.api_client.clone(),
            token: "token".into(),
        });
        let cancellation = CancellationToken::new();
        let mut submissions = SubmissionCoordinator::start(
            state.clone(),
            ("bot".into(), "game".into()),
            transport,
            Arc::new(std::sync::atomic::AtomicI64::new(0)),
            &cancellation,
        );
        submissions
            .submit(String::new(), "e2e4".into())
            .await
            .unwrap();
        http.wait_for_count("POST", "/api/bot/game/game/move/e2e4", 1)
            .await;

        let mut context = GameContext::default();
        let completed = process_game_event(
            &state,
            &config.accounts[0],
            "game",
            "token",
            &mut engine,
            &config.engines[0],
            None,
            &mut submissions,
            &mut context,
            serde_json::json!({
                "type": "gameFull",
                "initialFen": "startpos",
                "white": { "id": "bot", "name": "Bot" },
                "black": { "id": "opponent", "name": "Opponent" },
                "state": {
                    "moves": "",
                    "status": "started",
                    "wtime": 60_000,
                    "btime": 60_000,
                    "winc": 0,
                    "binc": 0
                }
            }),
        )
        .await
        .unwrap();
        assert!(!completed);
        assert!(submissions.handle.is_none(), "coordinator was not joined");
        assert_eq!(http.count("POST", "/api/bot/game/game/resign"), 1);
        let requests = http.requests();
        let move_index = requests
            .iter()
            .position(|request| request.path.ends_with("/move/e2e4"))
            .unwrap();
        let resign_index = requests
            .iter()
            .position(|request| request.path.ends_with("/resign"))
            .unwrap();
        assert!(move_index < resign_index);
        let calls_after_resign = requests.len();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert_eq!(http.requests().len(), calls_after_resign);
        engine.shutdown().await;
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shutdown_drains_campaign_game_submitter_and_engine_then_restart_reconciles_first() {
        let http = ScriptedHttp::start().await;
        let first_game_stream = CancellationToken::new();
        http.push(
            "GET",
            "/api/bot/game/stream/game-1",
            ScriptReply::NdjsonHold(
                r#"{"type":"gameFull","initialFen":"startpos","rated":false,"speed":"blitz","white":{"id":"bot","name":"Bot"},"black":{"id":"opponent","name":"Opponent"},"clock":{"initial":60000,"increment":0},"state":{"moves":"","status":"started","wtime":60000,"btime":60000,"winc":0,"binc":0}}"#.into(),
                first_game_stream.clone(),
            ),
        );
        http.push(
            "POST",
            "/api/bot/game/game-1/move/e2e4",
            ScriptReply::Delay(
                std::time::Duration::from_secs(60),
                Box::new(ScriptReply::Json(axum::http::StatusCode::OK, "{}".into())),
            ),
        );
        let root = temp_root("full-shutdown");
        let (engine_path, pid_path, quit_path, _commands_path) = fake_engine(&root);
        let config = app_config(engine_path.to_str().unwrap(), true);
        crate::storage::save(&crate::storage::config_path(&root), &config).unwrap();
        let secrets = Arc::new(MemorySecretStore::with("bot", "token"));
        let state =
            AppState::new_with_test_api(root.clone(), config.clone(), secrets.clone(), http.base())
                .unwrap();

        let supervisor_cancellation = CancellationToken::new();
        let supervisor_stopped = Arc::new(AtomicBool::new(false));
        let stopped = supervisor_stopped.clone();
        let task_cancellation = supervisor_cancellation.clone();
        state.0.supervisors.lock().await.insert(
            "bot".into(),
            SupervisorTask {
                generation: 1,
                cancellation: supervisor_cancellation.clone(),
                handle: Some(tokio::spawn(async move {
                    task_cancellation.cancelled().await;
                    stopped.store(true, Ordering::SeqCst);
                })),
            },
        );
        let campaign_cancellation = CancellationToken::new();
        let campaign_stopped = Arc::new(AtomicBool::new(false));
        let stopped = campaign_stopped.clone();
        let task_cancellation = campaign_cancellation.clone();
        state.0.campaign_tasks.lock().await.insert(
            "bot".into(),
            crate::campaign::CampaignTask {
                generation: 1,
                cancellation: campaign_cancellation,
                handle: Some(tokio::spawn(async move {
                    task_cancellation.cancelled().await;
                    stopped.store(true, Ordering::SeqCst);
                    Ok(())
                })),
            },
        );
        state
            .spawn_game_task(
                config.accounts[0].clone(),
                config.engines[0].clone(),
                "token".into(),
                "game-1".into(),
                supervisor_cancellation.child_token(),
                1,
            )
            .await
            .unwrap();
        http.wait_for_count("POST", "/api/bot/game/game-1/move/e2e4", 1)
            .await;
        assert!(pid_path.exists(), "the fake UCI child never started");

        let shutdown_started = std::time::Instant::now();
        state.shutdown().await.unwrap();
        assert!(shutdown_started.elapsed() < std::time::Duration::from_secs(12));
        assert!(campaign_stopped.load(Ordering::SeqCst));
        assert!(supervisor_stopped.load(Ordering::SeqCst));
        assert!(state.0.campaign_tasks.lock().await.is_empty());
        assert!(state.0.supervisors.lock().await.is_empty());
        assert!(state.0.game_tasks.lock().await.is_empty());
        let move_calls_after_join = http.count("POST", "/api/bot/game/game-1/move/e2e4");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert_eq!(
            http.count("POST", "/api/bot/game/game-1/move/e2e4"),
            move_calls_after_join,
            "the joined game coordinator issued HTTP after shutdown returned"
        );
        assert_eq!(std::fs::read_to_string(&quit_path).unwrap(), "quit");
        let pid: i32 = std::fs::read_to_string(&pid_path).unwrap().parse().unwrap();
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "engine process survived shutdown"
        );
        let intents = crate::storage::load_active_game_intents(
            &crate::storage::active_game_intents_path(&root),
        )
        .unwrap();
        assert!(intents.iter().any(|intent| intent.game_id == "game-1"));
        first_game_stream.cancel();
        drop(state);

        http.push(
            "GET",
            "/api/account/playing",
            ScriptReply::Json(
                axum::http::StatusCode::OK,
                r#"{"nowPlaying":[{"gameId":"game-1"}]}"#.into(),
            ),
        );
        let account_stream = CancellationToken::new();
        http.push(
            "GET",
            "/api/stream/event",
            ScriptReply::NdjsonHold(String::new(), account_stream.clone()),
        );
        let resumed_game_stream = CancellationToken::new();
        http.push(
            "GET",
            "/api/bot/game/stream/game-1",
            ScriptReply::NdjsonHold(
                r#"{"type":"gameState","moves":"e2e4","status":"started","wtime":59000,"btime":60000,"winc":0,"binc":0}"#.into(),
                resumed_game_stream.clone(),
            ),
        );
        let restarted = AppState::load_with_test_api(root.clone(), secrets, http.base()).unwrap();
        restarted.resume_enabled_accounts().await;
        http.wait_for_count("GET", "/api/stream/event", 1).await;
        let requests = http.requests();
        let playing = requests
            .iter()
            .rposition(|request| request.path == "/api/account/playing")
            .unwrap();
        let automation = requests
            .iter()
            .rposition(|request| {
                request.path == "/api/stream/event" || request.path == "/api/bot/game/stream/game-1"
            })
            .unwrap();
        assert!(
            playing < automation,
            "automation resumed before nowPlaying reconciliation"
        );
        restarted.shutdown().await.unwrap();
        account_stream.cancel();
        resumed_game_stream.cancel();
        drop(restarted);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn failed_restart_reconciliation_leaves_automation_stopped_and_visible() {
        let http = ScriptedHttp::start().await;
        http.push(
            "GET",
            "/api/account/playing",
            ScriptReply::Json(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                r#"{"error":"nowPlaying unavailable"}"#.into(),
            ),
        );
        let root = temp_root("failed-reconciliation");
        let config = app_config("unused-engine", true);
        crate::storage::save(&crate::storage::config_path(&root), &config).unwrap();
        crate::storage::save_active_game_intents(
            &crate::storage::active_game_intents_path(&root),
            &[crate::storage::ActiveGameIntent {
                account_id: "bot".into(),
                game_id: "game-1".into(),
            }],
        )
        .unwrap();
        let state = AppState::load_with_test_api(
            root.clone(),
            Arc::new(MemorySecretStore::with("bot", "token")),
            http.base(),
        )
        .unwrap();
        state.resume_enabled_accounts().await;
        let runtime = state.0.runtimes.read().await["bot"].clone();
        assert_eq!(runtime.status, "error");
        assert!(runtime
            .error
            .unwrap()
            .contains("Could not reconcile ongoing games before start"));
        assert!(state.0.supervisors.lock().await.is_empty());
        assert_eq!(http.count("GET", "/api/stream/event"), 0);
        assert_eq!(http.count("GET", "/api/bot/game/stream/game-1"), 0);
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn disabled_account_reconciles_persisted_intent_without_resuming_automation() {
        let http = ScriptedHttp::start().await;
        http.push(
            "GET",
            "/api/account/playing",
            ScriptReply::Json(
                axum::http::StatusCode::OK,
                r#"{"nowPlaying":[{"gameId":"game-1"}]}"#.into(),
            ),
        );
        let root = temp_root("disabled-reconciliation");
        let config = app_config("unused-engine", false);
        crate::storage::save(&crate::storage::config_path(&root), &config).unwrap();
        crate::storage::save_active_game_intents(
            &crate::storage::active_game_intents_path(&root),
            &[crate::storage::ActiveGameIntent {
                account_id: "bot".into(),
                game_id: "game-1".into(),
            }],
        )
        .unwrap();
        let state = AppState::load_with_test_api(
            root.clone(),
            Arc::new(MemorySecretStore::with("bot", "token")),
            http.base(),
        )
        .unwrap();
        state.resume_enabled_accounts().await;
        let runtime = state.0.runtimes.read().await["bot"].clone();
        assert_eq!(runtime.status, "error");
        assert!(runtime
            .error
            .unwrap()
            .contains("Automation remains disabled"));
        assert!(state.0.supervisors.lock().await.is_empty());
        assert_eq!(http.count("GET", "/api/stream/event"), 0);
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn local_account_removal_deletes_secret_and_failure_keeps_account_and_token() {
        let root = temp_root("remove-account");
        let secrets = Arc::new(MemorySecretStore::with("bot", "token"));
        let state = AppState::new_with_secret_store(
            root.clone(),
            app_config("unused-engine", false),
            secrets.clone(),
        )
        .unwrap();
        state
            .remember_known_outgoing_challenge("bot", "known", "Opponent")
            .await;
        state
            .remember_uncertain_challenge_creation("bot", "Opponent")
            .await
            .unwrap();
        remove_lichess_account("bot".into(), CoreStateRef::new(&state))
            .await
            .unwrap();
        assert!(!secrets.contains("bot"));
        assert!(state.0.config.read().await.accounts.is_empty());
        assert!(state.0.known_outgoing_challenges.lock().await.is_empty());
        assert!(state
            .0
            .uncertain_challenge_creations
            .lock()
            .await
            .is_empty());
        assert!(storage::load_uncertain_challenge_creations(
            &storage::uncertain_challenge_creations_path(&root)
        )
        .unwrap()
        .is_empty());
        drop(state);

        let failed_root = temp_root("remove-account-failure");
        let failed_secrets = Arc::new(MemorySecretStore::with("bot", "token"));
        failed_secrets.fail_deletes_with("credential store is unavailable");
        let failed_state = AppState::new_with_secret_store(
            failed_root.clone(),
            app_config("unused-engine", false),
            failed_secrets.clone(),
        )
        .unwrap();
        let error = remove_lichess_account("bot".into(), CoreStateRef::new(&failed_state))
            .await
            .unwrap_err();
        assert!(error.contains("credential store is unavailable"));
        assert!(failed_secrets.contains("bot"));
        assert_eq!(failed_state.0.config.read().await.accounts.len(), 1);
        drop(failed_state);
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(failed_root);
    }

    #[cfg(not(windows))]
    struct CountingMoveTransport(std::sync::atomic::AtomicUsize);

    #[cfg(not(windows))]
    impl MoveTransport for CountingMoveTransport {
        fn submit<'a>(
            &'a self,
            _game_id: &'a str,
            _chess_move: &'a str,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<(), crate::lichess::LichessError>>
                    + Send
                    + 'a,
            >,
        > {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bad_live_events_are_rejected_before_any_uci_write_or_move_submission() {
        let root = temp_root("bad-live-events");
        let (engine_path, _pid_path, _quit_path, command_path) = fake_engine(&root);
        let config = app_config(engine_path.to_str().unwrap(), false);
        let state = AppState::new_with_secret_store(
            root.clone(),
            config.clone(),
            Arc::new(MemorySecretStore::with("bot", "token")),
        )
        .unwrap();
        let mut engine = crate::uci::UciEngine::start(engine_path.to_str().unwrap(), &[], None)
            .await
            .unwrap();
        let transport = Arc::new(CountingMoveTransport(std::sync::atomic::AtomicUsize::new(
            0,
        )));
        let cancellation = CancellationToken::new();
        let mut submissions = SubmissionCoordinator::start(
            state.clone(),
            ("bot".into(), "bad-game".into()),
            transport.clone(),
            Arc::new(std::sync::atomic::AtomicI64::new(0)),
            &cancellation,
        );
        let baseline = std::fs::read_to_string(&command_path).unwrap();
        let mut black_to_move = GameContext::default();
        process_game_event(
            &state,
            &config.accounts[0],
            "bad-game",
            "token",
            &mut engine,
            &config.engines[0],
            None,
            &mut submissions,
            &mut black_to_move,
            serde_json::json!({
                "type": "gameFull",
                "initialFen": "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR b KQkq - 0 1",
                "white": { "id": "bot", "name": "Bot" },
                "black": { "id": "opponent", "name": "Opponent" },
                "state": {
                    "moves": "",
                    "status": "started",
                    "wtime": 60_000,
                    "btime": 60_000,
                    "winc": 0,
                    "binc": 0
                }
            }),
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read_to_string(&command_path).unwrap(), baseline);
        assert_eq!(transport.0.load(Ordering::SeqCst), 0);

        let overlong = std::iter::repeat_n(
            ["g1f3", "g8f6", "f3g1", "f6g8"],
            crate::position::MAX_LIVE_PLIES / 4 + 1,
        )
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
        let cases = [
            ("startpos\rquit".to_string(), String::new()),
            ("startpos\nquit".to_string(), String::new()),
            ("not a FEN".to_string(), String::new()),
            (
                " ".repeat(crate::position::MAX_LIVE_FEN_BYTES + 1),
                String::new(),
            ),
            (
                "startpos".to_string(),
                " ".repeat(crate::position::MAX_LIVE_MOVES_BYTES + 1),
            ),
            ("startpos".to_string(), "e2e4\rquit".to_string()),
            ("startpos".to_string(), "e2e4\nquit".to_string()),
            ("startpos".to_string(), "e2e5".to_string()),
            ("startpos".to_string(), overlong),
        ];
        for (fen, moves) in cases {
            let oversized_fen = fen.len() > crate::position::MAX_LIVE_FEN_BYTES;
            let oversized_moves = moves.len() > crate::position::MAX_LIVE_MOVES_BYTES;
            let event = serde_json::json!({
                "type": "gameFull",
                "initialFen": fen,
                "white": { "id": "bot", "name": "Bot" },
                "black": { "id": "opponent", "name": "Opponent" },
                "state": {
                    "moves": moves,
                    "status": "started",
                    "wtime": 60_000,
                    "btime": 60_000,
                    "winc": 0,
                    "binc": 0
                }
            });
            let mut context = GameContext::default();
            let error = process_game_event(
                &state,
                &config.accounts[0],
                "bad-game",
                "token",
                &mut engine,
                &config.engines[0],
                None,
                &mut submissions,
                &mut context,
                event,
            )
            .await
            .unwrap_err();
            assert!(!error.is_empty());
            if oversized_fen {
                assert!(
                    error.contains(&format!(
                        "exceeds the {}-byte safety limit",
                        crate::position::MAX_LIVE_FEN_BYTES
                    )),
                    "{error}"
                );
            }
            if oversized_moves {
                assert!(
                    error.contains(&format!(
                        "exceeds the {}-byte safety limit",
                        crate::position::MAX_LIVE_MOVES_BYTES
                    )),
                    "{error}"
                );
            }
            assert_eq!(std::fs::read_to_string(&command_path).unwrap(), baseline);
            assert_eq!(transport.0.load(Ordering::SeqCst), 0);
        }
        submissions.shutdown().await.unwrap();
        engine.shutdown().await;
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }
}

/// Keep at most this many finished games in memory; every snapshot serializes
/// the whole map, so finished games must not accumulate unbounded.
const MAX_FINISHED_GAMES: usize = 50;
/// Failed games remain operator-visible until dismissed. This separate bound
/// prevents an unattended runner from growing its snapshot without limit.
const MAX_RETAINED_ERROR_GAMES: usize = 32;

/// A game Lichess still considers in progress. Matches `isLiveGame` in
/// `src/lib/chess.ts`; both sides must agree on what "still playing" means.
fn is_live(game: &LiveGame) -> bool {
    game.status == "started" || game.status == "created"
}

fn prune_finished_games(games: &mut HashMap<GameKey, LiveGame>) {
    let mut errors: Vec<(u64, GameKey)> = games
        .iter()
        .filter(|(_, game)| game.status == "error")
        .map(|(key, game)| (game.clock_updated_at, key.clone()))
        .collect();
    if errors.len() > MAX_RETAINED_ERROR_GAMES {
        errors.sort_unstable();
        let excess = errors.len() - MAX_RETAINED_ERROR_GAMES;
        for (_, key) in errors.into_iter().take(excess) {
            games.remove(&key);
            diagnostics::record(
                DiagnosticEntry::warn(
                    "app",
                    "An old retained game error was dropped at the 32-game safety limit",
                )
                .with_account(&key.0)
                .with_game(&key.1),
            );
        }
    }

    let mut finished: Vec<(u64, GameKey)> = games
        .iter()
        .filter(|(_, game)| !is_live(game) && game.status != "error")
        .map(|(key, game)| (game.clock_updated_at, key.clone()))
        .collect();
    if finished.len() <= MAX_FINISHED_GAMES {
        return;
    }
    finished.sort_unstable();
    let excess = finished.len() - MAX_FINISHED_GAMES;
    for (_, key) in finished.into_iter().take(excess) {
        games.remove(&key);
    }
}

fn same_engine_executable(existing: &str, candidate: &str) -> bool {
    if let (Ok(existing_path), Ok(candidate_path)) = (
        std::path::Path::new(existing).canonicalize(),
        std::path::Path::new(candidate).canonicalize(),
    ) {
        return existing_path == candidate_path;
    }
    if cfg!(windows) {
        existing.eq_ignore_ascii_case(candidate)
    } else {
        existing == candidate
    }
}

#[cfg(test)]
mod state_maintenance_tests {
    use super::{
        dismiss_game_error, is_live, prune_finished_games, AppState, CoreStateRef, GameKey,
        MAX_FINISHED_GAMES, MAX_RETAINED_ERROR_GAMES,
    };
    use crate::{
        models::{AppSnapshot, CampaignSettings, LiveGame},
        storage,
        test_support::{app_config, temp_root, MemorySecretStore},
    };
    use std::collections::HashMap;
    use std::sync::Arc;

    fn game(account_id: &str, game_id: &str, status: &str, clock_updated_at: u64) -> LiveGame {
        LiveGame {
            id: game_id.to_string(),
            account_id: account_id.to_string(),
            bot_username: "bot".into(),
            opponent: "opponent".into(),
            bot_rating: None,
            opponent_rating: None,
            color: "white".into(),
            initial_fen: "startpos".into(),
            moves: String::new(),
            status: status.to_string(),
            white_time: 0,
            black_time: 0,
            white_increment: 0,
            black_increment: 0,
            clock_updated_at,
            result: None,
            engine_line: None,
            engine_info: None,
            engine_thinking: false,
            error: None,
        }
    }

    #[test]
    fn prunes_oldest_finished_games_and_keeps_running_ones() {
        let mut games: HashMap<GameKey, LiveGame> = HashMap::new();
        for index in 0..(MAX_FINISHED_GAMES + 10) {
            let id = format!("finished-{index}");
            games.insert(
                ("account".into(), id.clone()),
                game("account", &id, "mate", index as u64),
            );
        }
        for index in 0..5 {
            let id = format!("running-{index}");
            games.insert(
                ("account".into(), id.clone()),
                game("account", &id, "started", 0),
            );
        }
        prune_finished_games(&mut games);
        assert_eq!(games.len(), MAX_FINISHED_GAMES + 5);
        // The oldest finished games were evicted, the newest kept.
        for index in 0..10 {
            assert!(!games.contains_key(&("account".into(), format!("finished-{index}"))));
        }
        assert!(games.contains_key(&("account".into(), format!("finished-{}", 10))));
        // Running games always survive, even with the oldest timestamps.
        for index in 0..5 {
            assert!(games.contains_key(&("account".into(), format!("running-{index}"))));
        }
    }

    #[test]
    fn pruning_is_a_no_op_below_the_limit() {
        let mut games: HashMap<GameKey, LiveGame> = HashMap::new();
        games.insert(
            ("account".into(), "one".into()),
            game("account", "one", "resign", 1),
        );
        prune_finished_games(&mut games);
        assert_eq!(games.len(), 1);
    }

    #[test]
    fn errored_game_survives_finished_pruning_and_snapshot_round_trip() {
        let mut games: HashMap<GameKey, LiveGame> = HashMap::new();
        games.insert(
            ("account".into(), "error".into()),
            game("account", "error", "error", 0),
        );
        for index in 0..(MAX_FINISHED_GAMES + 10) {
            let id = format!("finished-{index}");
            games.insert(
                ("account".into(), id.clone()),
                game("account", &id, "mate", index as u64),
            );
        }

        prune_finished_games(&mut games);

        let retained = games
            .get(&("account".into(), "error".into()))
            .cloned()
            .expect("errored game retained");
        let snapshot = AppSnapshot {
            games: vec![retained],
            ..AppSnapshot::default()
        };
        let decoded: AppSnapshot =
            serde_json::from_value(serde_json::to_value(snapshot).unwrap()).unwrap();
        assert_eq!(decoded.games.len(), 1);
        assert_eq!(decoded.games[0].id, "error");
        assert_eq!(decoded.games[0].status, "error");
    }

    #[tokio::test]
    async fn dismiss_game_error_removes_the_retained_game() {
        let root = temp_root("dismiss-game-error");
        let state = AppState::new_with_secret_store(
            root.clone(),
            app_config("unused-engine", false),
            Arc::new(MemorySecretStore::default()),
        )
        .unwrap();
        state.0.games.write().await.insert(
            ("bot".into(), "failed-game".into()),
            game("bot", "failed-game", "error", 1),
        );

        dismiss_game_error("failed-game".into(), CoreStateRef::new(&state))
            .await
            .unwrap();

        assert!(state.snapshot().await.games.is_empty());
        assert_eq!(
            dismiss_game_error("failed-game".into(), CoreStateRef::new(&state))
                .await
                .unwrap_err(),
            "No retained game error was found for failed-game."
        );
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn campaign_settings_survive_state_reconstruction() {
        let root = temp_root("campaign-settings-restart");
        let mut config = app_config("unused-engine", false);
        config.campaigns.push(CampaignSettings {
            account_id: "bot".into(),
            min_rating: 1800,
            max_rating: 2400,
            concurrency: 4,
            clock_limit: 300,
            clock_increment: 3,
            rated: true,
            color: "black".into(),
        });
        storage::save(&storage::config_path(&root), &config).unwrap();

        let first =
            AppState::load_with_secret_store(root.clone(), Arc::new(MemorySecretStore::default()))
                .unwrap();
        let expected = serde_json::to_value(&first.snapshot().await.campaigns).unwrap();
        drop(first);

        let reconstructed =
            AppState::load_with_secret_store(root.clone(), Arc::new(MemorySecretStore::default()))
                .unwrap();
        assert_eq!(
            serde_json::to_value(&reconstructed.snapshot().await.campaigns).unwrap(),
            expected
        );
        assert!(reconstructed.snapshot().await.campaigns[0].rated);
        drop(reconstructed);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn retained_game_error_cap_drops_the_oldest_errors() {
        let mut games: HashMap<GameKey, LiveGame> = HashMap::new();
        for index in 0..(MAX_RETAINED_ERROR_GAMES + 10) {
            let id = format!("error-{index}");
            games.insert(
                ("account".into(), id.clone()),
                game("account", &id, "error", index as u64),
            );
        }

        prune_finished_games(&mut games);

        assert_eq!(games.len(), MAX_RETAINED_ERROR_GAMES);
        for index in 0..10 {
            assert!(!games.contains_key(&("account".into(), format!("error-{index}"))));
        }
        assert!(games.contains_key(&("account".into(), "error-10".into())));
    }

    /// The close guard counts these, so the definition has to match
    /// `isLiveGame` in `src/lib/chess.ts` exactly.
    #[test]
    fn only_started_and_created_games_count_as_live() {
        for status in ["started", "created"] {
            assert!(is_live(&game("account", "id", status, 0)), "{status}");
        }
        for status in [
            "mate",
            "resign",
            "stalemate",
            "timeout",
            "draw",
            "outoftime",
            "aborted",
            "noStart",
            "unknownFinish",
        ] {
            assert!(!is_live(&game("account", "id", status, 0)), "{status}");
        }
    }
}
