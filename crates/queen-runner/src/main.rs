mod availability;
mod engine_admin;
mod identity;
mod persistence;

use axum::{
    body::Bytes,
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path as RoutePath, State,
    },
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use persistence::{
    response_fits, CompletionState, IdempotencyBinding, RedeemError, Reservation, RunnerDatabase,
};
use queen_core::{storage::FileSecretStore, AppState, CoreEvent, CoreStateRef};
use queen_protocol::{
    command_body_digest, CommandRequest, CommandResponse, EngineBrowseRequest,
    EngineBrowseResponse, EngineRoot, EventEnvelope, HandoverInventory, HealthResponse,
    OpeningBookAsset, PairRedeemRequest, PairRedeemResponse, PendingResponse, RunnerCapabilities,
    RunnerCommand, SnapshotResponse, CAMPAIGN_COMPLETED_GAME_LIMIT_FEATURE,
    CAMPAIGN_SCHEDULING_FEATURE, CONTENT_SHA256_HEADER, IDEMPOTENCY_PENDING_WAIT_SECONDS,
    OPENING_BOOK_ASSETS_FEATURE, PAIRING_PAYLOAD_VERSION, PROTOCOL_VERSION,
};
use serde_json::{json, Value};
use std::{
    env,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
use tokio::sync::{broadcast, OwnedSemaphorePermit, Semaphore};
use tokio::{task::JoinHandle, time::Duration};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Strictly below the shipped systemd `TimeoutStopSec=15`. This bounds the
/// complete post-signal sequence, including HTTP drain, automation/engine
/// joins, and the core-event forwarder.
const RUNNER_SHUTDOWN_BUDGET: Duration = Duration::from_secs(13);
const QUERY_DEADLINE: Duration = Duration::from_secs(5);
const LOG_EXPORT_MAX_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone)]
struct ServerState {
    core: AppState,
    engine_admin: Arc<engine_admin::EngineAdmin>,
    database: RunnerDatabase,
    instance_id: Uuid,
    sequence: Arc<AtomicU64>,
    events: broadcast::Sender<EventEnvelope>,
    normal_admission: Arc<Semaphore>,
    query_admission: Arc<Semaphore>,
    _upload_admission: Arc<Semaphore>,
    blocking_admission: Arc<Semaphore>,
    lifecycle: availability::LifecycleActors,
    forward_cancellation: CancellationToken,
    forward_handle: Arc<std::sync::Mutex<Option<JoinHandle<()>>>>,
}

impl ServerState {
    fn new(
        core: AppState,
        database: RunnerDatabase,
        engine_admin: engine_admin::EngineAdmin,
    ) -> Self {
        let instance_id = Uuid::new_v4();
        let sequence = Arc::new(AtomicU64::new(0));
        let (events, _) = broadcast::channel(128);
        let forward_cancellation = CancellationToken::new();
        let normal_commands = engine_admin.limits.normal_commands;
        let query_concurrency = engine_admin.limits.query_concurrency;
        let lifecycle = availability::LifecycleActors::new(core.clone());
        let blocking_admission = engine_admin.blocking_admission();
        let state = Self {
            core,
            engine_admin: Arc::new(engine_admin),
            database,
            instance_id,
            sequence,
            events,
            normal_admission: Arc::new(Semaphore::new(normal_commands)),
            query_admission: Arc::new(Semaphore::new(query_concurrency)),
            _upload_admission: Arc::new(Semaphore::new(1)),
            blocking_admission,
            lifecycle,
            forward_cancellation,
            forward_handle: Arc::new(std::sync::Mutex::new(None)),
        };
        state.forward_core_events();
        state
    }

