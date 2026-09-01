use crate::{
    history::perf_key_for_clock,
    lichess,
    models::{
        AccountProfile, CampaignEvent, CampaignRuntime, CampaignSettings, CampaignStatus,
        ChallengeRequest, OnlineBot,
    },
    storage, AppState,
};
use futures_util::FutureExt;
use rand::seq::SliceRandom;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    panic::AssertUnwindSafe,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const DISCOVERY_INTERVAL: Duration = Duration::from_secs(20);
const PENDING_LIFETIME: Duration = Duration::from_secs(24);
const OPPONENT_COOLDOWN: Duration = Duration::from_secs(15 * 60);
const CHALLENGE_SPACING: Duration = Duration::from_secs(2);
const MAX_CONCURRENCY: u32 = 8;
const MAX_ACTIVITY_EVENTS: usize = 60;
const CAMPAIGN_JOIN_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_RUN_MINUTES: u32 = 7 * 24 * 60;
const MAX_RUN_GAMES: u32 = 10_000;

struct PendingChallenge {
    opponent: String,
    created_at: Instant,
    state: PendingState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum PendingState {
    Active,
    CancelPending(String),
}

pub(crate) struct CampaignTask {
    pub(crate) generation: u64,
    pub(crate) cancellation: CancellationToken,
    pub(crate) handle: Option<JoinHandle<Result<(), String>>>,
    /// Games whose `gameStart` was first observed while this exact campaign
    /// generation was active and which have not reached a terminal state yet.
    pub(crate) games: HashSet<String>,
    /// Terminal game ids retained for the campaign generation so duplicate or
    /// reconnected account/per-game streams cannot count or start them twice.
    pub(crate) settled_games: HashSet<String>,
}

#[derive(Default)]
struct FilterStats {
    total: u32,
    missing_pool: u32,
    provisional_or_unplayed: u32,
    outside_range: u32,
    busy_or_cooling_down: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct CampaignCapacity {
    occupied_slots: u32,
    games_completed: u64,
}

#[derive(Debug)]
struct IncomingChallenge {
    id: String,
    opponent: String,
}

#[derive(Debug)]
struct IncomingRejection {
    lichess_reason: &'static str,
    detail: String,
}

pub(super) async fn start(state: AppState, settings: CampaignSettings) -> Result<(), String> {
    validate(&settings)?;
    let finished = {
        let mut tasks = state.0.campaign_tasks.lock().await;
        match tasks.get(&settings.account_id) {
            Some(task) if task.handle.as_ref().is_some_and(JoinHandle::is_finished) => {
                tasks.remove(&settings.account_id)
            }
            Some(task) if task.handle.is_none() => {
                return Err("This campaign is still starting; wait for it to finish".into())
            }
            Some(_) => return Err("This account already has an active challenge campaign.".into()),
            None => None,
        }
    };
    if let Some(mut task) = finished {
        if let Some(handle) = task.handle.take() {
            let _ = handle.await;
        }
    }
    let cancellation = CancellationToken::new();
    let generation = state
        .0
        .campaign_generation
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        .saturating_add(1);
    {
        let mut tasks = state.0.campaign_tasks.lock().await;
        if tasks.contains_key(&settings.account_id) {
            return Err("This account already has an active challenge campaign.".into());
        }
        tasks.insert(
            settings.account_id.clone(),
            CampaignTask {
                generation,
                cancellation: cancellation.clone(),
                handle: None,
                games: HashSet::new(),
                settled_games: HashSet::new(),
            },
        );
    }

    let setup = async {
        let mut config = state.0.config.write().await;
        if !config
            .accounts
            .iter()
            .any(|account| account.id == settings.account_id)
        {
            return Err("Lichess account not found".into());
        }
        config
            .campaigns
            .retain(|campaign| campaign.account_id != settings.account_id);
        config.campaigns.push(settings.clone());
        storage::save(&state.0.config_path, &config)?;
        drop(config);

        if cancellation.is_cancelled() {
            return Err("The campaign was stopped during startup".into());
        }

        state.start_bot(&settings.account_id).await?;
        for _ in 0..50 {
            if account_connected(&state, &settings.account_id).await {
                break;
            }
            if wait_or_cancel(&cancellation, Duration::from_millis(100)).await {
                return Err("The campaign was stopped while waiting for the bot connection".into());
            }
        }
        if !account_connected(&state, &settings.account_id).await {
            return Err(
                "The bot could not connect to Lichess, so matchmaking was not started.".into(),
            );
        }
        state.token(&settings.account_id)
    }
    .await;
    let token = match setup {
        Ok(token) => token,
        Err(error) => {
            state
                .0
                .campaign_tasks
                .lock()
                .await
                .remove(&settings.account_id);
            return Err(error);
        }
    };
    let started_at = epoch_millis();
    let stop_at = settings.stop_after_minutes.map(|minutes| {
        started_at.saturating_add(Duration::from_secs(u64::from(minutes) * 60).as_millis() as u64)
    });
    state.0.campaign_runtimes.write().await.insert(
        settings.account_id.clone(),
        CampaignRuntime {
            account_id: settings.account_id.clone(),
            status: CampaignStatus::Starting,
            active_games: 0,
            pending_challenges: 0,
            eligible_bots: 0,
            online_bots_scanned: 0,
            challenges_sent: 0,
            games_started: 0,
            games_completed: 0,
            last_opponent: None,
            activity: "Connecting matchmaking…".into(),
            error: None,
            next_scan_at: None,
            stop_at,
            events: vec![new_event(
                "start",
                "Matchmaking started",
                Some(format!(
                    "Rating {}–{} · concurrency {} · {}+{} · {} · {} · {}",
                    settings.min_rating,
                    settings.max_rating,
                    settings.concurrency,
                    settings.clock_limit / 60,
                    settings.clock_increment,
                    if settings.rated { "rated" } else { "casual" },
                    if settings.accept_incoming_challenges {
                        "incoming challenges enabled"
                    } else {
                        "outgoing only"
                    },
                    campaign_limit_label(&settings)
                )),
            )],
        },
    );
    state.emit_snapshot().await;

    let task_cancellation = cancellation.clone();
    let task_state = state.clone();
    let install_account_id = settings.account_id.clone();
    let mut handle = Some(tokio::spawn(async move {
        let account_id = settings.account_id.clone();
        let outcome = AssertUnwindSafe(run(task_state.clone(), settings, token, task_cancellation))
            .catch_unwind()
            .await;
        match outcome {
            Ok(result) => result,
            Err(panic) => {
                let detail = panic
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(|value| value.to_string()))
                    .unwrap_or_else(|| "Unknown campaign controller panic".into());
                update_runtime(&task_state, &account_id, |runtime| {
                    runtime.status = CampaignStatus::Error;
                    runtime.activity = "The campaign controller stopped unexpectedly".into();
                    runtime.error = Some(detail.clone());
                    runtime.next_scan_at = None;
                })
                .await;
                record_event(
                    &task_state,
                    &account_id,
                    "error",
                    "Campaign controller crashed",
                    Some(detail.clone()),
                )
                .await;
                Err(format!("Campaign controller panicked: {detail}"))
            }
        }
    }));
    let installed = {
        let mut tasks = state.0.campaign_tasks.lock().await;
        if let Some(task) = tasks
            .get_mut(&install_account_id)
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
        if let Some(mut handle) = handle {
            if tokio::time::timeout(Duration::from_secs(15), &mut handle)
                .await
                .is_err()
            {
                handle.abort();
                let _ = handle.await;
            }
        }
        return Err("The campaign was canceled before its task was registered".into());
    }
    Ok(())
}

pub(super) async fn stop(state: &AppState, account_id: &str) -> Result<(), String> {
    let task = {
        let mut tasks = state.0.campaign_tasks.lock().await;
        tasks.get_mut(account_id).map(|task| {
            task.cancellation.cancel();
            (task.generation, task.handle.take())
        })
    };
    if let Some((generation, handle)) = task {
        update_runtime(state, account_id, |runtime| {
            runtime.status = CampaignStatus::Stopping;
            runtime.activity = "Canceling outstanding challenges…".into();
            push_event(
                runtime,
                new_event(
                    "stop",
                    "Stop requested",
                    Some("Canceling unanswered challenges".into()),
                ),
            );
        })
        .await;
        let result = if let Some(mut handle) = handle {
            match tokio::time::timeout(CAMPAIGN_JOIN_TIMEOUT, &mut handle).await {
                Ok(Ok(result)) => result,
                Ok(Err(error)) => Err(format!("Campaign task failed while joining: {error}")),
                Err(_) => {
                    handle.abort();
                    let _ = handle.await;
                    Err(format!(
                        "Campaign did not stop within {} seconds and was aborted",
                        CAMPAIGN_JOIN_TIMEOUT.as_secs()
                    ))
                }
            }
        } else {
            Ok(())
        };
        {
            let mut tasks = state.0.campaign_tasks.lock().await;
            if tasks
                .get(account_id)
                .is_some_and(|task| task.generation == generation)
            {
                tasks.remove(account_id);
            }
        }
        if let Err(detail) = result {
            let active_games = active_game_count(state, account_id).await;
            update_runtime(state, account_id, |runtime| {
                runtime.status = CampaignStatus::Error;
                runtime.active_games = active_games;
                runtime.activity = "Campaign shutdown failed".into();
                runtime.error = Some(detail.clone());
            })
            .await;
            state.emit_snapshot().await;
            return Err(detail);
        }
    } else {
        let active_games = active_game_count(state, account_id).await;
        if let Some(runtime) = state.0.campaign_runtimes.write().await.get_mut(account_id) {
            // Runner shutdown stops campaigns before account supervisors. Its
            // second idempotent stop must not erase an unresolved-cancel error
            // or pretend that challenge capacity has been released.
            runtime.active_games = active_games;
            if runtime.status != CampaignStatus::Error {
                runtime.status = CampaignStatus::Stopped;
                runtime.activity = "Ready".into();
                runtime.pending_challenges = 0;
            }
        }
    }
    state.emit_snapshot().await;
    Ok(())
}

async fn run(
    state: AppState,
    settings: CampaignSettings,
    token: String,
    cancellation: CancellationToken,
) -> Result<(), String> {
    run_with_pending_lifetime(state, settings, token, cancellation, PENDING_LIFETIME).await
}

async fn run_with_pending_lifetime(
    state: AppState,
    settings: CampaignSettings,
    token: String,
    cancellation: CancellationToken,
    pending_lifetime: Duration,
) -> Result<(), String> {
    let mut pending: HashMap<String, PendingChallenge> = HashMap::new();
    let mut recent_opponents: HashMap<String, Instant> = HashMap::new();
    let mut next_discovery = Instant::now();
    let mut challenges_sent = 0u64;
    let mut next_cancellation_reconciliation = Instant::now();
    let perf = perf_key_for_clock(settings.clock_limit, settings.clock_increment);
    let deadline = settings
        .stop_after_minutes
        .map(|minutes| Instant::now() + Duration::from_secs(u64::from(minutes) * 60));
    let mut automatic_stop_reason: Option<String> = None;
    // Campaign startup and every ambiguous POST pass through the same
    // authoritative reconciliation barrier before any new challenge creation.
    let mut unknown_creation = Some("startup reconciliation".to_string());

    'campaign: while !cancellation.is_cancelled() {
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            automatic_stop_reason = Some(format!(
                "The configured {} minute run time elapsed",
                settings.stop_after_minutes.unwrap_or_default()
            ));
            break;
        }
        let capacity = campaign_capacity(&state, &settings.account_id).await;
        if settings
            .stop_after_games
            .is_some_and(|limit| capacity.games_completed >= u64::from(limit))
        {
            automatic_stop_reason = Some(format!(
                "The configured {} completed-game limit was reached",
                settings.stop_after_games.unwrap_or_default()
            ));
            break;
        }
        if let Some(backoff_until) = campaign_backoff_until(&state, &settings.account_id).await {
            let remaining_ms = backoff_until.saturating_sub(epoch_millis()).min(1_000);
            if wait_or_cancel(&cancellation, Duration::from_millis(remaining_ms.max(1))).await {
                break;
            }
            continue;
        }
        if unknown_creation.is_none() {
            unknown_creation = state
                .0
                .uncertain_challenge_creations
                .lock()
                .await
                .get(&settings.account_id)
                .cloned();
        }
        if unknown_creation.is_some() {
            let reconciliation = {
                let _gate = state.0.matchmaking_api_gate.lock().await;
                tokio::select! {
                    _ = cancellation.cancelled() => break,
                    result = lichess::outgoing_challenges(
                        &state.0.api_base,
                        &state.0.api_client,
                        &token,
                    ) => result,
                }
            };
            match reconciliation {
                Ok(authoritative) => {
                    reconcile_pending_authoritatively(
                        &state,
                        &settings.account_id,
                        &mut pending,
                        authoritative,
                    )
                    .await;
                    if let Err(error) = state
                        .clear_uncertain_challenge_creation(&settings.account_id)
                        .await
                    {
                        let detail = format!(
                            "Challenge creation remains paused because the reconciled safety barrier could not be updated: {error}"
                        );
                        update_runtime(&state, &settings.account_id, |runtime| {
                            runtime.status = CampaignStatus::Unknown;
                            runtime.pending_challenges = pending.len().saturating_add(1) as u32;
                            runtime.activity =
                                "Preserving the uncertain challenge safety barrier…".into();
                            runtime.error = Some(detail.clone());
                            runtime.next_scan_at = None;
                        })
                        .await;
                        state.emit_snapshot().await;
                        if wait_or_cancel(&cancellation, Duration::from_secs(5)).await {
                            break;
                        }
                        continue;
                    }
                    unknown_creation = None;
                    let cancellation_warning = pending_cancellation_warning(&pending);
                    update_runtime(&state, &settings.account_id, |runtime| {
                        runtime.status = if cancellation_warning.is_some() {
                            CampaignStatus::Unknown
                        } else {
                            CampaignStatus::Running
                        };
                        runtime.pending_challenges = pending.len() as u32;
                        runtime.activity = "Outgoing challenges reconciled".into();
                        runtime.error = cancellation_warning;
                    })
                    .await;
                }
                Err(error) => {
                    let detail = lichess::actionable_missing_scope_message(&error).unwrap_or_else(
                        || {
                            format!(
                                "Challenge creation is paused until outgoing challenges can be reconciled: {error}"
                            )
                        },
                    );
                    update_runtime(&state, &settings.account_id, |runtime| {
                        runtime.status = CampaignStatus::Unknown;
                        runtime.pending_challenges = pending.len().saturating_add(1) as u32;
                        runtime.activity = "Reconciling an uncertain outgoing challenge…".into();
                        runtime.error = Some(detail.clone());
                        runtime.next_scan_at = None;
                    })
                    .await;
                    state.emit_snapshot().await;
                    if wait_or_cancel(&cancellation, Duration::from_secs(5)).await {
                        break;
                    }
                    continue;
                }
            }
        }
        let now = Instant::now();
        recent_opponents
            .retain(|_, challenged_at| now.duration_since(*challenged_at) < OPPONENT_COOLDOWN);
        let mut timed_out = Vec::new();
        {
            let active_games = state.0.active_games.lock().await;
            let mut all_outcomes = state.0.challenge_outcomes.lock().await;
            let outcomes = all_outcomes.entry(settings.account_id.clone()).or_default();
            let challenge_ids: Vec<_> = pending.keys().cloned().collect();
            for challenge_id in challenge_ids {
                let Some(challenge) = pending.get(&challenge_id) else {
                    continue;
                };
                let finished = outcomes.remove(&challenge_id);
                let started =
                    active_games.contains(&(settings.account_id.clone(), challenge_id.clone()));
                let expired = challenge.state == PendingState::Active
                    && now.duration_since(challenge.created_at) >= pending_lifetime;
                if expired {
                    timed_out.push((challenge_id.clone(), challenge.opponent.clone()));
                }
                if finished || started {
                    pending.remove(&challenge_id);
                } else if expired {
                    if let Some(challenge) = pending.get_mut(&challenge_id) {
                        challenge.state = PendingState::CancelPending(
                            "Cancellation has not been confirmed".into(),
                        );
                    }
                }
            }
            outcomes.retain(|challenge_id| pending.contains_key(challenge_id));
        }
        for (challenge_id, opponent) in timed_out {
            record_event(
                &state,
                &settings.account_id,
                "timeout",
                "Challenge timed out",
                Some(format!(
                    "{opponent} did not accept within {} seconds",
                    pending_lifetime.as_secs_f64()
                )),
            )
            .await;
            let cancel_result = {
                let _gate = state.0.matchmaking_api_gate.lock().await;
                lichess::cancel_challenge(
                    &state.0.api_base,
                    &state.0.api_client,
                    &token,
                    &challenge_id,
                )
                .await
            };
            match cancel_result {
                Ok(()) => {
                    pending.remove(&challenge_id);
                    state
                        .forget_known_outgoing_challenge(&settings.account_id, &challenge_id)
                        .await;
                }
                Err(error) => {
                    if let Some(challenge) = pending.get_mut(&challenge_id) {
                        challenge.state = PendingState::CancelPending(error.to_string());
                    }
                    let detail = format!("challenge {challenge_id} to {opponent}: {error}");
                    crate::diagnostics::record(
                        crate::diagnostics::DiagnosticEntry::warn(
                            "campaign",
                            "Could not confirm cancellation of an expired challenge",
                        )
                        .with_detail(detail.clone()),
                    );
                    update_runtime(&state, &settings.account_id, |runtime| {
                        runtime.status = CampaignStatus::Unknown;
                        runtime.error = Some(format!(
                            "Challenge {challenge_id} still occupies capacity because cancellation failed: {error}"
                        ));
                    })
                    .await;
                }
            }
        }

