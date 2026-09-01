use crate::{
    history::perf_key_for_clock,
    lichess,
    models::{
        CampaignEvent, CampaignRuntime, CampaignSettings, CampaignStatus, ChallengeRequest,
        OnlineBot,
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
}

#[derive(Default)]
struct FilterStats {
    total: u32,
    missing_pool: u32,
    provisional_or_unplayed: u32,
    outside_range: u32,
    busy_or_cooling_down: u32,
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
            last_opponent: None,
            activity: "Connecting matchmaking…".into(),
            error: None,
            next_scan_at: None,
            events: vec![new_event(
                "start",
                "Matchmaking started",
                Some(format!(
                    "Rating {}–{} · concurrency {} · {}+{} · {}",
                    settings.min_rating,
                    settings.max_rating,
                    settings.concurrency,
                    settings.clock_limit / 60,
                    settings.clock_increment,
                    if settings.rated { "rated" } else { "casual" }
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
    // Campaign startup and every ambiguous POST pass through the same
    // authoritative reconciliation barrier before any new challenge creation.
    let mut unknown_creation = Some("startup reconciliation".to_string());

    'campaign: while !cancellation.is_cancelled() {
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
        let occupied = active_games.saturating_add(pending.len() as u32);
        let available_slots = settings.concurrency.saturating_sub(occupied);
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
                runtime.activity = format!(
                    "At capacity: {} game(s), {} pending",
                    active_games,
                    pending.len()
                );
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
                } else {
                    "Matchmaking stopped"
                },
                stop_error.clone().or_else(|| {
                    Some("Outstanding challenges were canceled; active games continue".into())
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
            record_event(
                state,
                account_id,
                "accepted",
                "Challenge accepted — game started",
                Some(match opponent {
                    Some(opponent) => format!("Playing {opponent} · game #{game_id}"),
                    None => format!("Game #{game_id}"),
                }),
            )
            .await;
        }
        "gameFinish" => {
            let game_id = event
                .pointer("/game/gameId")
                .or_else(|| event.pointer("/game/id"))
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            record_event(
                state,
                account_id,
                "finished",
                "Game finished — capacity will refill",
                Some(format!("Game #{game_id}")),
            )
            .await;
        }
        _ => {}
    }
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
        filter_candidates, randomize_candidates, reconcile_pending,
        reconcile_pending_authoritatively, run_with_pending_lifetime, validate, CampaignTask,
        PendingChallenge, PendingState,
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