    fn forward_core_events(&self) {
        let mut source = self.core.subscribe();
        let target = self.events.clone();
        let core = self.core.clone();
        let instance_id = self.instance_id;
        let sequence = self.sequence.clone();
        let cancellation = self.forward_cancellation.clone();
        let handle = tokio::spawn(async move {
            loop {
                let event = tokio::select! {
                    _ = cancellation.cancelled() => return,
                    event = source.recv() => event,
                };
                match event {
                    Ok(event) => {
                        let next = sequence.fetch_add(1, Ordering::Relaxed) + 1;
                        let _ = target.send(EventEnvelope {
                            protocol_version: PROTOCOL_VERSION,
                            instance_id,
                            sequence: next,
                            event,
                        });
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        let next = sequence.fetch_add(1, Ordering::Relaxed) + 1;
                        let _ = target.send(EventEnvelope {
                            protocol_version: PROTOCOL_VERSION,
                            instance_id,
                            sequence: next,
                            event: CoreEvent::Snapshot(core.snapshot().await),
                        });
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        });
        *self
            .forward_handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(handle);
    }

    async fn shutdown_forwarder(&self) {
        self.forward_cancellation.cancel();
        let handle = self
            .forward_handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(mut handle) = handle {
            if tokio::time::timeout(Duration::from_secs(2), &mut handle)
                .await
                .is_err()
            {
                handle.abort();
                let _ = handle.await;
            }
        }
    }

    fn authorized(&self, headers: &HeaderMap) -> Result<Option<u64>, String> {
        let Some(value) = headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
        else {
            return Ok(None);
        };
        self.database.authenticate(value)
    }
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("queen-runner: {}", redact_for_log(&error));
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let data_dir = runner_data_dir()?;
    let tls_identity = identity::ensure(&data_dir)?;
    let database = RunnerDatabase::open(data_dir.clone(), tls_identity.fingerprint)?;
    let arguments: Vec<String> = env::args().skip(1).collect();
    if arguments.first().is_some_and(|argument| argument == "pair") {
        return pair_command(
            &arguments[1..],
            &data_dir,
            &database,
            &tls_identity.fingerprint,
        );
    }
    if !arguments.is_empty() {
        return Err("Usage: queen-runner [pair [--rotate] [--print]]".into());
    }
    let _ = runner_public_url(&data_dir)?;
    let listen: SocketAddr = env::var("QUEEN_RUNNER_LISTEN")
        .unwrap_or_else(|_| "127.0.0.1:7788".into())
        .parse()
        .map_err(|error| format!("Invalid QUEEN_RUNNER_LISTEN: {error}"))?;
    let engine_admin = engine_admin::EngineAdmin::load(&data_dir)?;
    let secrets = Arc::new(FileSecretStore::new(data_dir.join("secrets")));
    let core = AppState::load_with_secret_store(data_dir, secrets)?;
    engine_admin.validate_registered_engines(&core).await?;
    engine_admin.garbage_collect(&core).await;
    core.configure_engine_limits(queen_core::uci::EngineLimits {
        simultaneous_engines: engine_admin.limits.simultaneous_engines,
        total_memory_mb: engine_admin.limits.total_engine_memory_mb,
        total_cpu_threads: engine_admin.limits.total_engine_cpu_threads,
        total_tasks: engine_admin.limits.total_engine_tasks,
        output_bytes_per_engine_per_second: engine_admin.limits.engine_output_bytes_per_second,
        total_output_bytes_per_second: engine_admin.limits.total_engine_output_bytes_per_second,
        output_bytes_per_engine: engine_admin.limits.engine_output_total_bytes,
    })?;
    core.configure_blocking_workers(engine_admin.limits.blocking_workers)?;
    core.enforce_engine_log_byte_ceiling(engine_admin.limits.engine_log_bytes)
        .await?;
    core.resume_enabled_accounts().await;
    database.mark_interrupted_pending()?;
    database.reconcile_ambiguous()?;
    let state = ServerState::new(core, database, engine_admin);
    let app = router(state.clone());
    let tls_config = axum_server::tls_rustls::RustlsConfig::from_der(
        vec![tls_identity.certificate_der],
        tls_identity.private_key_der,
    )
    .await
    .map_err(|error| format!("Could not configure runner TLS: {error}"))?;
    eprintln!(
        "queen-runner {} protocol={} listening={} cpus={}",
        state.instance_id,
        PROTOCOL_VERSION,
        listen,
        std::thread::available_parallelism().map_or(1, usize::from)
    );
    let server_handle = axum_server::Handle::new();
    let server = axum_server::tls_rustls::bind_rustls(listen, tls_config)
        .handle(server_handle.clone())
        .serve(app.into_make_service());
    tokio::pin!(server);
    let early_server_result = tokio::select! {
        result = &mut server => Some(result),
        _ = shutdown_signal() => {
            server_handle.graceful_shutdown(Some(RUNNER_SHUTDOWN_BUDGET));
            state.forward_cancellation.cancel();
            None
        }
    };
    tokio::time::timeout(RUNNER_SHUTDOWN_BUDGET, async {
        let core_shutdown = state.core.shutdown();
        let (server_result, core_result) = if let Some(server_result) = early_server_result {
            server_handle.graceful_shutdown(Some(RUNNER_SHUTDOWN_BUDGET));
            (server_result, core_shutdown.await)
        } else {
            tokio::join!(&mut server, core_shutdown)
        };
        let server_result =
            server_result.map_err(|error| format!("Runner server stopped unexpectedly: {error}"));
        state.shutdown_forwarder().await;
        match (server_result, core_result) {
            (Err(server), Err(core)) => Err(format!("{server}; {core}")),
            (Err(error), _) | (_, Err(error)) => Err(error),
            (Ok(()), Ok(())) => Ok(()),
        }
    })
    .await
    .map_err(|_| {
        format!(
            "Runner graceful shutdown exceeded {} seconds",
            RUNNER_SHUTDOWN_BUDGET.as_secs()
        )
    })?
}

fn runner_data_dir() -> Result<PathBuf, String> {
    let data_dir = env::var_os("QUEEN_RUNNER_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .map(|path| path.join("queenui-runner"))
        })
        .or_else(|| {
            env::var_os("HOME")
                .map(PathBuf::from)
                .map(|path| path.join(".local/share/queenui-runner"))
        })
        .ok_or_else(|| "Set QUEEN_RUNNER_DATA_DIR to an absolute runner data path".to_string())?;
    if !data_dir.is_absolute() {
        return Err("QUEEN_RUNNER_DATA_DIR must be absolute".into());
    }
    Ok(data_dir)
}

fn pair_command(
    arguments: &[String],
    data_dir: &Path,
    database: &RunnerDatabase,
    certificate_fingerprint: &[u8; 32],
) -> Result<(), String> {
    let mut rotate = false;
    let mut print = false;
    for argument in arguments {
        match argument.as_str() {
            "--rotate" if !rotate => rotate = true,
            "--print" if !print => print = true,
            _ => return Err("Usage: queen-runner pair [--rotate] [--print]".into()),
        }
    }
    let public_url = runner_public_url(data_dir)?;
    let enrollment = database.mint_enrollment(rotate, epoch_seconds())?;
    let mut payload = url::Url::parse("queenui://pair")
        .map_err(|_| "Could not construct the pairing payload".to_string())?;
    payload
        .query_pairs_mut()
        .append_pair("v", &PAIRING_PAYLOAD_VERSION.to_string())
        .append_pair("url", &public_url)
        .append_pair("fp", &identity::fingerprint_hex(certificate_fingerprint))
        .append_pair("enroll", &enrollment.code);
    if print {
        println!("{payload}");
    } else {
        println!("QueenUI one-time pairing payload (expires in 10 minutes):");
        println!("{payload}");
        if rotate {
            println!("Redeeming this payload immediately revokes the current bearer.");
        }
    }
    Ok(())
}

fn runner_public_url(data_dir: &Path) -> Result<String, String> {
    const PUBLIC_URL_FILE: &str = "runner-public-url";
    if let Ok(configured) = env::var("QUEEN_RUNNER_PUBLIC_URL") {
        let canonical = canonical_public_url(&configured)?;
        let path = data_dir.join(PUBLIC_URL_FILE);
        let temporary = data_dir.join(format!(".{PUBLIC_URL_FILE}.tmp"));
        std::fs::write(&temporary, format!("{canonical}\n"))
            .map_err(|error| format!("Could not persist the runner public URL: {error}"))?;
        std::fs::rename(&temporary, path)
            .map_err(|error| format!("Could not replace the runner public URL: {error}"))?;
        return Ok(canonical);
    }
    let configured = std::fs::read_to_string(data_dir.join(PUBLIC_URL_FILE)).map_err(|_| {
        "Set QUEEN_RUNNER_PUBLIC_URL once so pairing can publish the runner's HTTPS URL".to_string()
    })?;
    canonical_public_url(configured.trim())
}

fn canonical_public_url(value: &str) -> Result<String, String> {
    let mut url = url::Url::parse(value.trim())
        .map_err(|_| "QUEEN_RUNNER_PUBLIC_URL is invalid".to_string())?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "QUEEN_RUNNER_PUBLIC_URL must be HTTPS and contain only host, port, and base path"
                .into(),
        );
    }
    if url.port() == Some(443) {
        url.set_port(None)
            .map_err(|_| "Could not normalize QUEEN_RUNNER_PUBLIC_URL".to_string())?;
    }
    let path = url.path().trim_end_matches('/').to_string();
    url.set_path(if path.is_empty() { "/" } else { &path });
    Ok(url.as_str().trim_end_matches('/').to_string())
}