        if pending
            .values()
            .any(|challenge| matches!(challenge.state, PendingState::CancelPending(_)))
            && Instant::now() >= next_cancellation_reconciliation
        {
            let reconciliation = {
                let _gate = state.0.matchmaking_api_gate.lock().await;
                lichess::outgoing_challenges(&state.0.api_base, &state.0.api_client, &token).await
            };
            if let Ok(authoritative) = reconciliation {
                // A failed cancel never frees capacity by itself. Only this
                // authoritative set can prove the challenge absent.
                reconcile_pending_authoritatively(
                    &state,
                    &settings.account_id,
                    &mut pending,
                    authoritative,
                )
                .await;
            }
            next_cancellation_reconciliation = Instant::now() + Duration::from_secs(5);
        }

        let active_games = active_game_count(&state, &settings.account_id).await;
        let capacity = campaign_capacity(&state, &settings.account_id).await;
        let available_slots = available_admissions(&settings, capacity);
        let cancellation_warning = pending_cancellation_warning(&pending);

        update_runtime(&state, &settings.account_id, |runtime| {
            runtime.active_games = active_games;
            runtime.pending_challenges = pending.len() as u32;
            runtime.challenges_sent = challenges_sent;
            if let Some(warning) = cancellation_warning.clone() {
                runtime.status = CampaignStatus::Unknown;
                runtime.error = Some(warning);
                runtime.activity = format!(
                    "Cancellation unresolved; {} game(s), {} pending slot(s)",
                    active_games,
                    pending.len()
                );
            } else if available_slots == 0 {
                runtime.status = CampaignStatus::Running;
                runtime.next_scan_at = None;
                runtime.activity = if settings.stop_after_games.is_some_and(|limit| {
                    capacity
                        .games_completed
                        .saturating_add(u64::from(capacity.occupied_slots))
                        >= u64::from(limit)
                }) {
                    "Completion quota reserved; waiting for active or pending games to finish"
                        .into()
                } else {
                    format!(
                        "At capacity: {} game(s), {} pending",
                        active_games,
                        pending.len()
                    )
                };
            }
        })
        .await;
        state.emit_snapshot().await;

