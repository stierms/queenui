use queen_core::{models::CampaignSettings, AppState, CoreStateRef};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::{mpsc, oneshot, Mutex};

const ACCOUNT_NORMAL_QUEUE: usize = 8;
const ACCOUNT_PRIORITY_QUEUE: usize = 8;
const PRIORITY_INTERRUPT_BUDGET: Duration = Duration::from_millis(750);

#[derive(Clone)]
pub(crate) struct LifecycleActors {
    core: AppState,
    actors: Arc<Mutex<HashMap<String, ActorHandle>>>,
}

#[derive(Clone)]
struct ActorHandle {
    normal: mpsc::Sender<NormalRequest>,
    priority: mpsc::Sender<PriorityRequest>,
}

struct NormalRequest {
    operation: NormalOperation,
    reply: oneshot::Sender<Result<(), String>>,
}

enum NormalOperation {
    StartBot,
    StartCampaign(CampaignSettings),
    #[cfg(test)]
    WaitUntilPriorityStop(oneshot::Sender<()>),
}

struct PriorityRequest {
    operation: PriorityOperation,
    reply: oneshot::Sender<Result<(), String>>,
}

#[derive(Clone, Copy)]
enum PriorityOperation {
    StopBot,
    StopCampaign,
}

impl LifecycleActors {
    pub(crate) fn new(core: AppState) -> Self {
        Self {
            core,
            actors: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(crate) async fn start_bot(&self, account_id: String) -> Result<(), String> {
        self.send_normal(account_id, NormalOperation::StartBot)
            .await
    }

    pub(crate) async fn start_campaign(&self, settings: CampaignSettings) -> Result<(), String> {
        let account_id = settings.account_id.clone();
        self.send_normal(account_id, NormalOperation::StartCampaign(settings))
            .await
    }

    pub(crate) async fn stop_bot(&self, account_id: String) -> Result<(), String> {
        self.send_priority(account_id, PriorityOperation::StopBot)
            .await
    }

    pub(crate) async fn stop_campaign(&self, account_id: String) -> Result<(), String> {
        self.send_priority(account_id, PriorityOperation::StopCampaign)
            .await
    }

    async fn send_normal(
        &self,
        account_id: String,
        operation: NormalOperation,
    ) -> Result<(), String> {
        self.ensure_account(&account_id).await?;
        let actor = self.actor(account_id).await;
        let (reply, response) = oneshot::channel();
        actor
            .normal
            .try_send(NormalRequest { operation, reply })
            .map_err(|_| "The account lifecycle queue is full".to_string())?;
        response
            .await
            .map_err(|_| "The account lifecycle owner stopped unexpectedly".to_string())?
    }

    async fn send_priority(
        &self,
        account_id: String,
        operation: PriorityOperation,
    ) -> Result<(), String> {
        self.ensure_account(&account_id).await?;
        let actor = self.actor(account_id).await;
        let (reply, response) = oneshot::channel();
        actor
            .priority
            .try_send(PriorityRequest { operation, reply })
            .map_err(|_| "The account lifecycle owner stopped unexpectedly".to_string())?;
        response
            .await
            .map_err(|_| "The account lifecycle owner stopped unexpectedly".to_string())?
    }

    async fn ensure_account(&self, account_id: &str) -> Result<(), String> {
        if self
            .core
            .snapshot()
            .await
            .accounts
            .iter()
            .any(|account| account.id == account_id)
        {
            Ok(())
        } else {
            Err("Lichess account not found".into())
        }
    }

    async fn actor(&self, account_id: String) -> ActorHandle {
        let mut actors = self.actors.lock().await;
        if let Some(actor) = actors.get(&account_id) {
            return actor.clone();
        }
        let (normal_tx, normal_rx) = mpsc::channel(ACCOUNT_NORMAL_QUEUE);
        let (priority_tx, priority_rx) = mpsc::channel(ACCOUNT_PRIORITY_QUEUE);
        let actor = ActorHandle {
            normal: normal_tx,
            priority: priority_tx,
        };
        tokio::spawn(run_actor(
            self.core.clone(),
            account_id.clone(),
            normal_rx,
            priority_rx,
        ));
        actors.insert(account_id, actor.clone());
        actor
    }
}

async fn run_actor(
    core: AppState,
    account_id: String,
    mut normal: mpsc::Receiver<NormalRequest>,
    mut priority: mpsc::Receiver<PriorityRequest>,
) {
    loop {
        tokio::select! {
            biased;
            Some(request) = priority.recv() => {
                reject_queued_normal(&mut normal);
                let result = run_priority(&core, &account_id, request.operation).await;
                let _ = request.reply.send(result);
            }
            Some(request) = normal.recv() => {
                run_normal(&core, &account_id, request, &mut normal, &mut priority).await;
            }
            else => return,
        }
    }
}

async fn run_normal(
    core: &AppState,
    account_id: &str,
    request: NormalRequest,
    normal: &mut mpsc::Receiver<NormalRequest>,
    priority: &mut mpsc::Receiver<PriorityRequest>,
) {
    let NormalRequest { operation, reply } = request;
    let step_cancellation = tokio_util::sync::CancellationToken::new();
    let _operation_cancellation = step_cancellation.clone();
    let operation = async move {
        match operation {
            NormalOperation::StartBot => {
                queen_core::start_bot(account_id.to_string(), CoreStateRef::new(core)).await
            }
            NormalOperation::StartCampaign(settings) => {
                queen_core::start_campaign(settings, CoreStateRef::new(core)).await
            }
            #[cfg(test)]
            NormalOperation::WaitUntilPriorityStop(started) => {
                let _ = started.send(());
                _operation_cancellation.cancelled().await;
                Err("The actor step observed priority cancellation".into())
            }
        }
    };
    let mut operation = Box::pin(operation);
    tokio::select! {
        biased;
        Some(stop) = priority.recv() => {
            step_cancellation.cancel();
            interrupt_priority(core, account_id, stop.operation).await;
            let interrupted = tokio::time::timeout(PRIORITY_INTERRUPT_BUDGET, &mut operation).await;
            drop(operation);
            reject_queued_normal(normal);
            let normal_error = if interrupted.is_ok() {
                "The lifecycle operation was interrupted by priority Stop"
            } else {
                "The lifecycle operation was cancelled after its bounded interrupt budget"
            };
            let _ = reply.send(Err(normal_error.into()));
            let result = run_priority(core, account_id, stop.operation).await;
            let _ = stop.reply.send(result);
        }
        result = &mut operation => {
            let _ = reply.send(result);
        }
    }
}

fn reject_queued_normal(normal: &mut mpsc::Receiver<NormalRequest>) {
    while let Ok(request) = normal.try_recv() {
        let _ = request.reply.send(Err(
            "The lifecycle operation was superseded by priority Stop".into(),
        ));
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::{LifecycleActors, NormalOperation};
    use queen_core::{
        models::{AccountProfile, AppConfig, CampaignStatus},
        storage::FileSecretStore,
        AppState,
    };
    use std::{sync::Arc, time::Duration};
    use tokio::sync::oneshot;
    use uuid::Uuid;

    fn lifecycle_actors() -> (LifecycleActors, AppState, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("queen-lifecycle-{}", Uuid::new_v4()));
        let core = AppState::new_with_secret_store(
            root.clone(),
            AppConfig {
                accounts: vec![AccountProfile {
                    id: "bot".into(),
                    username: "Bot".into(),
                    engine_id: "unused".into(),
                    rating: None,
                    enabled: false,
                }],
                ..AppConfig::default()
            },
            Arc::new(FileSecretStore::new(root.join("secrets"))),
        )
        .unwrap();
        (LifecycleActors::new(core.clone()), core, root)
    }

    #[tokio::test]
    async fn priority_stop_interrupts_a_long_running_actor_step_instead_of_queuing_behind_it() {
        let (actors, _core, root) = lifecycle_actors();
        let actor = actors.actor("bot".into()).await;
        let (started_tx, started_rx) = oneshot::channel();
        let (reply_tx, reply_rx) = oneshot::channel();
        actor
            .normal
            .try_send(super::NormalRequest {
                operation: NormalOperation::WaitUntilPriorityStop(started_tx),
                reply: reply_tx,
            })
            .unwrap();
        started_rx.await.unwrap();

        tokio::time::timeout(Duration::from_secs(2), actors.stop_bot("bot".into()))
            .await
            .expect("priority Stop deadline")
            .unwrap();
        assert!(reply_rx.await.unwrap().is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn stop_campaign_preserves_live_game_and_coordinator_then_stop_bot_drains_them() {
        let (actors, core, root) = lifecycle_actors();
        let game = core
            .install_running_campaign_game_for_test("bot", "game")
            .await;
        game.submit_move().await.unwrap();

        actors.stop_campaign("bot".into()).await.unwrap();

        assert!(game.campaign_is_stopped());
        assert!(!game.game_cancellation_requested());
        assert!(!game.game_is_stopped());
        assert_eq!(core.live_game_ownership_count().await, 1);
        game.submit_move().await.unwrap();

        actors.stop_bot("bot".into()).await.unwrap();

        assert!(game.game_cancellation_requested());
        assert!(game.game_is_stopped());
        assert_eq!(core.live_game_ownership_count().await, 0);
        assert!(game.submit_move().await.is_err());
        assert_eq!(
            core.snapshot().await.campaign_runtimes[0].status,
            CampaignStatus::Stopped
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn priority_stop_campaign_interrupts_a_long_running_actor_step_without_stopping_games() {
        let (actors, core, root) = lifecycle_actors();
        let game = core
            .install_running_campaign_game_for_test("bot", "game")
            .await;
        let actor = actors.actor("bot".into()).await;
        let (started_tx, started_rx) = oneshot::channel();
        let (reply_tx, reply_rx) = oneshot::channel();
        actor
            .normal
            .try_send(super::NormalRequest {
                operation: NormalOperation::WaitUntilPriorityStop(started_tx),
                reply: reply_tx,
            })
            .unwrap();
        started_rx.await.unwrap();

        tokio::time::timeout(Duration::from_secs(2), actors.stop_campaign("bot".into()))
            .await
            .expect("priority campaign Stop deadline")
            .unwrap();

        assert!(reply_rx.await.unwrap().is_err());
        assert!(game.campaign_is_stopped());
        assert!(!game.game_cancellation_requested());
        assert!(!game.game_is_stopped());
        game.submit_move().await.unwrap();
        actors.stop_bot("bot".into()).await.unwrap();
        assert!(game.game_is_stopped());
        let _ = std::fs::remove_dir_all(root);
    }
}

async fn interrupt_priority(core: &AppState, account_id: &str, operation: PriorityOperation) {
    match operation {
        PriorityOperation::StopBot => core.interrupt_account(account_id).await,
        PriorityOperation::StopCampaign => core.interrupt_campaign(account_id).await,
    }
}

async fn run_priority(
    core: &AppState,
    account_id: &str,
    operation: PriorityOperation,
) -> Result<(), String> {
    interrupt_priority(core, account_id, operation).await;
    match operation {
        PriorityOperation::StopBot => {
            queen_core::stop_bot(account_id.to_string(), CoreStateRef::new(core)).await
        }
        PriorityOperation::StopCampaign => {
            queen_core::stop_campaign(account_id.to_string(), CoreStateRef::new(core)).await
        }
    }
}