fn epoch_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn router(state: ServerState) -> Router {
    Router::new()
        .route("/v2/health", get(health))
        .route("/v2/pair/redeem", post(redeem_pairing))
        .route("/v2/capabilities", get(capabilities))
        .route("/v2/snapshot", get(snapshot))
        .route("/v2/commands", post(command))
        .route("/v2/engines/roots", get(engine_roots))
        .route("/v2/engines/browse", post(browse_engines))
        .route("/v2/opening-books", get(opening_books))
        .route("/v2/engines/upload", post(disabled_engine_install))
        .route("/v2/engines/register-path", post(disabled_engine_install))
        .route("/v2/events", get(events))
        .route("/v2/logs/{session_id}/export/{mode}", get(log_export))
        .with_state(state)
}

async fn health(State(state): State<ServerState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        service: "queen-runner".into(),
        protocol_version: PROTOCOL_VERSION,
        instance_id: state.instance_id,
    })
}

async fn redeem_pairing(
    State(state): State<ServerState>,
    Json(request): Json<PairRedeemRequest>,
) -> Result<Json<PairRedeemResponse>, ApiResponse> {
    let redeemed = state
        .database
        .redeem(&request.enroll, epoch_seconds())
        .map_err(|error| match error {
            RedeemError::Expired => ApiResponse {
                status: StatusCode::GONE,
                code: "enrollment_expired",
                message: "The enrollment code expired; mint a new code over the admin channel",
            },
            RedeemError::Rejected { .. }
            | RedeemError::Revoked
            | RedeemError::Unavailable
            | RedeemError::IdentityMismatch => ApiResponse {
                status: StatusCode::UNAUTHORIZED,
                code: "enrollment_rejected",
                message: "The enrollment code was rejected; mint a new code if necessary",
            },
        })?;
    Ok(Json(PairRedeemResponse {
        protocol_version: PROTOCOL_VERSION,
        runner_id: state.database.runner_id(),
        bearer: redeemed.bearer,
        generation: redeemed.generation,
    }))
}