        if available_slots > 0 && now >= next_discovery {
            update_runtime(&state, &settings.account_id, |runtime| {
                runtime.status = CampaignStatus::Discovering;
                runtime.activity = "Waiting for the matchmaking API request slot…".into();
                runtime.next_scan_at = None;
            })
            .await;
            record_event(
                &state,
                &settings.account_id,
                "scan",
                "Discovery queued",
                Some(format!(
                    "Waiting for an API slot, then scanning established {perf} ratings from {} to {}",
                    settings.min_rating, settings.max_rating
                )),
            )
            .await;

            // Acquire the shared API gate outside the timeout so time spent
            // queued behind other operations does not count against the call.
            let discovery = {
                let _gate = state.0.matchmaking_api_gate.lock().await;
                update_runtime(&state, &settings.account_id, |runtime| {
                    runtime.activity =
                        "Request sent to Lichess; reading the online bot list…".into();
                })
                .await;
                record_event(
                    &state,
                    &settings.account_id,
                    "request",
                    "Lichess discovery request started",
                    Some("Downloading the current online-bot NDJSON stream".into()),
                )
                .await;
                lichess::online_bots(&state.0.api_base, &state.0.api_client).await
            };
            match discovery {
                Ok(bots) => {
                    let blocked = blocked_opponents(&state, &settings.account_id, &pending).await;
                    // Never challenge any locally configured account, not just this
                    // campaign's own: two managed bots playing each other corrupts state.
                    let local_accounts: HashSet<String> = state
                        .0
                        .config
                        .read()
                        .await
                        .accounts
                        .iter()
                        .flat_map(|account| {
                            [account.id.to_lowercase(), account.username.to_lowercase()]
                        })
                        .collect();
                    let (candidates, filter_stats) = filter_candidates(
                        bots,
                        &settings,
                        perf,
                        &local_accounts,
                        &blocked,
                        &recent_opponents,
                    );
                    let candidates = randomize_candidates(candidates);
                    let eligible_count = candidates.len() as u32;
                    update_runtime(&state, &settings.account_id, |runtime| {
                        runtime.eligible_bots = eligible_count;
                        runtime.online_bots_scanned = filter_stats.total;
                        if cancellation_warning.is_none() {
                            runtime.error = None;
                        }
                        if eligible_count == 0 {
                            runtime.status = CampaignStatus::Waiting;
                            runtime.activity = format!(
                                "No challengeable {perf} bots rated {}–{} are online",
                                settings.min_rating, settings.max_rating
                            );
                        } else {
                            runtime.status = CampaignStatus::Challenging;
                            runtime.activity =
                                format!("Found {eligible_count} eligible online bots");
                        }
                    })
                    .await;
                    record_event(
                        &state,
                        &settings.account_id,
                        if eligible_count == 0 { "idle" } else { "found" },
                        if eligible_count == 0 {
                            "No eligible opponents this scan"
                        } else {
                            "Eligible opponents found"
                        },
                        Some(format!(
                            "{} online · {} eligible · {} outside range · {} provisional/unplayed · {} busy/cooldown · {} without {perf} rating",
                            filter_stats.total,
                            eligible_count,
                            filter_stats.outside_range,
                            filter_stats.provisional_or_unplayed,
                            filter_stats.busy_or_cooling_down,
                            filter_stats.missing_pool,
                        )),
                    )
                    .await;

                    let maximum_attempts = (available_slots * 3).min(12) as usize;
                    let mut filled = 0u32;
                    for (bot, rating) in candidates.into_iter().take(maximum_attempts) {
                        if cancellation.is_cancelled() || filled >= available_slots {
                            break;
                        }
                        recent_opponents.insert(bot.id.to_lowercase(), Instant::now());
                        update_runtime(&state, &settings.account_id, |runtime| {
                            runtime.activity =
                                format!("Challenging {} ({rating} {perf})…", bot.username);
                            runtime.last_opponent = Some(bot.username.clone());
                        })
                        .await;
                        record_event(
                            &state,
                            &settings.account_id,
                            "attempt",
                            format!("Challenging {}", bot.username),
                            Some(format!(
                                "{rating} {perf} · waiting up to {} seconds",
                                pending_lifetime.as_secs_f64()
                            )),
                        )
                        .await;

                        let request = ChallengeRequest {
                            account_id: settings.account_id.clone(),
                            opponent: bot.username.clone(),
                            clock_limit: settings.clock_limit,
                            clock_increment: settings.clock_increment,
                            rated: settings.rated,
                            color: settings.color.clone(),
                            variant: "standard".into(),
                        };
                        let (external_unknown, persistence_error, result, clear_error) = {
                            let _ownership = match state.outgoing_challenge_admission().await {
                                Ok(admission) => admission,
                                Err(_) => break 'campaign,
                            };
                            let _gate = state.0.matchmaking_api_gate.lock().await;
                            if available_admissions(
                                &settings,
                                campaign_capacity(&state, &settings.account_id).await,
                            ) == 0
                            {
                                break;
                            }
                            let external_unknown = state
                                .0
                                .uncertain_challenge_creations
                                .lock()
                                .await
                                .get(&settings.account_id)
                                .cloned();
                            if external_unknown.is_some() {
                                (external_unknown, None, None, None)
                            } else if let Err(error) = state
                                .remember_uncertain_challenge_creation(
                                    &settings.account_id,
                                    &bot.username,
                                )
                                .await
                            {
                                (None, Some(error), None, None)
                            } else {
                                state
                                    .remember_pending_outgoing_challenge(
                                        &settings.account_id,
                                        &bot.username,
                                    )
                                    .await;
                                let result = lichess::create_challenge(
                                    &state.0.api_base,
                                    &state.0.api_client,
                                    &token,
                                    &request,
                                )
                                .await;
                                if let Ok(challenge) = result.as_ref() {
                                    state
                                        .finalize_pending_outgoing_challenge(
                                            &settings.account_id,
                                            &challenge.id,
                                            &bot.username,
                                        )
                                        .await;
                                }
                                let definitive = match result.as_ref() {
                                    Ok(_) => true,
                                    Err(error) => !error.ambiguous_write,
                                };
                                if matches!(result.as_ref(), Err(error) if !error.ambiguous_write) {
                                    state
                                        .forget_known_outgoing_challenge(&settings.account_id, "")
                                        .await;
                                }
                                let clear_error = if definitive {
                                    state
                                        .clear_uncertain_challenge_creation(&settings.account_id)
                                        .await
                                        .err()
                                } else {
                                    None
                                };
                                (None, None, Some(result), clear_error)
                            }
                        };
                        if let Some(opponent) = external_unknown {
                            unknown_creation = Some(opponent);
                            break;
                        }
                        if let Some(error) = persistence_error {
                            let detail = format!(
                                "Challenge creation was not sent because its durable safety barrier could not be saved: {error}"
                            );
                            unknown_creation = Some(bot.username.clone());
                            update_runtime(&state, &settings.account_id, |runtime| {
                                runtime.status = CampaignStatus::Unknown;
                                runtime.pending_challenges = pending.len().saturating_add(1) as u32;
                                runtime.activity =
                                    "Preserving the uncertain challenge safety barrier…".into();
                                runtime.error = Some(detail.clone());
                                runtime.next_scan_at = None;
                            })
                            .await;
                            record_event(
                                &state,
                                &settings.account_id,
                                "error",
                                "Challenge safety barrier could not be saved",
                                Some(detail),
                            )
                            .await;
                            state.emit_snapshot().await;
                            if wait_or_cancel(&cancellation, Duration::from_secs(5)).await {
                                break 'campaign;
                            }
                            continue 'campaign;
                        }
                        let Some(result) = result else {
                            break;
                        };
                        if let Some(error) = clear_error.as_ref() {
                            crate::diagnostics::record(
                                crate::diagnostics::DiagnosticEntry::error(
                                    "storage",
                                    "Could not clear a definitive challenge-creation barrier",
                                )
                                .with_account(&settings.account_id)
                                .with_detail(error.clone()),
                            );
                        }
                        match result {
                            Ok(challenge) => {
                                pending.insert(
                                    challenge.id,
                                    PendingChallenge {
                                        opponent: bot.username.clone(),
                                        created_at: Instant::now(),
                                        state: PendingState::Active,
                                    },
                                );
                                filled += 1;
                                challenges_sent += 1;
                                update_runtime(&state, &settings.account_id, |runtime| {
                                    runtime.pending_challenges = pending.len() as u32;
                                    runtime.challenges_sent = challenges_sent;
                                    if cancellation_warning.is_none() {
                                        runtime.error = None;
                                    }
                                    runtime.activity =
                                        format!("Challenge sent to {} ({rating})", bot.username);
                                })
                                .await;
                                record_event(
                                    &state,
                                    &settings.account_id,
                                    "sent",
                                    "Challenge sent",
                                    Some(format!(
                                        "{} ({rating}) now occupies one pending slot",
                                        bot.username
                                    )),
                                )
                                .await;
                            }
                            Err(error) => {
                                let rate_limited = error.is_rate_limited();
                                let retry_after =
                                    error.retry_after.unwrap_or(Duration::from_secs(60));
                                let retry_at =
                                    epoch_millis().saturating_add(retry_after.as_millis() as u64);
                                let ambiguous = error.ambiguous_write;
                                let actionable_scope_error =
                                    lichess::actionable_missing_scope_message(&error);
                                let displayed_error = actionable_scope_error
                                    .clone()
                                    .unwrap_or_else(|| error.to_string());
                                let error_text = format!(
                                    "{}{}",
                                    displayed_error,
                                    clear_error
                                        .as_ref()
                                        .map(|detail| format!(
                                            "; the definitive response was received, but the durable safety barrier could not be cleared: {detail}"
                                        ))
                                        .unwrap_or_default()
                                );
                                update_runtime(&state, &settings.account_id, |runtime| {
                                    runtime.error = Some(error_text.clone());
                                    runtime.status = if actionable_scope_error.is_some()
                                        || ambiguous
                                        || clear_error.is_some()
                                    {
                                        CampaignStatus::Unknown
                                    } else if rate_limited {
                                        CampaignStatus::Backoff
                                    } else {
                                        CampaignStatus::Challenging
                                    };
                                    runtime.activity = if actionable_scope_error.is_some() {
                                        "Matchmaking paused: the token is missing a required scope"
                                            .into()
                                    } else if clear_error.is_some() {
                                        "Challenge response received; pausing until the safety barrier can be reconciled".into()
                                    } else if ambiguous {
                                        "Challenge outcome unknown; pausing to reconcile outgoing challenges".into()
                                    } else if rate_limited {
                                        "Lichess rate limit: retry scheduled automatically".into()
                                    } else {
                                        format!("{} did not accept this challenge type; trying another bot", bot.username)
                                    };
                                    if rate_limited {
                                        runtime.next_scan_at = Some(retry_at);
                                    }
                                })
                                .await;
                                record_event(
                                    &state,
                                    &settings.account_id,
                                    if actionable_scope_error.is_some()
                                        || ambiguous
                                        || clear_error.is_some()
                                    {
                                        "unknown"
                                    } else if rate_limited {
                                        "backoff"
                                    } else {
                                        "rejected"
                                    },
                                    if actionable_scope_error.is_some() {
                                        "Token scope is missing"
                                    } else if clear_error.is_some() {
                                        "Challenge safety barrier could not be cleared"
                                    } else if ambiguous {
                                        "Challenge result is unknown"
                                    } else if rate_limited {
                                        "Lichess rate limit reached"
                                    } else {
                                        "Challenge could not be created"
                                    },
                                    Some(format!("{}: {error_text}", bot.username)),
                                )
                                .await;
                                if actionable_scope_error.is_some()
                                    || ambiguous
                                    || clear_error.is_some()
                                {
                                    unknown_creation = Some(bot.username.clone());
                                    break;
                                }
                                if rate_limited {
                                    next_discovery = Instant::now() + retry_after;
                                    break;
                                }
                            }
                        }
                        if clear_error.is_some() {
                            unknown_creation = Some(bot.username.clone());
                            break;
                        }
                        if wait_or_cancel(&cancellation, CHALLENGE_SPACING).await {
                            break;
                        }
                    }
                    if next_discovery <= now {
                        next_discovery = Instant::now() + DISCOVERY_INTERVAL;
                    }
                    update_runtime(&state, &settings.account_id, |runtime| {
                        if runtime.status != CampaignStatus::Backoff {
                            runtime.next_scan_at =
                                Some(epoch_millis() + DISCOVERY_INTERVAL.as_millis() as u64);
                        }
                    })
                    .await;
                }
                Err(error) => {
                    let rate_limited = error.is_rate_limited();
                    let retry_after = error.retry_after.unwrap_or(Duration::from_secs(60));
                    let error = error.to_string();
                    next_discovery = Instant::now()
                        + if rate_limited {
                            retry_after
                        } else {
                            DISCOVERY_INTERVAL
                        };
                    update_runtime(&state, &settings.account_id, |runtime| {
                        runtime.status = if rate_limited {
                            CampaignStatus::Backoff
                        } else {
                            CampaignStatus::Waiting
                        };
                        runtime.activity = if rate_limited {
                            "Lichess rate limit: retry scheduled automatically".into()
                        } else {
                            "Online bot discovery failed; retrying".into()
                        };
                        runtime.error = Some(error.clone());
                        runtime.next_scan_at = Some(
                            epoch_millis()
                                + if rate_limited {
                                    retry_after.as_millis() as u64
                                } else {
                                    DISCOVERY_INTERVAL.as_millis() as u64
                                },
                        );
                    })
                    .await;
                    record_event(
                        &state,
                        &settings.account_id,
                        if rate_limited { "backoff" } else { "error" },
                        if rate_limited {
                            "Discovery rate limited"
                        } else {
                            "Discovery failed"
                        },
                        Some(error),
                    )
                    .await;
                }
            }
            state.emit_snapshot().await;
        }

        if wait_or_cancel(&cancellation, Duration::from_secs(1)).await {
            break;
        }
    }

    let mut unresolved = Vec::new();
    if unknown_creation.is_some() {
        let reconciliation = {
            let _gate = state.0.matchmaking_api_gate.lock().await;
            lichess::outgoing_challenges(&state.0.api_base, &state.0.api_client, &token).await
        };
        match reconciliation {
            Ok(authoritative) => {
                reconcile_pending_authoritatively(
                    &state,
                    &settings.account_id,
                    &mut pending,
                    authoritative,
                )
                .await;
                match state
                    .clear_uncertain_challenge_creation(&settings.account_id)
                    .await
                {
                    Ok(()) => unknown_creation = None,
                    Err(error) => unresolved.push(format!(
                        "reconciled challenge-creation barrier could not be updated: {error}"
                    )),
                }
            }
            Err(error) => unresolved.push(format!(
                "unknown challenge creation could not be reconciled: {error}"
            )),
        }
    }
    let challenge_ids: Vec<_> = pending.keys().cloned().collect();
    for challenge_id in challenge_ids {
        let result = {
            let _gate = state.0.matchmaking_api_gate.lock().await;
            lichess::cancel_challenge(
                &state.0.api_base,
                &state.0.api_client,
                &token,
                &challenge_id,
            )
            .await
        };
        match result {
            Ok(()) => {
                pending.remove(&challenge_id);
                state
                    .forget_known_outgoing_challenge(&settings.account_id, &challenge_id)
                    .await;
            }
            Err(error) => {
                if let Some(challenge) = pending.get_mut(&challenge_id) {
                    challenge.state = PendingState::CancelPending(error.to_string());
                }
                unresolved.push(format!("{challenge_id}: {error}"));
            }
        }
    }
    if !unresolved.is_empty() {
        // A failed cancel remains capacity-consuming unless an authoritative
        // read proves the challenge no longer exists.
        if let Ok(authoritative) =
            lichess::outgoing_challenges(&state.0.api_base, &state.0.api_client, &token).await
        {
            reconcile_pending_authoritatively(
                &state,
                &settings.account_id,
                &mut pending,
                authoritative,
            )
            .await;
            unresolved.retain(|detail| {
                pending.keys().any(|id| detail.starts_with(id))
                    || detail.starts_with("unknown challenge")
                    || detail.starts_with("reconciled challenge-creation barrier")
            });
            if unknown_creation.is_some() {
                unresolved.retain(|detail| {
                    detail.starts_with("unknown challenge")
                        || detail.starts_with("reconciled challenge-creation barrier")
                });
            }
        }
    }
    let stop_error = (!unresolved.is_empty()).then(|| {
        format!(
            "Matchmaking stopped with unresolved outgoing challenges still occupying capacity: {}",
            unresolved.join("; ")
        )
    });
    let active_games = active_game_count(&state, &settings.account_id).await;
    update_runtime(&state, &settings.account_id, |runtime| {
        runtime.status = if stop_error.is_some() {
            CampaignStatus::Error
        } else {
            CampaignStatus::Stopped
        };
        runtime.active_games = active_games;
        runtime.pending_challenges = pending.len() as u32 + u32::from(unknown_creation.is_some());
        runtime.activity = if stop_error.is_some() {
            "Stopped, but challenge cancellation is unresolved".into()
        } else if let Some(reason) = automatic_stop_reason.as_ref() {
            format!("{reason}; active games will finish normally")
        } else {
            "Stopped; active games will finish normally".into()
        };
        runtime.error = stop_error.clone();
        runtime.next_scan_at = None;
        push_event(
            runtime,
            new_event(
                if stop_error.is_some() {
                    "error"
                } else {
                    "stop"
                },
                if stop_error.is_some() {
                    "Matchmaking stopped with unresolved challenges"
                } else if automatic_stop_reason.is_some() {
                    "Automatic run limit reached"
                } else {
                    "Matchmaking stopped"
                },
                stop_error.clone().or_else(|| {
                    Some(automatic_stop_reason.clone().unwrap_or_else(|| {
                        "Outstanding challenges were canceled; active games continue".into()
                    }))
                }),
            ),
        );
    })
    .await;
    state.emit_snapshot().await;
    if let Some(error) = stop_error {
        Err(error)
    } else {
        Ok(())
    }
}

