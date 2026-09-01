use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Clone, Debug, Default, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    pub engines: Vec<EngineProfile>,
    pub accounts: Vec<AccountProfile>,
    #[serde(default)]
    pub campaigns: Vec<CampaignSettings>,
    /// How much engine-log history to keep on disk. Defined by the log store
    /// itself, so the policy and its enforcement stay in one module.
    #[serde(default)]
    pub log_retention: crate::enginelog::LogRetention,
}

#[derive(Clone, Debug, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct EngineProfile {
    pub id: String,
    pub name: String,
    pub path: String,
    pub author: Option<String>,
    pub option_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub last_probed_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub probe_ok: Option<bool>,
    #[serde(default)]
    pub options: Vec<UciOption>,
    #[serde(default)]
    pub opening_book: Option<OpeningBookConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UciOption {
    pub name: String,
    pub option_type: String,
    pub default_value: Option<String>,
    pub value: Option<String>,
    pub min: Option<i64>,
    pub max: Option<i64>,
    #[serde(default)]
    pub choices: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct OpeningBookConfig {
    pub enabled: bool,
    pub path: String,
    pub name: String,
    pub format: String,
    pub max_plies: u32,
    pub top_move_percent: u32,
    pub entry_count: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct EngineOptionUpdate {
    pub name: String,
    pub value: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct OpeningBookUpdate {
    pub engine_id: String,
    pub path: String,
    pub enabled: bool,
    pub max_plies: u32,
    pub top_move_percent: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AccountProfile {
    pub id: String,
    pub username: String,
    pub engine_id: String,
    pub rating: Option<i64>,
    /// Desired supervisor state. Headless runners restore enabled accounts
    /// after a service restart; transient runtime status remains separate.
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BotRuntime {
    pub account_id: String,
    pub status: String,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LiveGame {
    pub id: String,
    pub account_id: String,
    pub bot_username: String,
    pub opponent: String,
    pub bot_rating: Option<i64>,
    pub opponent_rating: Option<i64>,
    pub color: String,
    pub initial_fen: String,
    pub moves: String,
    pub status: String,
    pub white_time: i64,
    pub black_time: i64,
    pub white_increment: i64,
    pub black_increment: i64,
    pub clock_updated_at: u64,
    pub result: Option<String>,
    pub engine_line: Option<String>,
    pub engine_info: Option<EngineTelemetry>,
    pub engine_thinking: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct EngineTelemetry {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub depth: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub selective_depth: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub score_cp: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub mate_in: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub score_bound: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub nodes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub nodes_per_second: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub time_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub hash_full: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub tablebase_hits: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub multi_pv: Option<u32>,
    pub principal_variation: Vec<String>,
    pub raw: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AppSnapshot {
    pub engines: Vec<EngineProfile>,
    pub accounts: Vec<AccountProfile>,
    pub runtimes: Vec<BotRuntime>,
    pub games: Vec<LiveGame>,
    pub campaigns: Vec<CampaignSettings>,
    pub campaign_runtimes: Vec<CampaignRuntime>,
}

#[derive(Clone, Debug, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CampaignSettings {
    pub account_id: String,
    pub min_rating: i64,
    pub max_rating: i64,
    pub concurrency: u32,
    pub clock_limit: u32,
    pub clock_increment: u32,
    /// Defaults to rated matchmaking when absent from persisted backend data.
    #[serde(default = "default_campaign_rated")]
    pub rated: bool,
    pub color: String,
    /// Accept compatible incoming challenges while this campaign is active.
    #[serde(default)]
    pub accept_incoming_challenges: bool,
    /// Mutually exclusive automatic stop conditions. `None` means the run is
    /// stopped manually.
    #[serde(default)]
    pub stop_after_minutes: Option<u32>,
    #[serde(default)]
    pub stop_after_games: Option<u32>,
}

fn default_campaign_rated() -> bool {
    true
}

impl Default for CampaignSettings {
    fn default() -> Self {
        Self {
            account_id: String::new(),
            min_rating: 0,
            max_rating: 0,
            concurrency: 0,
            clock_limit: 0,
            clock_increment: 0,
            rated: default_campaign_rated(),
            color: String::new(),
            accept_incoming_challenges: false,
            stop_after_minutes: None,
            stop_after_games: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CampaignRuntime {
    pub account_id: String,
    pub status: CampaignStatus,
    pub active_games: u32,
    pub pending_challenges: u32,
    pub eligible_bots: u32,
    pub online_bots_scanned: u32,
    pub challenges_sent: u64,
    pub games_started: u64,
    pub last_opponent: Option<String>,
    pub activity: String,
    pub error: Option<String>,
    pub next_scan_at: Option<u64>,
    pub stop_at: Option<u64>,
    pub events: Vec<CampaignEvent>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum CampaignStatus {
    Starting,
    Discovering,
    Challenging,
    Running,
    Waiting,
    Backoff,
    Stopping,
    Stopped,
    Error,
    /// A Tier-A ambiguous Lichess write is held here until authoritative
    /// reconciliation proves whether the challenge exists.
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CampaignEvent {
    pub id: String,
    pub timestamp: u64,
    pub kind: String,
    pub title: String,
    pub detail: Option<String>,
}

impl CampaignRuntime {
    pub fn stopped(account_id: String) -> Self {
        Self {
            account_id,
            status: CampaignStatus::Stopped,
            active_games: 0,
            pending_challenges: 0,
            eligible_bots: 0,
            online_bots_scanned: 0,
            challenges_sent: 0,
            games_started: 0,
            last_opponent: None,
            activity: "Ready".into(),
            error: None,
            next_scan_at: None,
            stop_at: None,
            events: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AddAccountRequest {
    pub token: String,
    pub engine_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AddAccountResult {
    pub account: AccountProfile,
    /// OAuth scopes reported by Lichess for the token that was just validated.
    pub scopes: Vec<String>,
    /// Required matchmaking scopes absent from `scopes`, in stable display order.
    pub missing_for_matchmaking: Vec<String>,
    /// Explicitly distinguishes a token that cannot operate bot games at all.
    pub can_play_games: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ChallengeRequest {
    pub account_id: String,
    pub opponent: String,
    pub clock_limit: u32,
    pub clock_increment: u32,
    pub rated: bool,
    pub color: String,
    pub variant: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ChallengeResult {
    pub id: String,
    pub status: String,
    pub url: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct LichessAccount {
    pub id: String,
    pub username: String,
    pub title: Option<String>,
    #[serde(default)]
    pub perfs: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize)]
pub struct OnlineBot {
    pub id: String,
    pub username: String,
    #[serde(default)]
    pub perfs: serde_json::Value,
}

impl OnlineBot {
    pub fn rating_for(&self, perf: &str) -> Option<(i64, i64, bool)> {
        let value = self.perfs.get(perf)?;
        Some((
            value.get("rating")?.as_i64()?,
            value.get("games")?.as_i64()?,
            value
                .get("prov")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        ))
    }
}

impl LichessAccount {
    pub fn rating(&self) -> Option<i64> {
        self.perfs
            .get("blitz")
            .and_then(|value| value.get("rating"))
            .and_then(serde_json::Value::as_i64)
            .or_else(|| {
                self.perfs
                    .as_object()
                    .and_then(|perfs| perfs.values().find_map(|perf| perf.get("rating")))
                    .and_then(serde_json::Value::as_i64)
            })
    }
}