async fn capabilities(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<RunnerCapabilities>, ApiResponse> {
    require_auth(&state, &headers)?;
    Ok(Json(RunnerCapabilities {
        protocol_version: PROTOCOL_VERSION,
        instance_id: state.instance_id,
        hostname: hostname(),
        operating_system: env::consts::OS.into(),
        architecture: env::consts::ARCH.into(),
        logical_cpus: std::thread::available_parallelism().map_or(1, usize::from),
        features: vec![
            CAMPAIGN_SCHEDULING_FEATURE.into(),
            CAMPAIGN_COMPLETED_GAME_LIMIT_FEATURE.into(),
            OPENING_BOOK_ASSETS_FEATURE.into(),
        ],
    }))
}

async fn snapshot(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<SnapshotResponse>, ApiResponse> {
    require_auth(&state, &headers)?;
    Ok(Json(SnapshotResponse {
        protocol_version: PROTOCOL_VERSION,
        instance_id: state.instance_id,
        snapshot: state.core.snapshot().await,
    }))
}

async fn engine_roots(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<Vec<EngineRoot>>, ApiResponse> {
    require_auth(&state, &headers)?;
    Ok(Json(state.engine_admin.roots()))
}

async fn opening_books(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<Vec<OpeningBookAsset>>, ApiResponse> {
    require_auth(&state, &headers)?;
    state
        .engine_admin
        .opening_books()
        .await
        .map(Json)
        .map_err(engine_admin_failure)
}

async fn browse_engines(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(request): Json<EngineBrowseRequest>,
) -> Result<Json<EngineBrowseResponse>, ApiResponse> {
    require_auth(&state, &headers)?;
    state
        .engine_admin
        .browse(request)
        .await
        .map(Json)
        .map_err(engine_admin_failure)
}

/// Trusted-engine mode has deliberately no request-body extractor here. The
/// refusal is produced by dispatch without polling an upload or legacy
/// arbitrary-path registration body.
async fn disabled_engine_install() -> ApiResponse {
    ApiResponse {
        status: StatusCode::NOT_FOUND,
        code: "engine_install_disabled",
        message:
            "Remote upload and arbitrary-path registration are unavailable in trusted-engine mode",
    }
}

async fn command(
    State(state): State<ServerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiResponse> {
    let generation = require_auth(&state, &headers)?;
    let declared_digest = parse_digest_header(&headers)?;
    let actual_digest = command_body_digest(&body);
    if declared_digest != actual_digest {
        return Err(ApiResponse {
            status: StatusCode::BAD_REQUEST,
            code: "body_digest_mismatch",
            message: "The command body does not match its declared digest",
        });
    }
    let request: CommandRequest = serde_json::from_slice(&body).map_err(|_| ApiResponse {
        status: StatusCode::BAD_REQUEST,
        code: "invalid_command_body",
        message: "The command body is not a valid protocol v2 command",
    })?;
    validate_runner_command(&request.command)?;
    let request_id = request.request_id;
    let (command_kind, reconciliation) = command_spec(&request.command);
    let binding = IdempotencyBinding {
        key: request_id,
        credential_generation: generation,
        protocol_version: PROTOCOL_VERSION,
        method: "POST",
        normalized_path: "/v2/commands",
        body_digest: actual_digest,
        command_kind,
        reconciliation,
    };
    match await_reservation(&state.database, &binding).await? {
        Reservation::Replay(response) => return Ok(Json(response).into_response()),
        Reservation::Pending => return Ok(pending_response(request_id)),
        Reservation::Conflict => return Err(idempotency_conflict()),
        Reservation::Execute => {}
    }
    let _admission = match acquire_command_admission(&state, &request.command).await {
        Ok(permit) => permit,
        Err(error) => {
            release_failed_admission(&state, &binding, request_id)?;
            return Err(error);
        }
    };
    let _blocking = match acquire_blocking_admission(&state, &request.command) {
        Ok(permit) => permit,
        Err(error) => {
            release_failed_admission(&state, &binding, request_id)?;
            return Err(error);
        }
    };
    let execution = if is_query_command(&request.command) {
        tokio::time::timeout(QUERY_DEADLINE, execute(&state, request.command))
            .await
            .unwrap_or_else(|_| Err("The bounded runner query deadline was exceeded".into()))
    } else {
        execute(&state, request.command).await
    };
    let mut response = match execution.as_ref() {
        Ok(result) => CommandResponse::success(request_id, result.clone()),
        Err(error) => CommandResponse::failure(request_id, "command_failed", error.clone()),
    };
    let mut completion = match execution {
        Ok(_) => CompletionState::Done,
        Err(error) if transient_failure(&error) => CompletionState::FailedTransient,
        Err(_) => CompletionState::FailedDeterministic,
    };
    if !response_fits(&response) {
        response = CommandResponse::failure(
            request_id,
            "response_too_large",
            "The command outcome exceeded the durable replay limit",
        );
        completion = CompletionState::FailedDeterministic;
    }
    state
        .database
        .complete(&binding, completion, &response)
        .map_err(persistence_failure)?;
    Ok(Json(response).into_response())
}

async fn await_reservation(
    database: &RunnerDatabase,
    binding: &IdempotencyBinding,
) -> Result<Reservation, ApiResponse> {
    await_reservation_for(
        database,
        binding,
        Duration::from_secs(IDEMPOTENCY_PENDING_WAIT_SECONDS),
    )
    .await
}

async fn await_reservation_for(
    database: &RunnerDatabase,
    binding: &IdempotencyBinding,
    wait: Duration,
) -> Result<Reservation, ApiResponse> {
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        let reservation = database.reserve(binding).map_err(persistence_failure)?;
        if !matches!(reservation, Reservation::Pending) {
            return Ok(reservation);
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(Reservation::Pending);
        }
        let poll_at = (tokio::time::Instant::now() + Duration::from_millis(50)).min(deadline);
        tokio::time::sleep_until(poll_at).await;
        if tokio::time::Instant::now() >= deadline {
            return Ok(Reservation::Pending);
        }
    }
}

fn pending_response(request_id: Uuid) -> Response {
    (
        StatusCode::ACCEPTED,
        Json(PendingResponse {
            protocol_version: PROTOCOL_VERSION,
            request_id,
            status: "pending".into(),
        }),
    )
        .into_response()
}

fn idempotency_conflict() -> ApiResponse {
    ApiResponse {
        status: StatusCode::CONFLICT,
        code: "idempotency_key_conflict",
        message: "This idempotency key is already bound to a different request",
    }
}

fn persistence_failure(error: String) -> ApiResponse {
    if error.starts_with("rate:") {
        ApiResponse {
            status: StatusCode::TOO_MANY_REQUESTS,
            code: "idempotency_rate_limited",
            message: "Too many new idempotency keys; retry later",
        }
    } else if error.starts_with("quota:") {
        ApiResponse {
            status: StatusCode::INSUFFICIENT_STORAGE,
            code: "idempotency_quota_full",
            message: "The durable idempotency quota is full; retry after cleanup",
        }
    } else {
        ApiResponse {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "idempotency_unavailable",
            message: "Durable idempotency is unavailable; the command was not admitted",
        }
    }
}

fn engine_admin_failure(_error: String) -> ApiResponse {
    ApiResponse {
        status: StatusCode::BAD_REQUEST,
        code: "engine_namespace_rejected",
        message: "The scoped engine namespace rejected the request",
    }
}

fn release_failed_admission(
    state: &ServerState,
    binding: &IdempotencyBinding,
    request_id: Uuid,
) -> Result<(), ApiResponse> {
    let response = CommandResponse::failure(
        request_id,
        "runner_saturated",
        "The command was not admitted and may be retried fresh",
    );
    state
        .database
        .complete(binding, CompletionState::FailedTransient, &response)
        .map_err(persistence_failure)
}

async fn acquire_command_admission(
    state: &ServerState,
    command: &RunnerCommand,
) -> Result<Option<OwnedSemaphorePermit>, ApiResponse> {
    if matches!(
        command,
        RunnerCommand::StopBot { .. } | RunnerCommand::StopCampaign { .. }
    ) {
        return Ok(None);
    }
    let lane = if is_query_command(command) {
        state.query_admission.clone()
    } else {
        state.normal_admission.clone()
    };
    lane.try_acquire_owned().map(Some).map_err(|_| ApiResponse {
        status: StatusCode::TOO_MANY_REQUESTS,
        code: "runner_saturated",
        message: "The bounded runner admission lane is full; retry later",
    })
}

fn is_query_command(command: &RunnerCommand) -> bool {
    matches!(
        command,
        RunnerCommand::HandoverInventory
            | RunnerCommand::GetScorebookStats { .. }
            | RunnerCommand::ListLogSessions { .. }
            | RunnerCommand::GetLogPage { .. }
            | RunnerCommand::GetLogOutline { .. }
            | RunnerCommand::SearchLogSession { .. }
            | RunnerCommand::SearchLogSessions { .. }
            | RunnerCommand::GetLogsOverview
            | RunnerCommand::GetDiagnostics { .. }
    )
}

fn acquire_blocking_admission(
    state: &ServerState,
    command: &RunnerCommand,
) -> Result<Option<OwnedSemaphorePermit>, ApiResponse> {
    if !matches!(
        command,
        RunnerCommand::RegisterEngine { .. }
            | RunnerCommand::ConfigureOpeningBook { .. }
            | RunnerCommand::ImportLichessHistory { .. }
            | RunnerCommand::DeleteLogSession { .. }
            | RunnerCommand::ClearLogSessions
            | RunnerCommand::ClearDiagnostics
    ) {
        return Ok(None);
    }
    state
        .blocking_admission
        .clone()
        .try_acquire_owned()
        .map(Some)
        .map_err(|_| ApiResponse {
            status: StatusCode::TOO_MANY_REQUESTS,
            code: "blocking_work_saturated",
            message: "The bounded blocking-work pool is full; retry later",
        })
}

fn validate_runner_command(command: &RunnerCommand) -> Result<(), ApiResponse> {
    let invalid = match command {
        RunnerCommand::ListLogSessions { filter } => {
            filter.limit.is_none_or(|limit| limit == 0 || limit > 500)
                || filter.query.as_ref().is_some_and(|query| query.len() > 512)
        }
        RunnerCommand::GetLogPage { limit, .. } => *limit == 0 || *limit > 1_000,
        RunnerCommand::SearchLogSession { query, .. } => {
            query.limit == 0 || query.limit > 500 || query.text.len() > 512
        }
        RunnerCommand::SearchLogSessions { filter, query } => {
            filter.limit.is_none_or(|limit| limit == 0 || limit > 500)
                || filter.query.as_ref().is_some_and(|value| value.len() > 512)
                || query.limit == 0
                || query.limit > 500
                || query.text.len() > 512
        }
        _ => false,
    };
    if invalid {
        Err(ApiResponse {
            status: StatusCode::BAD_REQUEST,
            code: "query_limit_required",
            message: "Runner queries require bounded row and input limits",
        })
    } else {
        Ok(())
    }
}

fn parse_digest_header(headers: &HeaderMap) -> Result<[u8; 32], ApiResponse> {
    let encoded = required_header(headers, CONTENT_SHA256_HEADER, "missing_body_digest")?;
    if encoded.len() != 64 || !encoded.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ApiResponse {
            status: StatusCode::BAD_REQUEST,
            code: "invalid_body_digest",
            message: "The body digest must be 64 hexadecimal characters",
        });
    }
    let mut digest = [0u8; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&encoded[index * 2..index * 2 + 2], 16).map_err(|_| {
            ApiResponse {
                status: StatusCode::BAD_REQUEST,
                code: "invalid_body_digest",
                message: "The body digest is invalid",
            }
        })?;
    }
    Ok(digest)
}

fn required_header(
    headers: &HeaderMap,
    name: &str,
    code: &'static str,
) -> Result<String, ApiResponse> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or(ApiResponse {
            status: StatusCode::BAD_REQUEST,
            code,
            message: "A required request header is missing",
        })
}