async fn reconcile_pending_authoritatively(
    state: &AppState,
    account_id: &str,
    pending: &mut HashMap<String, PendingChallenge>,
    authoritative: Vec<lichess::OutgoingChallenge>,
) {
    state
        .reconcile_known_outgoing_challenges(account_id, &authoritative, &[])
        .await;
    reconcile_pending(pending, authoritative);
}

fn reconcile_pending(
    pending: &mut HashMap<String, PendingChallenge>,
    authoritative: Vec<lichess::OutgoingChallenge>,
) {
    let now = Instant::now();
    let mut reconciled = HashMap::new();
    for challenge in authoritative {
        let existing = pending.remove(&challenge.id);
        reconciled.insert(
            challenge.id,
            PendingChallenge {
                opponent: challenge.opponent,
                created_at: existing
                    .as_ref()
                    .map(|challenge| challenge.created_at)
                    .unwrap_or(now),
                state: existing
                    .map(|challenge| challenge.state)
                    .unwrap_or(PendingState::Active),
            },
        );
    }
    *pending = reconciled;
}

fn pending_cancellation_warning(pending: &HashMap<String, PendingChallenge>) -> Option<String> {
    pending.iter().find_map(|(challenge_id, challenge)| {
        if let PendingState::CancelPending(detail) = &challenge.state {
            Some(format!(
                "Challenge {challenge_id} still occupies capacity because cancellation is unconfirmed: {detail}"
            ))
        } else {
            None
        }
    })
}

fn campaign_limit_label(settings: &CampaignSettings) -> String {
    if let Some(minutes) = settings.stop_after_minutes {
        format!(
            "stop after {minutes} minute{}",
            if minutes == 1 { "" } else { "s" }
        )
    } else if let Some(games) = settings.stop_after_games {
        format!("complete {games} game{}", if games == 1 { "" } else { "s" })
    } else {
        "manual stop".into()
    }
}

async fn campaign_capacity(state: &AppState, account_id: &str) -> CampaignCapacity {
    let active_ids: HashSet<String> = state
        .0
        .active_games
        .lock()
        .await
        .iter()
        .filter(|(game_account, _)| game_account == account_id)
        .map(|(_, game_id)| game_id.clone())
        .collect();
    let known_ids: Vec<String> = state
        .0
        .known_outgoing_challenges
        .lock()
        .await
        .keys()
        .filter(|(challenge_account, _)| challenge_account == account_id)
        .map(|(_, challenge_id)| challenge_id.clone())
        .collect();
    let intent_ids: Vec<String> = state
        .0
        .active_intents
        .lock()
        .await
        .iter()
        .filter(|intent| intent.account_id == account_id)
        .map(|intent| intent.game_id.clone())
        .collect();

    let mut occupied = active_ids.clone();
    for challenge_id in known_ids {
        // The write-ahead outgoing reservation intentionally has no server id
        // yet. Give it a collision-free local key so it still consumes one
        // slot and one future-game allowance.
        let key = if challenge_id.is_empty() {
            "\0pending-outgoing".to_string()
        } else {
            challenge_id
        };
        occupied.insert(key);
    }
    for game_id in intent_ids {
        occupied.insert(game_id);
    }
    let games_completed = state
        .0
        .campaign_runtimes
        .read()
        .await
        .get(account_id)
        .map(|runtime| runtime.games_completed)
        .unwrap_or_default();
    CampaignCapacity {
        occupied_slots: occupied.len().min(u32::MAX as usize) as u32,
        games_completed,
    }
}

fn available_admissions(settings: &CampaignSettings, capacity: CampaignCapacity) -> u32 {
    let slots = settings.concurrency.saturating_sub(capacity.occupied_slots);
    let Some(game_limit) = settings.stop_after_games else {
        return slots;
    };
    // Every active game or not-yet-started challenge reserves one possible
    // completion. This prevents a concurrent campaign from overshooting its
    // target, while an aborted/no-start game releases the reservation and is
    // replaced on the next scan.
    let committed = capacity
        .games_completed
        .saturating_add(u64::from(capacity.occupied_slots));
    let game_allowance = u64::from(game_limit)
        .saturating_sub(committed)
        .min(u64::from(u32::MAX)) as u32;
    slots.min(game_allowance)
}

fn filter_candidates(
    bots: Vec<OnlineBot>,
    settings: &CampaignSettings,
    perf: &str,
    local_accounts: &HashSet<String>,
    blocked: &HashSet<String>,
    recent: &HashMap<String, Instant>,
) -> (Vec<(OnlineBot, i64)>, FilterStats) {
    let mut stats = FilterStats {
        total: bots.len() as u32,
        ..FilterStats::default()
    };
    let mut candidates = Vec::new();
    for bot in bots {
        let id = bot.id.to_lowercase();
        if id == settings.account_id.to_lowercase()
            || local_accounts.contains(&id)
            || local_accounts.contains(&bot.username.to_lowercase())
            || blocked.contains(&id)
            || recent.contains_key(&id)
        {
            stats.busy_or_cooling_down += 1;
            continue;
        }
        let Some((rating, games, provisional)) = bot.rating_for(perf) else {
            stats.missing_pool += 1;
            continue;
        };
        if games == 0 || provisional {
            stats.provisional_or_unplayed += 1;
            continue;
        }
        if rating < settings.min_rating || rating > settings.max_rating {
            stats.outside_range += 1;
            continue;
        }
        candidates.push((bot, rating));
    }
    (candidates, stats)
}

fn randomize_candidates(mut candidates: Vec<(OnlineBot, i64)>) -> Vec<(OnlineBot, i64)> {
    candidates.shuffle(&mut rand::rng());
    candidates
}

async fn blocked_opponents(
    state: &AppState,
    account_id: &str,
    pending: &HashMap<String, PendingChallenge>,
) -> HashSet<String> {
    let mut blocked: HashSet<_> = pending
        .values()
        .map(|challenge| challenge.opponent.to_lowercase())
        .collect();
    blocked.extend(
        state
            .0
            .games
            .read()
            .await
            .values()
            .filter(|game| {
                game.account_id == account_id
                    && (game.status == "started" || game.status == "created")
            })
            .map(|game| game.opponent.to_lowercase()),
    );
    blocked
}

async fn update_runtime(
    state: &AppState,
    account_id: &str,
    update: impl FnOnce(&mut CampaignRuntime),
) {
    let mut runtimes = state.0.campaign_runtimes.write().await;
    let runtime = runtimes
        .entry(account_id.to_string())
        .or_insert_with(|| CampaignRuntime::stopped(account_id.to_string()));
    update(runtime);
}

async fn active_game_count(state: &AppState, account_id: &str) -> u32 {
    state
        .0
        .active_games
        .lock()
        .await
        .iter()
        .filter(|(game_account_id, _)| game_account_id == account_id)
        .count() as u32
}

/// Applies the active campaign's rules to one account-stream challenge event.
/// The durable game intent is written before the accept POST, making an
/// ambiguous response capacity-consuming and restart-reconcilable just like a
/// gameStart that arrived during a runner handover.
pub(super) async fn handle_incoming_challenge(
    state: &AppState,
    account: &AccountProfile,
    token: &str,
    event: &Value,
) -> Result<(), String> {
    let challenger = event
        .pointer("/challenge/challenger/id")
        .or_else(|| event.pointer("/challenge/challenger/name"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if challenger.eq_ignore_ascii_case(&account.id)
        || challenger.eq_ignore_ascii_case(&account.username)
        || event
            .pointer("/challenge/direction")
            .and_then(Value::as_str)
            == Some("out")
    {
        // Lichess echoes our own outgoing challenge on this stream.
        return Ok(());
    }

    let Some((settings, cancellation, status)) = incoming_campaign(state, &account.id).await else {
        decline_incoming(state, &account.id, token, event, "generic", None).await;
        return Ok(());
    };
    if matches!(status, CampaignStatus::Backoff | CampaignStatus::Unknown) {
        // A rate-limit or ambiguous-write pause applies to every matchmaking
        // write. Leave the challenge untouched instead of violating it with an
        // accept or decline request; Lichess will expire it normally.
        return Ok(());
    }
    if !settings.accept_incoming_challenges {
        decline_incoming(state, &account.id, token, event, "generic", None).await;
        return Ok(());
    }

    let local_accounts: HashSet<String> = state
        .0
        .config
        .read()
        .await
        .accounts
        .iter()
        .flat_map(|configured| {
            [
                configured.id.to_lowercase(),
                configured.username.to_lowercase(),
            ]
        })
        .collect();
    let incoming = match validate_incoming_challenge(event, &settings, &local_accounts) {
        Ok(incoming) => incoming,
        Err(rejection) => {
            decline_incoming(
                state,
                &account.id,
                token,
                event,
                rejection.lichess_reason,
                Some(rejection.detail),
            )
            .await;
            return Ok(());
        }
    };

    let _ownership = match state.incoming_challenge_admission().await {
        Ok(admission) => admission,
        Err(_) => return Ok(()),
    };
    let _gate = state.0.matchmaking_api_gate.lock().await;
    let Some((current_settings, current_cancellation, current_status)) =
        incoming_campaign(state, &account.id).await
    else {
        drop(_gate);
        decline_incoming(
            state,
            &account.id,
            token,
            event,
            "later",
            Some("The matchmaking run stopped before this challenge could be accepted".into()),
        )
        .await;
        return Ok(());
    };
    if cancellation.is_cancelled()
        || current_cancellation.is_cancelled()
        || !current_settings.accept_incoming_challenges
        || !matches!(
            current_status,
            CampaignStatus::Starting
                | CampaignStatus::Discovering
                | CampaignStatus::Challenging
                | CampaignStatus::Running
                | CampaignStatus::Waiting
        )
        || current_settings
            .stop_after_minutes
            .zip(
                state
                    .0
                    .campaign_runtimes
                    .read()
                    .await
                    .get(&account.id)
                    .and_then(|runtime| runtime.stop_at),
            )
            .is_some_and(|(_, stop_at)| epoch_millis() >= stop_at)
        || available_admissions(
            &current_settings,
            campaign_capacity(state, &account.id).await,
        ) == 0
        || opponent_busy(state, &account.id, &incoming.opponent).await
    {
        drop(_gate);
        decline_incoming(
            state,
            &account.id,
            token,
            event,
            "later",
            Some("No campaign capacity or run allowance remained".into()),
        )
        .await;
        return Ok(());
    }

    state
        .add_active_intent(&account.id, &incoming.id)
        .await
        .map_err(|error| {
            format!(
                "Incoming challenge {} was not accepted because its durable game barrier could not be saved: {error}",
                incoming.id
            )
        })?;
    match lichess::accept_challenge(&state.0.api_base, &state.0.api_client, token, &incoming.id)
        .await
    {
        Ok(()) => {
            update_runtime(state, &account.id, |runtime| {
                runtime.activity =
                    format!("Accepted an incoming challenge from {}", incoming.opponent);
                runtime.last_opponent = Some(incoming.opponent.clone());
                push_event(
                    runtime,
                    new_event(
                        "accepted",
                        "Incoming challenge accepted",
                        Some(format!(
                            "{} now occupies one campaign slot",
                            incoming.opponent
                        )),
                    ),
                );
            })
            .await;
            state.emit_snapshot().await;
            Ok(())
        }
        Err(error) => {
            let ambiguous = error.ambiguous_write;
            let rate_limited = error.is_rate_limited();
            let retry_after = error.retry_after.unwrap_or(Duration::from_secs(60));
            let barrier_error = if ambiguous {
                None
            } else {
                state
                    .remove_active_intent(&account.id, &incoming.id)
                    .await
                    .err()
            };
            let detail = format!(
                "{}: {error}{}",
                incoming.opponent,
                barrier_error
                    .as_ref()
                    .map(|barrier| format!("; the definitive failure was received, but the durable game barrier could not be cleared: {barrier}"))
                    .unwrap_or_default()
            );
            update_runtime(state, &account.id, |runtime| {
                runtime.activity = if rate_limited {
                    "Lichess rate limit: incoming acceptance will retry automatically".into()
                } else if ambiguous || barrier_error.is_some() {
                    "Incoming challenge outcome is being reconciled".into()
                } else {
                    "Incoming challenge could not be accepted".into()
                };
                if rate_limited {
                    runtime.status = CampaignStatus::Backoff;
                    runtime.error = Some(detail.clone());
                    runtime.next_scan_at =
                        Some(epoch_millis().saturating_add(retry_after.as_millis() as u64));
                } else if ambiguous || barrier_error.is_some() {
                    runtime.status = CampaignStatus::Unknown;
                    runtime.error = Some(detail.clone());
                }
                push_event(
                    runtime,
                    new_event(
                        if rate_limited {
                            "backoff"
                        } else if ambiguous || barrier_error.is_some() {
                            "unknown"
                        } else {
                            "rejected"
                        },
                        if rate_limited {
                            "Incoming challenge acceptance was rate limited"
                        } else if ambiguous || barrier_error.is_some() {
                            "Incoming challenge result is unknown"
                        } else {
                            "Incoming challenge could not be accepted"
                        },
                        Some(detail.clone()),
                    ),
                );
            })
            .await;
            state.emit_snapshot().await;
            crate::diagnostics::record(
                crate::diagnostics::DiagnosticEntry::warn(
                    "lichess",
                    "Could not accept an incoming challenge",
                )
                .with_account(&account.id)
                .with_detail(detail.clone()),
            );
            if ambiguous || barrier_error.is_some() {
                Err(detail)
            } else {
                Ok(())
            }
        }
    }
}

async fn incoming_campaign(
    state: &AppState,
    account_id: &str,
) -> Option<(CampaignSettings, CancellationToken, CampaignStatus)> {
    let cancellation = state
        .0
        .campaign_tasks
        .lock()
        .await
        .get(account_id)?
        .cancellation
        .clone();
    if cancellation.is_cancelled() {
        return None;
    }
    let status = state
        .0
        .campaign_runtimes
        .read()
        .await
        .get(account_id)?
        .status;
    let settings = state
        .0
        .config
        .read()
        .await
        .campaigns
        .iter()
        .find(|campaign| campaign.account_id == account_id)?
        .clone();
    Some((settings, cancellation, status))
}

async fn campaign_backoff_until(state: &AppState, account_id: &str) -> Option<u64> {
    state
        .0
        .campaign_runtimes
        .read()
        .await
        .get(account_id)
        .filter(|runtime| runtime.status == CampaignStatus::Backoff)
        .and_then(|runtime| runtime.next_scan_at)
        .filter(|backoff_until| *backoff_until > epoch_millis())
}

fn validate_incoming_challenge(
    event: &Value,
    settings: &CampaignSettings,
    local_accounts: &HashSet<String>,
) -> Result<IncomingChallenge, IncomingRejection> {
    let challenge = event.get("challenge").ok_or_else(|| IncomingRejection {
        lichess_reason: "generic",
        detail: "Lichess omitted the incoming challenge details".into(),
    })?;
    let id = challenge
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| IncomingRejection {
            lichess_reason: "generic",
            detail: "Lichess omitted the incoming challenge id".into(),
        })?;
    let opponent = challenge
        .pointer("/challenger/name")
        .or_else(|| challenge.pointer("/challenger/id"))
        .and_then(Value::as_str)
        .filter(|opponent| !opponent.is_empty())
        .ok_or_else(|| IncomingRejection {
            lichess_reason: "generic",
            detail: "Lichess omitted the challenger's identity".into(),
        })?;
    if local_accounts.contains(&opponent.to_lowercase()) {
        return Err(IncomingRejection {
            lichess_reason: "generic",
            detail: format!("{opponent} is another QueenUI-managed account"),
        });
    }
    if event.pointer("/compat/bot").and_then(Value::as_bool) != Some(true) {
        return Err(IncomingRejection {
            lichess_reason: "noBot",
            detail: format!("{opponent}'s challenge is not compatible with the Bot API"),
        });
    }
    if challenge.pointer("/variant/key").and_then(Value::as_str) != Some("standard") {
        return Err(IncomingRejection {
            lichess_reason: "standard",
            detail: format!("{opponent} requested a non-standard variant"),
        });
    }
    if challenge
        .pointer("/timeControl/type")
        .and_then(Value::as_str)
        != Some("clock")
        || challenge
            .pointer("/timeControl/limit")
            .and_then(Value::as_u64)
            != Some(u64::from(settings.clock_limit))
        || challenge
            .pointer("/timeControl/increment")
            .and_then(Value::as_u64)
            != Some(u64::from(settings.clock_increment))
    {
        return Err(IncomingRejection {
            lichess_reason: "timeControl",
            detail: format!(
                "{opponent}'s challenge does not match {}+{}",
                settings.clock_limit / 60,
                settings.clock_increment
            ),
        });
    }
    if challenge.get("rated").and_then(Value::as_bool) != Some(settings.rated) {
        return Err(IncomingRejection {
            lichess_reason: if settings.rated { "rated" } else { "casual" },
            detail: format!(
                "{opponent}'s challenge does not match the {} campaign mode",
                if settings.rated { "rated" } else { "casual" }
            ),
        });
    }
    let rating = challenge
        .pointer("/challenger/rating")
        .and_then(Value::as_i64);
    let provisional = challenge
        .pointer("/challenger/provisional")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if provisional
        || rating.is_none_or(|rating| rating < settings.min_rating || rating > settings.max_rating)
    {
        return Err(IncomingRejection {
            lichess_reason: "generic",
            detail: format!(
                "{opponent} has no established rating inside {}–{}",
                settings.min_rating, settings.max_rating
            ),
        });
    }
    if settings.color != "random" {
        let bot_color = match challenge.get("finalColor").and_then(Value::as_str) {
            Some("white") => Some("black"),
            Some("black") => Some("white"),
            _ => None,
        };
        if bot_color != Some(settings.color.as_str()) {
            return Err(IncomingRejection {
                lichess_reason: "generic",
                detail: format!(
                    "{opponent}'s challenge does not assign the bot {}",
                    settings.color
                ),
            });
        }
    }
    Ok(IncomingChallenge {
        id: id.to_string(),
        opponent: opponent.to_string(),
    })
}

async fn opponent_busy(state: &AppState, account_id: &str, opponent: &str) -> bool {
    let playing = state.0.games.read().await.values().any(|game| {
        game.account_id == account_id
            && game.opponent.eq_ignore_ascii_case(opponent)
            && (game.status == "created" || game.status == "started")
    });
    playing
        || state.0.known_outgoing_challenges.lock().await.iter().any(
            |((challenge_account, _), known_opponent)| {
                challenge_account == account_id && known_opponent.eq_ignore_ascii_case(opponent)
            },
        )
}

async fn decline_incoming(
    state: &AppState,
    account_id: &str,
    token: &str,
    event: &Value,
    reason: &'static str,
    campaign_detail: Option<String>,
) {
    let Some(challenge_id) = event.pointer("/challenge/id").and_then(Value::as_str) else {
        return;
    };
    let result = {
        let _gate = state.0.matchmaking_api_gate.lock().await;
        lichess::decline_challenge(
            &state.0.api_base,
            &state.0.api_client,
            token,
            challenge_id,
            reason,
        )
        .await
    };
    if let Err(error) = result {
        crate::diagnostics::record(
            crate::diagnostics::DiagnosticEntry::warn(
                "lichess",
                "Could not decline an incoming challenge",
            )
            .with_account(account_id)
            .with_detail(format!("challenge {challenge_id}: {error}")),
        );
        if error.is_rate_limited() && state.0.campaign_tasks.lock().await.contains_key(account_id) {
            let retry_after = error.retry_after.unwrap_or(Duration::from_secs(60));
            let detail = format!("Incoming challenge decline was rate limited: {error}");
            update_runtime(state, account_id, |runtime| {
                runtime.status = CampaignStatus::Backoff;
                runtime.activity =
                    "Lichess rate limit: matchmaking will resume automatically".into();
                runtime.error = Some(detail.clone());
                runtime.next_scan_at =
                    Some(epoch_millis().saturating_add(retry_after.as_millis() as u64));
                push_event(
                    runtime,
                    new_event(
                        "backoff",
                        "Incoming challenge decline was rate limited",
                        Some(detail.clone()),
                    ),
                );
            })
            .await;
            state.emit_snapshot().await;
        }
        if let Some(detail) = campaign_detail {
            record_event(
                state,
                account_id,
                "error",
                "Incoming challenge could not be declined",
                Some(format!("{detail}; Lichess returned: {error}")),
            )
            .await;
        }
        return;
    }
    if let Some(detail) = campaign_detail {
        record_event(
            state,
            account_id,
            "rejected",
            "Incoming challenge declined",
            Some(detail),
        )
        .await;
    }
}

pub(super) async fn record_account_event(
    state: &AppState,
    account_id: &str,
    event_type: &str,
    event: &Value,
) {
    if !state.0.campaign_tasks.lock().await.contains_key(account_id) {
        return;
    }
    match event_type {
        "challengeDeclined" | "challengeCanceled" => {
            let Some(challenge_id) = event.pointer("/challenge/id").and_then(Value::as_str) else {
                return;
            };
            state
                .0
                .challenge_outcomes
                .lock()
                .await
                .entry(account_id.to_string())
                .or_default()
                .insert(challenge_id.to_string());
            let opponent = event
                .pointer("/challenge/destUser/name")
                .or_else(|| event.pointer("/challenge/destUser/id"))
                .and_then(Value::as_str)
                .unwrap_or("Opponent");
            let reason = event
                .pointer("/challenge/declineReason")
                .and_then(Value::as_str)
                .map(str::to_string);
            record_event(
                state,
                account_id,
                if event_type == "challengeDeclined" {
                    "declined"
                } else {
                    "canceled"
                },
                if event_type == "challengeDeclined" {
                    format!("{opponent} declined")
                } else {
                    format!("Challenge to {opponent} was canceled")
                },
                reason,
            )
            .await;
        }
        "gameStart" => {
            let game_id = event
                .pointer("/game/gameId")
                .or_else(|| event.pointer("/game/id"))
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let opponent = event
                .pointer("/game/opponent/username")
                .or_else(|| event.pointer("/game/opponent/id"))
                .and_then(Value::as_str);
            let first_for_campaign = state
                .0
                .campaign_tasks
                .lock()
                .await
                .get_mut(account_id)
                .is_some_and(|task| {
                    !task.settled_games.contains(game_id) && task.games.insert(game_id.to_string())
                });
            if !first_for_campaign {
                return;
            }
            update_runtime(state, account_id, |runtime| {
                runtime.games_started = runtime.games_started.saturating_add(1);
                runtime.activity = match opponent {
                    Some(opponent) => format!("Game started against {opponent}"),
                    None => "Game started".into(),
                };
                push_event(
                    runtime,
                    new_event(
                        "accepted",
                        "Challenge accepted — game started",
                        Some(match opponent {
                            Some(opponent) => format!("Playing {opponent} · game #{game_id}"),
                            None => format!("Game #{game_id}"),
                        }),
                    ),
                );
            })
            .await;
            state.emit_snapshot().await;
        }
        // The per-game stream remains authoritative, but current account
        // `gameFinish` events also carry a terminal status. Use it as an
        // idempotent fallback for a game stream that closes at the boundary.
        "gameFinish" => {
            let game_id = event
                .pointer("/game/gameId")
                .or_else(|| event.pointer("/game/id"))
                .and_then(Value::as_str);
            let status = event.pointer("/game/status").and_then(Value::as_str);
            if let (Some(game_id), Some(status)) = (game_id, status) {
                if !matches!(status, "created" | "started") {
                    record_game_completion(state, account_id, game_id, status).await;
                }
            }
        }
        _ => {}
    }
}

pub(super) async fn record_game_completion(
    state: &AppState,
    account_id: &str,
    game_id: &str,
    status: &str,
) {
    let tracked = state
        .0
        .campaign_tasks
        .lock()
        .await
        .get_mut(account_id)
        .is_some_and(|task| {
            if task.games.remove(game_id) {
                task.settled_games.insert(game_id.to_string());
                true
            } else {
                false
            }
        });
    if !tracked {
        return;
    }

    let counts_toward_limit = counts_toward_completed_game_limit(status);
    update_runtime(state, account_id, |runtime| {
        if counts_toward_limit {
            runtime.games_completed = runtime.games_completed.saturating_add(1);
            runtime.activity = format!("Completed game #{}", runtime.games_completed);
            push_event(
                runtime,
                new_event(
                    "finished",
                    "Completed game counted",
                    Some(format!(
                        "Game #{game_id} · {} completed this run · {status}",
                        runtime.games_completed
                    )),
                ),
            );
        } else {
            runtime.activity =
                "Aborted game did not count; matchmaking will refill the slot".into();
            push_event(
                runtime,
                new_event(
                    "aborted",
                    "Aborted game not counted",
                    Some(format!("Game #{game_id} · {status}")),
                ),
            );
        }
    })
    .await;
    state.emit_snapshot().await;
}

fn counts_toward_completed_game_limit(status: &str) -> bool {
    !matches!(status, "aborted" | "noStart")
}

async fn record_event(
    state: &AppState,
    account_id: &str,
    kind: &str,
    title: impl Into<String>,
    detail: Option<String>,
) {
    update_runtime(state, account_id, |runtime| {
        push_event(runtime, new_event(kind, title, detail));
    })
    .await;
    state.emit_snapshot().await;
}

fn push_event(runtime: &mut CampaignRuntime, event: CampaignEvent) {
    runtime.events.push(event);
    if runtime.events.len() > MAX_ACTIVITY_EVENTS {
        runtime
            .events
            .drain(..runtime.events.len() - MAX_ACTIVITY_EVENTS);
    }
}

fn new_event(kind: &str, title: impl Into<String>, detail: Option<String>) -> CampaignEvent {
    CampaignEvent {
        id: Uuid::new_v4().to_string(),
        timestamp: epoch_millis(),
        kind: kind.to_string(),
        title: title.into(),
        detail,
    }
}

fn epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

async fn account_connected(state: &AppState, account_id: &str) -> bool {
    state
        .0
        .runtimes
        .read()
        .await
        .get(account_id)
        .map(|runtime| runtime.status == "online" || runtime.status == "playing")
        .unwrap_or(false)
}

async fn wait_or_cancel(cancellation: &CancellationToken, duration: Duration) -> bool {
    tokio::select! {
        _ = cancellation.cancelled() => true,
        _ = tokio::time::sleep(duration) => false,
    }
}

fn validate(settings: &CampaignSettings) -> Result<(), String> {
    if settings.min_rating < 0
        || settings.max_rating > 5000
        || settings.min_rating > settings.max_rating
    {
        return Err("Choose a valid rating range between 0 and 5000.".into());
    }
    if settings.concurrency == 0 || settings.concurrency > MAX_CONCURRENCY {
        return Err(format!(
            "Challenge concurrency must be between 1 and {MAX_CONCURRENCY}."
        ));
    }
    validate_clock(settings.clock_limit, settings.clock_increment)?;
    if !matches!(settings.color.as_str(), "white" | "black" | "random") {
        return Err("Challenge color must be white, black, or random.".into());
    }
    if settings.stop_after_minutes.is_some() && settings.stop_after_games.is_some() {
        return Err("Choose either a time limit or a game limit, not both.".into());
    }
    if settings
        .stop_after_minutes
        .is_some_and(|minutes| !(1..=MAX_RUN_MINUTES).contains(&minutes))
    {
        return Err(format!(
            "Campaign duration must be between 1 and {MAX_RUN_MINUTES} minutes."
        ));
    }
    if settings
        .stop_after_games
        .is_some_and(|games| !(1..=MAX_RUN_GAMES).contains(&games))
    {
        return Err(format!(
            "Campaign game limit must be between 1 and {MAX_RUN_GAMES}."
        ));
    }
    Ok(())
}

pub(super) fn validate_clock(limit: u32, increment: u32) -> Result<(), String> {
    let valid_limit = matches!(limit, 15 | 30 | 45 | 60 | 90)
        || ((120..=10_800).contains(&limit) && limit.is_multiple_of(60));
    if !valid_limit || increment > 60 {
        return Err("Choose a Lichess-supported real-time clock.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        available_admissions, campaign_capacity, filter_candidates, randomize_candidates,
        reconcile_pending, reconcile_pending_authoritatively, record_account_event,
        record_game_completion, run_with_pending_lifetime, validate, validate_incoming_challenge,
        CampaignCapacity, CampaignTask, PendingChallenge, PendingState,
    };
    use crate::lichess::OutgoingChallenge;
    use crate::models::{CampaignRuntime, CampaignSettings, CampaignStatus, OnlineBot};
    use crate::test_support::{
        app_config, temp_root, MemorySecretStore, ScriptReply, ScriptedHttp,
    };
    use crate::{storage, AppState};
    use std::collections::{HashMap, HashSet};
    use std::fs;
    use std::sync::Arc;
    use std::time::Instant;
    use tokio_util::sync::CancellationToken;

    #[test]
    fn authoritative_outgoing_challenges_clear_absent_and_preserve_unresolved_capacity() {
        let mut pending = HashMap::from([
            (
                "still-out".into(),
                PendingChallenge {
                    opponent: "Opponent".into(),
                    created_at: Instant::now(),
                    state: PendingState::CancelPending("timeout".into()),
                },
            ),
            (
                "gone".into(),
                PendingChallenge {
                    opponent: "Other".into(),
                    created_at: Instant::now(),
                    state: PendingState::CancelPending("timeout".into()),
                },
            ),
        ]);
        reconcile_pending(
            &mut pending,
            vec![OutgoingChallenge {
                id: "still-out".into(),
                opponent: "Opponent".into(),
                status: "created".into(),
            }],
        );
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending["still-out"].state,
            PendingState::CancelPending("timeout".into())
        );
    }

    #[tokio::test]
    async fn campaign_authoritative_reconciliation_prunes_known_tombstones() {
        let root = temp_root("campaign-known-prune");
        let state = AppState::new_with_secret_store(
            root.clone(),
            app_config("unused-engine", false),
            Arc::new(MemorySecretStore::with("bot", "token")),
        )
        .unwrap();
        state
            .remember_known_outgoing_challenge("bot", "gone", "Opponent")
            .await;
        let mut pending = HashMap::new();

        reconcile_pending_authoritatively(&state, "bot", &mut pending, Vec::new()).await;

        assert!(state.0.known_outgoing_challenges.lock().await.is_empty());
        drop(state);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn filters_candidates_by_established_rating_and_campaign_limits() {
        let settings = CampaignSettings {
            account_id: "queenbot".into(),
            min_rating: 2000,
            max_rating: 2400,
            concurrency: 3,
            clock_limit: 180,
            clock_increment: 2,
            rated: false,
            color: "random".into(),
            accept_incoming_challenges: false,
            stop_after_minutes: None,
            stop_after_games: None,
        };
        assert!(validate(&settings).is_ok());
        let candidate = OnlineBot {
            id: "opponent".into(),
            username: "Opponent".into(),
            perfs: serde_json::json!({
                "blitz": { "rating": 2200, "games": 100, "prov": false }
            }),
        };
        let (eligible, _) = filter_candidates(
            vec![candidate.clone()],
            &settings,
            "blitz",
            &HashSet::new(),
            &HashSet::new(),
            &HashMap::new(),
        );
        assert_eq!(eligible.len(), 1);

        let provisional = OnlineBot {
            id: "newbot".into(),
            username: "NewBot".into(),
            perfs: serde_json::json!({
                "blitz": { "rating": 2200, "games": 3, "prov": true }
            }),
        };
        let (eligible, stats) = filter_candidates(
            vec![provisional],
            &settings,
            "blitz",
            &HashSet::new(),
            &HashSet::new(),
            &HashMap::new(),
        );
        assert!(eligible.is_empty());
        assert_eq!(stats.provisional_or_unplayed, 1);

        // Any locally configured account (id or username) is never challenged,
        // even when it would otherwise be an eligible opponent.
        let local_accounts: HashSet<String> = ["opponent".to_string()].into_iter().collect();
        let (eligible, stats) = filter_candidates(
            vec![candidate],
            &settings,
            "blitz",
            &local_accounts,
            &HashSet::new(),
            &HashMap::new(),
        );
        assert!(eligible.is_empty());
        assert_eq!(stats.busy_or_cooling_down, 1);
    }

    #[test]
    fn incoming_challenges_must_match_every_campaign_rule() {
        let mut settings = settings();
        settings.accept_incoming_challenges = true;
        settings.color = "black".into();
        let local_accounts = HashSet::from(["other-managed-bot".into()]);
        let matching = serde_json::json!({
            "type": "challenge",
            "compat": { "bot": true },
            "challenge": {
                "id": "incoming-1",
                "direction": "in",
                "challenger": {
                    "id": "opponent",
                    "name": "Opponent",
                    "rating": 2200,
                    "provisional": false
                },
                "variant": { "key": "standard" },
                "rated": false,
                "timeControl": { "type": "clock", "limit": 180, "increment": 2 },
                "finalColor": "white"
            }
        });

        let accepted = validate_incoming_challenge(&matching, &settings, &local_accounts).unwrap();
        assert_eq!(accepted.id, "incoming-1");
        assert_eq!(accepted.opponent, "Opponent");

        let mut wrong_clock = matching.clone();
        wrong_clock["challenge"]["timeControl"]["increment"] = serde_json::json!(3);
        assert_eq!(
            validate_incoming_challenge(&wrong_clock, &settings, &local_accounts)
                .unwrap_err()
                .lichess_reason,
            "timeControl"
        );

        let mut managed = matching;
        managed["challenge"]["challenger"]["name"] = serde_json::json!("other-managed-bot");
        assert!(validate_incoming_challenge(&managed, &settings, &local_accounts).is_err());
    }

    #[tokio::test]
    async fn accepting_incoming_challenge_persists_ownership_and_uses_capacity() {
        let http = ScriptedHttp::start().await;
        http.push(
            "POST",
            "/api/challenge/incoming-1/accept",
            ScriptReply::Json(axum::http::StatusCode::OK, r#"{"ok":true}"#.into()),
        );
        let root = temp_root("campaign-accept-incoming");
        let mut config = app_config("unused-engine", false);
        let mut campaign_settings = settings();
        campaign_settings.accept_incoming_challenges = true;
        config.campaigns.push(campaign_settings);
        let account = config.accounts[0].clone();
        let state = AppState::new_with_test_api(
            root.clone(),
            config,
            Arc::new(MemorySecretStore::with("bot", "token")),
            http.base(),
        )
        .unwrap();
        state.0.campaign_tasks.lock().await.insert(
            "bot".into(),
            CampaignTask {
                generation: 1,
                cancellation: CancellationToken::new(),
                handle: None,
                games: HashSet::new(),
                settled_games: HashSet::new(),
            },
        );
        let mut runtime = CampaignRuntime::stopped("bot".into());
        runtime.status = CampaignStatus::Running;
        state
            .0
            .campaign_runtimes
            .write()
            .await
            .insert("bot".into(), runtime);
        let event = serde_json::json!({
            "type": "challenge",
            "compat": { "bot": true },
            "challenge": {
                "id": "incoming-1",
                "direction": "in",
                "challenger": {
                    "id": "opponent",
                    "name": "Opponent",
                    "rating": 2200,
                    "provisional": false
                },
                "variant": { "key": "standard" },
                "rated": false,
                "timeControl": { "type": "clock", "limit": 180, "increment": 2 },
                "finalColor": "white"
            }
        });

        super::handle_incoming_challenge(&state, &account, "token", &event)
            .await
            .unwrap();

        http.wait_for_count("POST", "/api/challenge/incoming-1/accept", 1)
            .await;
        let intents =
            storage::load_active_game_intents(&storage::active_game_intents_path(&root)).unwrap();
        assert_eq!(intents.len(), 1);
        assert_eq!(intents[0].account_id, "bot");
        assert_eq!(intents[0].game_id, "incoming-1");
        assert_eq!(campaign_capacity(&state, "bot").await.occupied_slots, 1);

        drop(state);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn incoming_accept_rate_limit_pauses_campaign_until_lichess_reset() {
        let http = ScriptedHttp::start().await;
        http.push(
            "POST",
            "/api/challenge/incoming-1/accept",
            ScriptReply::Json(
                axum::http::StatusCode::TOO_MANY_REQUESTS,
                r#"{"error":"You played 100 games against other bots today","ratelimit":{"key":"bot.vsBot.day","seconds":7325}}"#.into(),
            ),
        );
        let root = temp_root("campaign-incoming-rate-limit");
        let mut config = app_config("unused-engine", false);
        let mut campaign_settings = settings();
        campaign_settings.accept_incoming_challenges = true;
        config.campaigns.push(campaign_settings);
        let account = config.accounts[0].clone();
        let state = AppState::new_with_test_api(
            root.clone(),
            config,
            Arc::new(MemorySecretStore::with("bot", "token")),
            http.base(),
        )
        .unwrap();
        state.0.campaign_tasks.lock().await.insert(
            "bot".into(),
            CampaignTask {
                generation: 1,
                cancellation: CancellationToken::new(),
                handle: None,
                games: HashSet::new(),
                settled_games: HashSet::new(),
            },
        );
        let mut runtime = CampaignRuntime::stopped("bot".into());
        runtime.status = CampaignStatus::Running;
        state
            .0
            .campaign_runtimes
            .write()
            .await
            .insert("bot".into(), runtime);
        let event = serde_json::json!({
            "type": "challenge",
            "compat": { "bot": true },
            "challenge": {
                "id": "incoming-1",
                "direction": "in",
                "challenger": {
                    "id": "opponent",
                    "name": "Opponent",
                    "rating": 2200,
                    "provisional": false
                },
                "variant": { "key": "standard" },
                "rated": false,
                "timeControl": { "type": "clock", "limit": 180, "increment": 2 },
                "finalColor": "white"
            }
        });
        let before = super::epoch_millis();

        super::handle_incoming_challenge(&state, &account, "token", &event)
            .await
            .unwrap();

        let runtimes = state.0.campaign_runtimes.read().await;
        let runtime = &runtimes["bot"];
        assert_eq!(runtime.status, CampaignStatus::Backoff);
        assert!(runtime
            .next_scan_at
            .is_some_and(|reset| reset >= before + 7_325_000));
        drop(runtimes);
        assert!(state.0.active_intents.lock().await.is_empty());

        drop(state);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn completed_game_limit_reserves_active_and_pending_games_before_admitting_more() {
        let mut settings = settings();
        settings.concurrency = 8;
        settings.stop_after_games = Some(3);

        assert_eq!(
            available_admissions(
                &settings,
                CampaignCapacity {
                    occupied_slots: 2,
                    games_completed: 1,
                },
            ),
            0
        );
        assert_eq!(
            available_admissions(
                &settings,
                CampaignCapacity {
                    occupied_slots: 1,
                    games_completed: 1,
                },
            ),
            1
        );
    }

    #[tokio::test]
    async fn campaign_counts_terminal_games_once_and_retries_aborted_games() {
        let root = temp_root("campaign-completed-game-quota");
        let state = AppState::new_with_secret_store(
            root.clone(),
            app_config("unused-engine", false),
            Arc::new(MemorySecretStore::with("bot", "token")),
        )
        .unwrap();
        state.0.campaign_tasks.lock().await.insert(
            "bot".into(),
            CampaignTask {
                generation: 1,
                cancellation: CancellationToken::new(),
                handle: None,
                games: HashSet::new(),
                settled_games: HashSet::new(),
            },
        );
        let mut runtime = CampaignRuntime::stopped("bot".into());
        runtime.status = CampaignStatus::Running;
        state
            .0
            .campaign_runtimes
            .write()
            .await
            .insert("bot".into(), runtime);

        let completed_start = serde_json::json!({
            "type": "gameStart",
            "game": { "gameId": "completed", "opponent": { "username": "MateBot" } }
        });
        record_account_event(&state, "bot", "gameStart", &completed_start).await;
        record_game_completion(&state, "bot", "completed", "mate").await;
        record_game_completion(&state, "bot", "completed", "mate").await;

        let aborted_start = serde_json::json!({
            "type": "gameStart",
            "game": { "gameId": "aborted", "opponent": { "username": "AbortBot" } }
        });
        record_account_event(&state, "bot", "gameStart", &aborted_start).await;
        record_game_completion(&state, "bot", "aborted", "aborted").await;

        let fallback_start = serde_json::json!({
            "type": "gameStart",
            "game": { "gameId": "fallback", "opponent": { "username": "StreamCloseBot" } }
        });
        record_account_event(&state, "bot", "gameStart", &fallback_start).await;
        record_account_event(&state, "bot", "gameStart", &fallback_start).await;
        let fallback_finish = serde_json::json!({
            "type": "gameFinish",
            "game": { "gameId": "fallback", "status": "resign" }
        });
        record_account_event(&state, "bot", "gameFinish", &fallback_finish).await;
        record_account_event(&state, "bot", "gameFinish", &fallback_finish).await;

        let no_start = serde_json::json!({
            "type": "gameStart",
            "game": { "gameId": "no-start", "opponent": { "username": "NoStartBot" } }
        });
        record_account_event(&state, "bot", "gameStart", &no_start).await;
        let no_start_finish = serde_json::json!({
            "type": "gameFinish",
            "game": { "gameId": "no-start", "status": "noStart" }
        });
        record_account_event(&state, "bot", "gameFinish", &no_start_finish).await;

        let runtime = state.0.campaign_runtimes.read().await["bot"].clone();
        assert_eq!(runtime.games_started, 4);
        assert_eq!(runtime.games_completed, 2);
        assert!(runtime
            .events
            .iter()
            .any(|event| event.title == "Completed game counted"));
        assert!(runtime
            .events
            .iter()
            .any(|event| event.title == "Aborted game not counted"));
        let tasks = state.0.campaign_tasks.lock().await;
        assert!(tasks["bot"].games.is_empty());
        assert_eq!(tasks["bot"].settled_games.len(), 4);
        drop(tasks);

        drop(state);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn campaign_accepts_only_one_kind_of_automatic_limit() {
        let mut limited = settings();
        limited.stop_after_minutes = Some(30);
        assert!(validate(&limited).is_ok());
        limited.stop_after_games = Some(10);
        assert!(validate(&limited).is_err());
        limited.stop_after_minutes = None;
        assert!(validate(&limited).is_ok());
        limited.stop_after_games = Some(0);
        assert!(validate(&limited).is_err());
    }

    #[test]
    fn randomizes_with_one_stable_key_per_candidate() {
        let candidates: Vec<_> = (0..512)
            .map(|index| {
                (
                    OnlineBot {
                        id: format!("bot-{index}"),
                        username: format!("Bot {index}"),
                        perfs: serde_json::Value::Null,
                    },
                    2000 + index,
                )
            })
            .collect();
        let randomized = randomize_candidates(candidates);
        assert_eq!(randomized.len(), 512);
        let ids: HashSet<_> = randomized.into_iter().map(|(bot, _)| bot.id).collect();
        assert_eq!(ids.len(), 512);
    }

    fn settings() -> CampaignSettings {
        CampaignSettings {
            account_id: "bot".into(),
            min_rating: 2000,
            max_rating: 2400,
            concurrency: 1,
            clock_limit: 180,
            clock_increment: 2,
            rated: false,
            color: "random".into(),
            accept_incoming_challenges: false,
            stop_after_minutes: None,
            stop_after_games: None,
        }
    }

    fn outgoing(id: &str, opponent: &str) -> String {
        format!(
            r#"{{"out":[{{"id":"{id}","status":"created","destUser":{{"id":"{opponent}"}}}}]}}"#
        )
    }

    #[tokio::test]
    async fn campaign_reconciliation_surfaces_missing_scope_remedy() {
        let http = ScriptedHttp::start().await;
        for _ in 0..2 {
            http.push(
                "GET",
                "/api/challenge",
                ScriptReply::Json(
                    axum::http::StatusCode::FORBIDDEN,
                    r#"{"error":"Missing scope: challenge:read"}"#.into(),
                ),
            );
        }
        let root = temp_root("campaign-missing-scope");
        let state = AppState::new_with_test_api(
            root.clone(),
            app_config("unused-engine", false),
            Arc::new(MemorySecretStore::with("bot", "token")),
            http.base(),
        )
        .unwrap();
        let cancellation = CancellationToken::new();
        let task_state = state.clone();
        let task_cancellation = cancellation.clone();
        let campaign = tokio::spawn(async move {
            run_with_pending_lifetime(
                task_state,
                settings(),
                "token".into(),
                task_cancellation,
                std::time::Duration::from_secs(60),
            )
            .await
        });

        http.wait_for_count("GET", "/api/challenge", 1).await;
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                let error = state
                    .0
                    .campaign_runtimes
                    .read()
                    .await
                    .get("bot")
                    .and_then(|runtime| runtime.error.clone());
                if error.is_some() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            state.0.campaign_runtimes.read().await["bot"]
                .error
                .as_deref(),
            Some("Matchmaking is paused because this Lichess token is missing scope challenge:read; create a new token at lichess.org/account/oauth/token/create with Play-bot, Read-challenges, and Send-challenges ticked—games continue with the current token.")
        );

        cancellation.cancel();
        assert!(campaign.await.unwrap().is_err());
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn campaign_stop_records_the_current_active_game_count() {
        let http = ScriptedHttp::start().await;
        let root = temp_root("campaign-stop-active-games");
        let state = AppState::new_with_test_api(
            root.clone(),
            app_config("unused-engine", false),
            Arc::new(MemorySecretStore::with("bot", "token")),
            http.base(),
        )
        .unwrap();
        let mut runtime = CampaignRuntime::stopped("bot".into());
        runtime.status = CampaignStatus::Running;
        runtime.active_games = 7;
        runtime.pending_challenges = 2;
        state
            .0
            .campaign_runtimes
            .write()
            .await
            .insert("bot".into(), runtime);
        state.0.active_games.lock().await.extend([
            ("bot".into(), "live-one".into()),
            ("bot".into(), "live-two".into()),
            ("other-bot".into(), "other-game".into()),
        ]);

        crate::stop_campaign("bot".into(), crate::CoreStateRef::new(&state))
            .await
            .unwrap();

        let runtime = state.0.campaign_runtimes.read().await["bot"].clone();
        assert_eq!(runtime.status, CampaignStatus::Stopped);
        assert_eq!(runtime.active_games, 2);
        assert_eq!(runtime.pending_challenges, 0);

        drop(state);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn campaign_self_ambiguity_inserts_barrier_and_breaks_the_candidate_loop() {
        let http = ScriptedHttp::start().await;
        http.push(
            "GET",
            "/api/challenge",
            ScriptReply::Json(axum::http::StatusCode::OK, r#"{"out":[]}"#.into()),
        );
        http.push(
            "GET",
            "/api/bot/online",
            ScriptReply::Json(
                axum::http::StatusCode::OK,
                concat!(
                    r#"{"id":"opponent-one","username":"OpponentOne","perfs":{"blitz":{"rating":2200,"games":100,"prov":false}}}"#,
                    "\n",
                    r#"{"id":"opponent-two","username":"OpponentTwo","perfs":{"blitz":{"rating":2200,"games":100,"prov":false}}}"#,
                    "\n"
                )
                .into(),
            ),
        );
        for opponent in ["OpponentOne", "OpponentTwo"] {
            http.push(
                "POST",
                &format!("/api/challenge/{opponent}"),
                ScriptReply::BodyError(axum::http::StatusCode::OK),
            );
        }
        http.push(
            "GET",
            "/api/challenge",
            ScriptReply::Json(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                r#"{"error":"reconciliation unavailable"}"#.into(),
            ),
        );
        let root = temp_root("campaign-self-ambiguity");
        let state = AppState::new_with_test_api(
            root.clone(),
            app_config("unused-engine", false),
            Arc::new(MemorySecretStore::with("bot", "token")),
            http.base(),
        )
        .unwrap();
        let cancellation = CancellationToken::new();
        let task_state = state.clone();
        let task_cancellation = cancellation.clone();
        let campaign = tokio::spawn(async move {
            run_with_pending_lifetime(
                task_state,
                settings(),
                "token".into(),
                task_cancellation,
                std::time::Duration::from_secs(60),
            )
            .await
        });

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let create_posts = http.count("POST", "/api/challenge/OpponentOne")
                    + http.count("POST", "/api/challenge/OpponentTwo");
                if create_posts == 1 && http.count("GET", "/api/challenge") >= 2 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("campaign did not reach authoritative reconciliation");
        assert!(state
            .0
            .uncertain_challenge_creations
            .lock()
            .await
            .contains_key("bot"));
        assert_eq!(
            http.count("POST", "/api/challenge/OpponentOne")
                + http.count("POST", "/api/challenge/OpponentTwo"),
            1,
            "an ambiguous campaign create must block every later create POST"
        );
        let runtime = state.0.campaign_runtimes.read().await["bot"].clone();
        assert_eq!(
            runtime
                .events
                .iter()
                .filter(|event| event.kind == "attempt")
                .count(),
            1,
            "the candidate loop must break immediately after its own ambiguous POST"
        );

        cancellation.cancel();
        assert!(campaign.await.unwrap().is_err());
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn campaign_barrier_persist_failure_blocks_create_post_and_surfaces_error() {
        let http = ScriptedHttp::start().await;
        for _ in 0..3 {
            http.push(
                "GET",
                "/api/challenge",
                ScriptReply::Json(axum::http::StatusCode::OK, r#"{"out":[]}"#.into()),
            );
        }
        http.push(
            "GET",
            "/api/bot/online",
            ScriptReply::Json(
                axum::http::StatusCode::OK,
                r#"{"id":"opponent","username":"Opponent","perfs":{"blitz":{"rating":2200,"games":100,"prov":false}}}"#.into(),
            ),
        );
        let root = temp_root("campaign-write-ahead-failure");
        let state = AppState::new_with_test_api(
            root.clone(),
            app_config("unused-engine", false),
            Arc::new(MemorySecretStore::with("bot", "token")),
            http.base(),
        )
        .unwrap();
        fs::create_dir(storage::uncertain_challenge_creations_path(&root)).unwrap();
        let cancellation = CancellationToken::new();
        let task_state = state.clone();
        let task_cancellation = cancellation.clone();
        let campaign = tokio::spawn(async move {
            run_with_pending_lifetime(
                task_state,
                settings(),
                "token".into(),
                task_cancellation,
                std::time::Duration::from_secs(60),
            )
            .await
        });

        let runtime = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let Some(runtime) = state
                    .0
                    .campaign_runtimes
                    .read()
                    .await
                    .get("bot")
                    .filter(|runtime| {
                        runtime
                            .error
                            .as_deref()
                            .is_some_and(|detail| detail.contains("durable safety barrier"))
                    })
                    .cloned()
                {
                    break runtime;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("campaign did not surface the barrier persistence failure");
        assert_eq!(runtime.status, CampaignStatus::Unknown);
        assert_eq!(http.count("POST", "/api/challenge/Opponent"), 0);

        cancellation.cancel();
        assert!(campaign.await.unwrap().is_err());
        drop(state);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn campaign_stop_surfaces_reconciled_barrier_clear_failure() {
        let http = ScriptedHttp::start().await;
        for _ in 0..3 {
            http.push(
                "GET",
                "/api/challenge",
                ScriptReply::Json(axum::http::StatusCode::OK, r#"{"out":[]}"#.into()),
            );
        }
        let root = temp_root("campaign-stop-barrier-clear");
        let state = AppState::new_with_test_api(
            root.clone(),
            app_config("unused-engine", false),
            Arc::new(MemorySecretStore::with("bot", "token")),
            http.base(),
        )
        .unwrap();
        state
            .remember_uncertain_challenge_creation("bot", "Opponent")
            .await
            .unwrap();
        let barrier_path = storage::uncertain_challenge_creations_path(&root);
        fs::remove_file(&barrier_path).unwrap();
        fs::create_dir(&barrier_path).unwrap();

        let cancellation = CancellationToken::new();
        let task_state = state.clone();
        let task_cancellation = cancellation.clone();
        let handle = tokio::spawn(async move {
            run_with_pending_lifetime(
                task_state,
                settings(),
                "token".into(),
                task_cancellation,
                std::time::Duration::from_secs(60),
            )
            .await
        });
        state.0.campaign_tasks.lock().await.insert(
            "bot".into(),
            CampaignTask {
                generation: 1,
                cancellation,
                handle: Some(handle),
                games: HashSet::new(),
                settled_games: HashSet::new(),
            },
        );
        http.wait_for_count("GET", "/api/challenge", 1).await;

        let error = crate::stop_campaign("bot".into(), crate::CoreStateRef::new(&state))
            .await
            .unwrap_err();
        assert!(error.contains("barrier"), "{error}");
        let runtime = state.0.campaign_runtimes.read().await["bot"].clone();
        assert_eq!(runtime.status, CampaignStatus::Error);
        assert_eq!(runtime.error.as_deref(), Some(error.as_str()));
        assert!(runtime.events.iter().all(|event| {
            !event
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("Outstanding challenges were canceled"))
        }));

        drop(state);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn authoritative_unknown_pending_challenge_consumes_campaign_capacity() {
        let http = ScriptedHttp::start().await;
        http.push(
            "GET",
            "/api/challenge",
            ScriptReply::Json(
                axum::http::StatusCode::OK,
                outgoing("authoritative-only", "Opponent"),
            ),
        );
        http.push(
            "POST",
            "/api/challenge/authoritative-only/cancel",
            ScriptReply::Json(axum::http::StatusCode::OK, "{}".into()),
        );
        let root = temp_root("authoritative-pending-capacity");
        let state = AppState::new_with_test_api(
            root.clone(),
            app_config("unused-engine", false),
            Arc::new(MemorySecretStore::with("bot", "token")),
            http.base(),
        )
        .unwrap();
        let cancellation = CancellationToken::new();
        let task_state = state.clone();
        let task_cancellation = cancellation.clone();
        let campaign = tokio::spawn(async move {
            run_with_pending_lifetime(
                task_state,
                settings(),
                "token".into(),
                task_cancellation,
                std::time::Duration::from_secs(60),
            )
            .await
        });

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if state
                    .0
                    .campaign_runtimes
                    .read()
                    .await
                    .get("bot")
                    .is_some_and(|runtime| runtime.pending_challenges == 1)
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("authoritative pending challenge was not counted");
        assert_eq!(http.count("GET", "/api/bot/online"), 0);
        assert_eq!(
            http.requests()
                .iter()
                .filter(|request| {
                    request.method == "POST"
                        && request.path.starts_with("/api/challenge/")
                        && !request.path.ends_with("/cancel")
                })
                .count(),
            0,
            "the authoritative-only pending challenge must occupy the sole slot"
        );

        cancellation.cancel();
        campaign.await.unwrap().unwrap();
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn failed_cancel_keeps_capacity_and_stop_surfaces_the_unresolved_id() {
        let http = ScriptedHttp::start().await;
        http.push(
            "GET",
            "/api/challenge",
            ScriptReply::Json(axum::http::StatusCode::OK, r#"{"out":[]}"#.into()),
        );
        http.push(
            "GET",
            "/api/bot/online",
            ScriptReply::Json(
                axum::http::StatusCode::OK,
                r#"{"id":"opponent","username":"Opponent","perfs":{"blitz":{"rating":2200,"games":100,"prov":false}}}"#.into(),
            ),
        );
        http.push(
            "POST",
            "/api/challenge/Opponent",
            ScriptReply::Json(
                axum::http::StatusCode::OK,
                r#"{"challenge":{"id":"c1","status":"created"}}"#.into(),
            ),
        );
        for _ in 0..2 {
            http.push(
                "POST",
                "/api/challenge/c1/cancel",
                ScriptReply::Json(
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    r#"{"error":"cancel uncertain"}"#.into(),
                ),
            );
        }
        http.push(
            "GET",
            "/api/challenge",
            ScriptReply::Json(axum::http::StatusCode::OK, outgoing("c1", "Opponent")),
        );
        http.push(
            "GET",
            "/api/challenge",
            ScriptReply::Json(axum::http::StatusCode::OK, outgoing("c1", "Opponent")),
        );
        let root = temp_root("cancel-capacity");
        let state = AppState::new_with_test_api(
            root.clone(),
            app_config("unused-engine", false),
            Arc::new(MemorySecretStore::with("bot", "token")),
            http.base(),
        )
        .unwrap();
        let cancellation = CancellationToken::new();
        let task_state = state.clone();
        let task_cancel = cancellation.clone();
        let campaign = tokio::spawn(async move {
            run_with_pending_lifetime(
                task_state,
                settings(),
                "token".into(),
                task_cancel,
                std::time::Duration::from_millis(10),
            )
            .await
        });
        http.wait_for_count("POST", "/api/challenge/Opponent", 1)
            .await;
        http.wait_for_count("POST", "/api/challenge/c1/cancel", 1)
            .await;
        http.wait_for_count("GET", "/api/challenge", 2).await;
        assert_eq!(http.count("POST", "/api/challenge/Opponent"), 1);
        let runtime = state.0.campaign_runtimes.read().await["bot"].clone();
        assert_eq!(runtime.pending_challenges, 1);
        assert_eq!(runtime.status, CampaignStatus::Unknown);
        assert!(runtime.error.unwrap().contains("c1"));

        cancellation.cancel();
        let error = campaign.await.unwrap().unwrap_err();
        assert!(error.contains("unresolved outgoing challenges"), "{error}");
        assert!(error.contains("c1"), "{error}");
        let runtime = state.0.campaign_runtimes.read().await["bot"].clone();
        assert_eq!(runtime.status, CampaignStatus::Error);
        assert_eq!(runtime.pending_challenges, 1);
        assert!(runtime
            .events
            .last()
            .and_then(|event| event.detail.as_deref())
            .is_some_and(|detail| detail.contains("c1")));
        assert!(runtime.events.iter().all(|event| {
            !event
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("Outstanding challenges were canceled"))
        }));
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn public_stop_campaign_returns_the_unresolved_cancel_error() {
        let http = ScriptedHttp::start().await;
        http.push(
            "GET",
            "/api/challenge",
            ScriptReply::Json(axum::http::StatusCode::OK, r#"{"out":[]}"#.into()),
        );
        http.push(
            "GET",
            "/api/bot/online",
            ScriptReply::Json(
                axum::http::StatusCode::OK,
                r#"{"id":"opponent","username":"Opponent","perfs":{"blitz":{"rating":2200,"games":100,"prov":false}}}"#.into(),
            ),
        );
        http.push(
            "POST",
            "/api/challenge/Opponent",
            ScriptReply::Json(
                axum::http::StatusCode::OK,
                r#"{"challenge":{"id":"public-stop-id","status":"created"}}"#.into(),
            ),
        );
        for _ in 0..2 {
            http.push(
                "POST",
                "/api/challenge/public-stop-id/cancel",
                ScriptReply::Json(
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    r#"{"error":"cancel remains uncertain"}"#.into(),
                ),
            );
        }
        for _ in 0..2 {
            http.push(
                "GET",
                "/api/challenge",
                ScriptReply::Json(
                    axum::http::StatusCode::OK,
                    outgoing("public-stop-id", "Opponent"),
                ),
            );
        }
        let root = temp_root("public-stop-error");
        let state = AppState::new_with_test_api(
            root.clone(),
            app_config("unused-engine", false),
            Arc::new(MemorySecretStore::with("bot", "token")),
            http.base(),
        )
        .unwrap();
        let cancellation = CancellationToken::new();
        let task_state = state.clone();
        let task_cancellation = cancellation.clone();
        let handle = tokio::spawn(async move {
            run_with_pending_lifetime(
                task_state,
                settings(),
                "token".into(),
                task_cancellation,
                std::time::Duration::from_millis(10),
            )
            .await
        });
        state.0.campaign_tasks.lock().await.insert(
            "bot".into(),
            CampaignTask {
                generation: 1,
                cancellation,
                handle: Some(handle),
                games: HashSet::new(),
                settled_games: HashSet::new(),
            },
        );
        http.wait_for_count("POST", "/api/challenge/public-stop-id/cancel", 1)
            .await;
        http.wait_for_count("GET", "/api/challenge", 2).await;

        let error = crate::stop_campaign("bot".into(), crate::CoreStateRef::new(&state))
            .await
            .unwrap_err();
        assert!(error.contains("unresolved outgoing challenges"), "{error}");
        assert!(error.contains("public-stop-id"), "{error}");
        assert!(!state.0.campaign_tasks.lock().await.contains_key("bot"));
        let runtime = state.0.campaign_runtimes.read().await["bot"].clone();
        assert_eq!(runtime.status, CampaignStatus::Error);
        assert_eq!(runtime.error.as_deref(), Some(error.as_str()));

        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn only_authoritative_absence_releases_a_failed_cancel_slot() {
        let http = ScriptedHttp::start().await;
        http.push(
            "GET",
            "/api/challenge",
            ScriptReply::Json(axum::http::StatusCode::OK, r#"{"out":[]}"#.into()),
        );
        http.push(
            "GET",
            "/api/bot/online",
            ScriptReply::Json(
                axum::http::StatusCode::OK,
                r#"{"id":"opponent","username":"Opponent","perfs":{"blitz":{"rating":2200,"games":100,"prov":false}}}"#.into(),
            ),
        );
        http.push(
            "POST",
            "/api/challenge/Opponent",
            ScriptReply::Json(
                axum::http::StatusCode::OK,
                r#"{"challenge":{"id":"c1","status":"created"}}"#.into(),
            ),
        );
        http.push(
            "POST",
            "/api/challenge/c1/cancel",
            ScriptReply::Json(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                r#"{"error":"cancel uncertain"}"#.into(),
            ),
        );
        http.push(
            "GET",
            "/api/challenge",
            ScriptReply::Json(axum::http::StatusCode::OK, r#"{"out":[]}"#.into()),
        );
        let root = temp_root("cancel-authoritative-absence");
        let state = AppState::new_with_test_api(
            root.clone(),
            app_config("unused-engine", false),
            Arc::new(MemorySecretStore::with("bot", "token")),
            http.base(),
        )
        .unwrap();
        let cancellation = CancellationToken::new();
        let task_state = state.clone();
        let task_cancel = cancellation.clone();
        let campaign = tokio::spawn(async move {
            run_with_pending_lifetime(
                task_state,
                settings(),
                "token".into(),
                task_cancel,
                std::time::Duration::from_millis(10),
            )
            .await
        });

        http.wait_for_count("POST", "/api/challenge/c1/cancel", 1)
            .await;
        http.wait_for_count("GET", "/api/challenge", 2).await;
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if state.0.campaign_runtimes.read().await["bot"].pending_challenges == 0 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("authoritative absence did not release the slot");
        let requests = http.requests();
        let cancel_index = requests
            .iter()
            .position(|request| request.path == "/api/challenge/c1/cancel")
            .unwrap();
        let absence_index = requests
            .iter()
            .enumerate()
            .filter(|(_, request)| request.path == "/api/challenge")
            .nth(1)
            .map(|(index, _)| index)
            .unwrap();
        assert!(cancel_index < absence_index);
        assert_eq!(http.count("POST", "/api/challenge/Opponent"), 1);

        cancellation.cancel();
        campaign.await.unwrap().unwrap();
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }
}