async fn events(
    State(state): State<ServerState>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiResponse> {
    require_auth(&state, &headers)?;
    Ok(upgrade
        .on_upgrade(move |socket| event_stream(socket, state))
        .into_response())
}

async fn log_export(
    State(state): State<ServerState>,
    headers: HeaderMap,
    RoutePath((session_id, mode)): RoutePath<(String, String)>,
) -> Result<Response, ApiResponse> {
    require_auth(&state, &headers)?;
    let _query = state
        .query_admission
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiResponse {
            status: StatusCode::TOO_MANY_REQUESTS,
            code: "query_saturated",
            message: "The bounded query lane is full; retry later",
        })?;
    let parsed_mode = queen_core::enginelog::ExportMode::parse(&mode).map_err(|_| ApiResponse {
        status: StatusCode::BAD_REQUEST,
        code: "export_failed",
        message: "The requested log export mode is invalid",
    })?;
    let bytes = tokio::time::timeout(
        QUERY_DEADLINE,
        queen_core::export_log_session_bytes_bounded(
            session_id,
            parsed_mode,
            LOG_EXPORT_MAX_BYTES,
            CoreStateRef::new(&state.core),
        ),
    )
    .await
    .map_err(|_| ApiResponse {
        status: StatusCode::REQUEST_TIMEOUT,
        code: "export_timeout",
        message: "The bounded log export deadline was exceeded",
    })?
    .map_err(|_| ApiResponse {
        status: StatusCode::BAD_REQUEST,
        code: "export_failed",
        message: "The requested log could not be exported",
    })?;
    let content_type = if mode == "archive" {
        "application/gzip"
    } else {
        "text/plain; charset=utf-8"
    };
    Ok(([(axum::http::header::CONTENT_TYPE, content_type)], bytes).into_response())
}

async fn event_stream(mut socket: WebSocket, state: ServerState) {
    let mut events = state.events.subscribe();
    let mut cursor = state.sequence.load(Ordering::Relaxed);
    let initial = EventEnvelope {
        protocol_version: PROTOCOL_VERSION,
        instance_id: state.instance_id,
        sequence: cursor,
        event: CoreEvent::Snapshot(state.core.snapshot().await),
    };
    if send_event(&mut socket, &initial).await.is_err() {
        return;
    }
    loop {
        match events.recv().await {
            Ok(event) if event.sequence > cursor => {
                if send_event(&mut socket, &event).await.is_err() {
                    return;
                }
                cursor = event.sequence;
            }
            Ok(_) => {}
            Err(broadcast::error::RecvError::Lagged(_)) => {
                let replacement = EventEnvelope {
                    protocol_version: PROTOCOL_VERSION,
                    instance_id: state.instance_id,
                    sequence: state.sequence.load(Ordering::Relaxed),
                    event: CoreEvent::Snapshot(state.core.snapshot().await),
                };
                if send_event(&mut socket, &replacement).await.is_err() {
                    return;
                }
                cursor = replacement.sequence;
            }
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

async fn send_event(socket: &mut WebSocket, event: &EventEnvelope) -> Result<(), ()> {
    let encoded = serde_json::to_string(event).map_err(|_| ())?;
    socket
        .send(Message::Text(encoded.into()))
        .await
        .map_err(|_| ())
}

async fn execute(server: &ServerState, command: RunnerCommand) -> Result<Value, String> {
    let core = &server.core;
    let state = || CoreStateRef::new(core);
    match command {
        RunnerCommand::RegisterEngine {
            root_id,
            relative_path,
        } => value(
            server
                .engine_admin
                .register(root_id, relative_path, core)
                .await?,
        ),
        RunnerCommand::RemoveEngine { engine_id } => {
            queen_core::remove_engine(engine_id, state()).await?;
            server.engine_admin.garbage_collect(core).await;
            Ok(Value::Null)
        }
        RunnerCommand::UpdateEngineOptions { engine_id, options } => {
            queen_core::update_engine_options(engine_id, options, state()).await?;
            Ok(Value::Null)
        }
        RunnerCommand::RefreshEngineOptions { engine_id } => {
            queen_core::refresh_engine_options(engine_id, state()).await?;
            Ok(Value::Null)
        }
        RunnerCommand::ConfigureOpeningBook { mut request } => {
            let current_managed_path = core
                .snapshot()
                .await
                .engines
                .into_iter()
                .find(|engine| engine.id == request.engine_id)
                .and_then(|engine| engine.opening_book.map(|book| book.path));
            request.path = server
                .engine_admin
                .validate_opening_book(&request.path, current_managed_path.as_deref())?;
            value(queen_core::configure_opening_book(request, state()).await?)
        }
        RunnerCommand::ClearEngineOpeningBook { engine_id } => {
            queen_core::clear_engine_opening_book(engine_id, state()).await?;
            Ok(Value::Null)
        }
        RunnerCommand::AddLichessAccount { request } => {
            value(queen_core::add_lichess_account(request, state()).await?)
        }
        RunnerCommand::UpdateLichessAccountToken { account_id, token } => {
            value(queen_core::update_lichess_account_token(account_id, token, state()).await?)
        }
        RunnerCommand::UpdateAccountEngine {
            account_id,
            engine_id,
        } => {
            queen_core::update_account_engine(account_id, engine_id, state()).await?;
            Ok(Value::Null)
        }
        RunnerCommand::RemoveLichessAccount { account_id } => {
            queen_core::remove_lichess_account(account_id, state()).await?;
            Ok(Value::Null)
        }
        RunnerCommand::StartBot { account_id } => {
            server.lifecycle.start_bot(account_id).await?;
            Ok(Value::Null)
        }
        RunnerCommand::StopBot { account_id } => {
            server.lifecycle.stop_bot(account_id).await?;
            Ok(Value::Null)
        }
        RunnerCommand::StartCampaign { settings } => {
            server.lifecycle.start_campaign(settings).await?;
            Ok(Value::Null)
        }
        RunnerCommand::StopCampaign { account_id } => {
            server.lifecycle.stop_campaign(account_id).await?;
            Ok(Value::Null)
        }
        RunnerCommand::CreateChallenge { request } => {
            value(queen_core::create_challenge(request, state()).await?)
        }
        RunnerCommand::DismissGameError { game_id } => {
            queen_core::dismiss_game_error(game_id, state()).await?;
            Ok(Value::Null)
        }
        RunnerCommand::HandoverInventory => value(HandoverInventory {
            live_games: core.live_game_ownership_count().await,
            outgoing_challenges: core.outstanding_outgoing_challenge_count().await,
        }),
        RunnerCommand::GetScorebookStats { filter } => {
            value(queen_core::get_scorebook_stats(filter, state()).await?)
        }
        RunnerCommand::ImportLichessHistory { account_id, max } => {
            value(queen_core::import_lichess_history(account_id, max, state()).await?)
        }
        RunnerCommand::ListLogSessions { filter } => {
            value(queen_core::list_log_sessions(filter, state()).await?)
        }
        RunnerCommand::GetLogPage {
            session_id,
            offset,
            limit,
        } => value(queen_core::get_log_page(session_id, offset, limit, state()).await?),
        RunnerCommand::GetLogOutline { session_id } => {
            value(queen_core::get_log_outline(session_id, state()).await?)
        }
        RunnerCommand::SearchLogSession { session_id, query } => {
            value(queen_core::search_log_session(session_id, query, state()).await?)
        }
        RunnerCommand::SearchLogSessions { filter, query } => {
            value(queen_core::search_log_sessions(filter, query, state()).await?)
        }
        RunnerCommand::DeleteLogSession { session_id } => {
            queen_core::delete_log_session(session_id, state()).await?;
            Ok(Value::Null)
        }
        RunnerCommand::ClearLogSessions => value(queen_core::clear_log_sessions(state()).await?),
        RunnerCommand::GetLogsOverview => value(queen_core::get_logs_overview(state()).await?),
        RunnerCommand::SetLogRetention { retention } => {
            queen_core::set_log_retention(retention, state()).await?;
            Ok(Value::Null)
        }
        RunnerCommand::GetDiagnostics { filter } => {
            value(queen_core::get_diagnostics(filter, state()).await?)
        }
        RunnerCommand::ClearDiagnostics => {
            queen_core::clear_diagnostics(state()).await?;
            Ok(Value::Null)
        }
    }
}

fn command_spec(command: &RunnerCommand) -> (&'static str, &'static str) {
    match command {
        RunnerCommand::RegisterEngine { .. } => (
            "registerEngine",
            "engine registry and content-addressed store",
        ),
        RunnerCommand::RemoveEngine { .. } => ("removeEngine", "engine registry and process table"),
        RunnerCommand::UpdateEngineOptions { .. } => {
            ("updateEngineOptions", "engine registry and process table")
        }
        RunnerCommand::RefreshEngineOptions { .. } => {
            ("refreshEngineOptions", "engine registry and process table")
        }
        RunnerCommand::ConfigureOpeningBook { .. } => {
            ("configureOpeningBook", "engine configuration state")
        }
        RunnerCommand::ClearEngineOpeningBook { .. } => {
            ("clearEngineOpeningBook", "engine configuration state")
        }
        RunnerCommand::AddLichessAccount { .. } => ("addLichessAccount", "Lichess account state"),
        RunnerCommand::UpdateLichessAccountToken { .. } => {
            ("updateLichessAccountToken", "Lichess account secret")
        }
        RunnerCommand::UpdateAccountEngine { .. } => {
            ("updateAccountEngine", "Lichess account and engine state")
        }
        RunnerCommand::RemoveLichessAccount { .. } => {
            ("removeLichessAccount", "Lichess account state")
        }
        RunnerCommand::StartBot { .. } => ("startBot", "Lichess account state"),
        RunnerCommand::StopBot { .. } => ("stopBot", "Lichess account state"),
        RunnerCommand::StartCampaign { .. } => ("startCampaign", "Lichess challenge state"),
        RunnerCommand::StopCampaign { .. } => ("stopCampaign", "Lichess challenge state"),
        RunnerCommand::CreateChallenge { .. } => ("createChallenge", "Lichess challenge state"),
        RunnerCommand::DismissGameError { .. } => ("dismissGameError", "retained game error state"),
        RunnerCommand::HandoverInventory => {
            ("handoverInventory", "runner automation ownership state")
        }
        RunnerCommand::GetScorebookStats { .. } => ("getScorebookStats", "runner history state"),
        RunnerCommand::ImportLichessHistory { .. } => {
            ("importLichessHistory", "Lichess game history state")
        }
        RunnerCommand::ListLogSessions { .. } => ("listLogSessions", "runner log state"),
        RunnerCommand::GetLogPage { .. } => ("getLogPage", "runner log state"),
        RunnerCommand::GetLogOutline { .. } => ("getLogOutline", "runner log state"),
        RunnerCommand::SearchLogSession { .. } => ("searchLogSession", "runner log state"),
        RunnerCommand::SearchLogSessions { .. } => ("searchLogSessions", "runner log state"),
        RunnerCommand::DeleteLogSession { .. } => ("deleteLogSession", "runner log state"),
        RunnerCommand::ClearLogSessions => ("clearLogSessions", "runner log state"),
        RunnerCommand::GetLogsOverview => ("getLogsOverview", "runner log state"),
        RunnerCommand::SetLogRetention { .. } => ("setLogRetention", "runner log state"),
        RunnerCommand::GetDiagnostics { .. } => ("getDiagnostics", "runner diagnostics state"),
        RunnerCommand::ClearDiagnostics => ("clearDiagnostics", "runner diagnostics state"),
    }
}

fn transient_failure(error: &str) -> bool {
    let normalized = error.to_ascii_lowercase();
    [
        "could not reach lichess",
        "timed out",
        "timeout",
        "deadline",
        "temporar",
        "rate limit",
        "database is locked",
        "connection reset",
    ]
    .iter()
    .any(|fragment| normalized.contains(fragment))
}

fn redact_for_log(message: &str) -> String {
    let mut redacted = message.to_string();
    for marker in ["enroll=", "Bearer "] {
        let mut search_from = 0;
        while let Some(relative) = redacted[search_from..].find(marker) {
            let start = search_from + relative + marker.len();
            let end = redacted[start..]
                .find(|character: char| {
                    character == '&' || character.is_ascii_whitespace() || character == '"'
                })
                .map_or(redacted.len(), |offset| start + offset);
            redacted.replace_range(start..end, "<redacted>");
            search_from = start + "<redacted>".len();
        }
    }
    redacted
}

fn value<T: serde::Serialize>(value: T) -> Result<Value, String> {
    serde_json::to_value(value)
        .map_err(|error| format!("Could not encode runner response: {error}"))
}

fn require_auth(state: &ServerState, headers: &HeaderMap) -> Result<u64, ApiResponse> {
    state
        .authorized(headers)
        .map_err(|_| ApiResponse {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "authentication_unavailable",
            message: "Runner authentication is temporarily unavailable",
        })?
        .ok_or(ApiResponse {
            status: StatusCode::UNAUTHORIZED,
            code: "unauthorized",
            message: "Supply the runner bearer token",
        })
}

#[derive(Debug)]
struct ApiResponse {
    status: StatusCode,
    code: &'static str,
    message: &'static str,
}

impl IntoResponse for ApiResponse {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({ "error": { "code": self.code, "message": self.message } })),
        )
            .into_response()
    }
}

fn hostname() -> String {
    env::var("HOSTNAME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|value| value.trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let Ok(mut terminate) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        else {
            eprintln!("queen-runner: could not install SIGTERM handler; waiting for Ctrl-C");
            let _ = tokio::signal::ctrl_c().await;
            return;
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod tests {
    #[cfg(not(windows))]
    use super::acquire_command_admission;
    use super::{
        await_reservation_for, canonical_public_url, execute, pending_response, redact_for_log,
        release_failed_admission, router, validate_runner_command, ServerState,
        RUNNER_SHUTDOWN_BUDGET,
    };
    use crate::engine_admin::EngineAdmin;
    use crate::persistence::{IdempotencyBinding, Reservation, RunnerDatabase};
    use axum::{
        body::{to_bytes, Body, Bytes},
        http::{header::AUTHORIZATION, Request, StatusCode},
    };
    use futures_util::stream;
    use queen_core::{
        models::{AccountProfile, AppConfig, EngineProfile},
        storage::{FileSecretStore, SecretStore},
        AppState,
    };
    use queen_protocol::{OpeningBookAsset, PendingResponse, RunnerCommand, PROTOCOL_VERSION};
    #[cfg(not(windows))]
    use std::time::Instant;
    use std::{
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
        task::Poll,
        time::Duration,
    };
    use tower::ServiceExt;
    use uuid::Uuid;

    #[test]
    fn pairing_url_is_canonical_https_only() {
        assert_eq!(
            canonical_public_url("HTTPS://BÜCHER.example:443/runner/").unwrap(),
            "https://xn--bcher-kva.example/runner"
        );
        assert!(canonical_public_url("http://127.0.0.1:7788").is_err());
        assert!(canonical_public_url("https://user@runner.example").is_err());
    }

    #[test]
    fn log_boundary_redacts_enrollment_and_bearer_values() {
        let enrollment = "enrollment-secret-value";
        let bearer = "bearer-secret-value";
        let message = format!(
            "failed queenui://pair?v=2&enroll={enrollment}&fp=abc Authorization: Bearer {bearer}"
        );
        let redacted = redact_for_log(&message);
        assert!(!redacted.contains(enrollment));
        assert!(!redacted.contains(bearer));
        assert!(redacted.matches("<redacted>").count() >= 2);
    }

    #[tokio::test]
    async fn duplicate_pending_wait_is_bounded_and_returns_202_with_the_key() {
        let directory =
            std::env::temp_dir().join(format!("queen-runner-pending-{}", Uuid::new_v4()));
        let database = RunnerDatabase::open(directory.clone(), [3; 32]).unwrap();
        let key = Uuid::new_v4();
        let binding = IdempotencyBinding {
            key,
            credential_generation: 1,
            protocol_version: PROTOCOL_VERSION,
            method: "POST",
            normalized_path: "/v2/commands",
            body_digest: [4; 32],
            command_kind: "stopBot",
            reconciliation: "Lichess account state",
        };
        assert!(matches!(
            database.reserve(&binding).unwrap(),
            Reservation::Execute
        ));
        assert!(matches!(
            await_reservation_for(&database, &binding, Duration::ZERO).await,
            Ok(Reservation::Pending)
        ));

        let response = pending_response(key);
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let pending: PendingResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(pending.request_id, key);
        assert_eq!(pending.status, "pending");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn runner_log_queries_reject_uncapped_rows_and_oversized_search_input() {
        let uncapped = RunnerCommand::ListLogSessions {
            filter: queen_core::enginelog::LogFilter::default(),
        };
        assert!(validate_runner_command(&uncapped).is_err());
        let oversized = RunnerCommand::SearchLogSession {
            session_id: "session".into(),
            query: queen_core::enginelog::LogQuery {
                text: "x".repeat(513),
                limit: 10,
                ..Default::default()
            },
        };
        assert!(validate_runner_command(&oversized).is_err());
    }

    #[tokio::test]
    async fn authenticated_opening_book_endpoint_lists_only_admin_assets() {
        let root = std::env::temp_dir().join(format!("queenui-opening-assets-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let book = root.join("approved.bin");
        std::fs::write(&book, [0_u8; 16]).unwrap();
        std::fs::write(
            root.join("runner-config.json"),
            serde_json::json!({"opening_book_allowlist": [book.clone()]}).to_string(),
        )
        .unwrap();
        let core = AppState::new_with_secret_store(
            root.clone(),
            AppConfig::default(),
            Arc::new(FileSecretStore::new(root.join("secrets"))),
        )
        .unwrap();
        let database = RunnerDatabase::open(root.clone(), [8; 32]).unwrap();
        let enrollment = database.mint_enrollment(false, 100).unwrap();
        let bearer = database.redeem(&enrollment.code, 101).unwrap().bearer;
        let state = ServerState::new(core, database, EngineAdmin::load(&root).unwrap());

        let unauthenticated = router(state.clone())
            .oneshot(
                Request::get("/v2/opening-books")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

        let authenticated = router(state.clone())
            .oneshot(
                Request::get("/v2/opening-books")
                    .header(AUTHORIZATION, format!("Bearer {bearer}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(authenticated.status(), StatusCode::OK);
        let body = to_bytes(authenticated.into_body(), 4096).await.unwrap();
        let assets: Vec<OpeningBookAsset> = serde_json::from_slice(&body).unwrap();
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].name, "approved.bin");
        assert_eq!(assets[0].size, 16);
        assert_eq!(
            assets[0].path,
            book.canonicalize().unwrap().to_string_lossy()
        );

        state.shutdown_forwarder().await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn rejected_admission_releases_the_pending_idempotency_reservation_for_retry() {
        let root = std::env::temp_dir().join(format!("queenui-admission-retry-{}", Uuid::new_v4()));
        let core = AppState::new_with_secret_store(
            root.clone(),
            AppConfig::default(),
            Arc::new(FileSecretStore::new(root.join("secrets"))),
        )
        .unwrap();
        let database = RunnerDatabase::open(root.clone(), [6; 32]).unwrap();
        let state = ServerState::new(core, database.clone(), EngineAdmin::load(&root).unwrap());
        let binding = IdempotencyBinding {
            key: Uuid::new_v4(),
            credential_generation: 1,
            protocol_version: PROTOCOL_VERSION,
            method: "POST",
            normalized_path: "/v2/commands",
            body_digest: [2; 32],
            command_kind: "startBot",
            reconciliation: "Lichess account state",
        };
        assert!(matches!(
            database.reserve(&binding).unwrap(),
            Reservation::Execute
        ));
        release_failed_admission(&state, &binding, binding.key).unwrap();
        assert!(matches!(
            database.reserve(&binding).unwrap(),
            Reservation::Execute
        ));
        state.shutdown_forwarder().await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn graceful_shutdown_budget_fits_the_shipped_service_deadline() {
        let unit = include_str!("../../../deploy/systemd/queen-runner.service");
        let stop_seconds = unit
            .lines()
            .find_map(|line| line.strip_prefix("TimeoutStopSec="))
            .expect("systemd stop deadline")
            .parse::<u64>()
            .expect("numeric systemd stop deadline");
        assert!(
            RUNNER_SHUTDOWN_BUDGET.as_secs() < stop_seconds,
            "the runner must return before systemd is allowed to send SIGKILL"
        );
    }

    #[tokio::test]
    async fn trusted_engine_dispatch_refuses_upload_and_legacy_paths_without_polling_their_bodies()
    {
        let root = std::env::temp_dir().join(format!("queenui-disabled-upload-{}", Uuid::new_v4()));
        let core = AppState::new_with_secret_store(
            root.clone(),
            AppConfig::default(),
            Arc::new(FileSecretStore::new(root.join("secrets"))),
        )
        .unwrap();
        let database = RunnerDatabase::open(root.clone(), [7; 32]).unwrap();
        let state = ServerState::new(core, database, EngineAdmin::load(&root).unwrap());
        for endpoint in ["/v2/engines/upload", "/v2/engines/register-path"] {
            let polls = Arc::new(AtomicUsize::new(0));
            let body_polls = polls.clone();
            let body = Body::from_stream(stream::poll_fn(move |_| {
                body_polls.fetch_add(1, Ordering::SeqCst);
                Poll::Ready(Some(Ok::<Bytes, std::io::Error>(Bytes::from_static(
                    b"secret executable bytes",
                ))))
            }));
            let response = router(state.clone())
                .oneshot(Request::post(endpoint).body(body).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
            assert_eq!(polls.load(Ordering::SeqCst), 0, "body polled at {endpoint}");
        }
        state.shutdown_forwarder().await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stop_completes_under_two_seconds_when_normal_upload_query_and_engine_output_are_saturated(
    ) {
        use queen_core::uci::{EngineGovernor, EngineLimits, UciEngine};
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!("queenui-saturation-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let engine_path = root.join("flooding-uci.sh");
        std::fs::write(
            &engine_path,
            r#"#!/bin/sh
ready=0
while IFS= read -r line; do
  case "$line" in
    uci) echo "id name Flood"; echo "uciok" ;;
    isready)
      echo "readyok"
      ready=$((ready + 1))
      if [ "$ready" -ge 2 ]; then
        while :; do echo "info depth 1 score cp 0 nodes 1"; done
      fi
      ;;
  esac
done
"#,
        )
        .unwrap();
        std::fs::set_permissions(&engine_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let governor = EngineGovernor::new(EngineLimits {
            simultaneous_engines: 1,
            total_memory_mb: 512,
            total_cpu_threads: 1,
            total_tasks: 64,
            output_bytes_per_engine_per_second: 8 * 1024 * 1024,
            total_output_bytes_per_second: 8 * 1024 * 1024,
            output_bytes_per_engine: 64 * 1024 * 1024,
        })
        .unwrap();
        let mut engine =
            UciEngine::start_governed(engine_path.to_str().unwrap(), &[], None, &governor)
                .await
                .unwrap();

        let core = AppState::new_with_secret_store(
            root.clone(),
            AppConfig {
                accounts: vec![AccountProfile {
                    id: "saturated-account".into(),
                    username: "Saturated".into(),
                    engine_id: "unused".into(),
                    rating: None,
                    enabled: false,
                }],
                ..AppConfig::default()
            },
            Arc::new(FileSecretStore::new(root.join("secrets"))),
        )
        .unwrap();
        let database = RunnerDatabase::open(root.clone(), [8; 32]).unwrap();
        let state = ServerState::new(core, database, EngineAdmin::load(&root).unwrap());
        let _normal = state
            .normal_admission
            .clone()
            .acquire_many_owned(32)
            .await
            .unwrap();
        let _upload = state
            ._upload_admission
            .clone()
            .acquire_owned()
            .await
            .unwrap();
        let _query = state
            .query_admission
            .clone()
            .acquire_many_owned(4)
            .await
            .unwrap();

        let stop = RunnerCommand::StopBot {
            account_id: "saturated-account".into(),
        };
        let started = Instant::now();
        let admission = acquire_command_admission(&state, &stop).await.unwrap();
        assert!(admission.is_none(), "Stop queued at normal admission");
        execute(&state, stop).await.unwrap();
        assert!(started.elapsed() < Duration::from_secs(2));

        engine.shutdown().await;
        state.shutdown_forwarder().await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn remove_account_runner_command_crosses_dispatch_and_deletes_runner_secret() {
        let root =
            std::env::temp_dir().join(format!("queenui-runner-remove-{}", uuid::Uuid::new_v4()));
        let secrets = Arc::new(FileSecretStore::new(root.join("secrets")));
        secrets.store("bot", "lichess-token").unwrap();
        let config = AppConfig {
            engines: vec![EngineProfile {
                id: "engine".into(),
                name: "unused".into(),
                path: "unused".into(),
                author: None,
                option_count: 0,
                last_probed_at_ms: None,
                probe_ok: None,
                options: Vec::new(),
                opening_book: None,
            }],
            accounts: vec![AccountProfile {
                id: "bot".into(),
                username: "Bot".into(),
                engine_id: "engine".into(),
                rating: None,
                enabled: false,
            }],
            ..AppConfig::default()
        };
        let core = AppState::new_with_secret_store(root.clone(), config, secrets.clone()).unwrap();
        let database = RunnerDatabase::open(root.clone(), [9; 32]).unwrap();
        let server = ServerState::new(core.clone(), database, EngineAdmin::load(&root).unwrap());
        execute(
            &server,
            RunnerCommand::RemoveLichessAccount {
                account_id: "bot".into(),
            },
        )
        .await
        .unwrap();
        assert!(core.snapshot().await.accounts.is_empty());
        assert!(secrets.get("bot").is_err());
        server.shutdown_forwarder().await;
        drop(server);
        drop(core);
        let _ = std::fs::remove_dir_all(root);
    }
}
