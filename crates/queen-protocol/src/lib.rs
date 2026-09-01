use queen_core::{
    enginelog::{LogFilter, LogQuery, LogRetention},
    history::ScorebookFilter,
    models::{
        AddAccountRequest, CampaignSettings, ChallengeRequest, EngineOptionUpdate,
        OpeningBookUpdate,
    },
    CoreEvent,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

// Campaign scheduling and incoming-challenge acceptance add command semantics
// that an older runner would silently ignore as unknown JSON fields. Require a
// matching runner instead of presenting controls that are not enforced.
pub const PROTOCOL_VERSION: u32 = 3;
pub const CONTENT_SHA256_HEADER: &str = "x-queenui-content-sha256";
pub const REQUEST_ID_HEADER: &str = "x-queenui-request-id";
pub const IDEMPOTENCY_TTL_SECONDS: i64 = 24 * 60 * 60;
pub const IDEMPOTENCY_PENDING_WAIT_SECONDS: u64 = 30;
pub const PAIRING_PAYLOAD_VERSION: u32 = 2;
pub const ENGINE_BROWSE_DEFAULT_PAGE_ENTRIES: u16 = 100;
pub const ENGINE_BROWSE_MAX_PAGE_ENTRIES: u16 = 200;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct EngineRoot {
    /// Stable administrator-chosen identifier. The absolute host path never
    /// crosses the runner protocol.
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ts_rs::TS)]
#[serde(default, rename_all = "camelCase")]
pub struct EngineBrowseRequest {
    pub root_id: String,
    /// Slash-separated path relative to the configured root. Empty means the
    /// root itself; absolute paths and empty interior components are invalid.
    pub relative_path: String,
    pub cursor: Option<String>,
    pub page_entries: Option<u16>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct EngineBrowseEntry {
    pub name: String,
    pub relative_path: String,
    pub kind: EngineBrowseEntryKind,
    pub size: u64,
    pub modified_at_ms: Option<u64>,
    pub executable: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub enum EngineBrowseEntryKind {
    Directory,
    File,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct EngineBrowseResponse {
    pub root_id: String,
    pub relative_path: String,
    pub entries: Vec<EngineBrowseEntry>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunnerIdentity {
    pub version: u32,
    pub url: String,
    /// Lowercase SHA-256 hex of the runner certificate's DER bytes.
    pub cert_fp: String,
    /// Long-lived bearer issued by the runner. It is never included in a
    /// pairing carrier and is stored only by the Rust desktop backend.
    pub bearer: String,
    /// Authenticated credential generation. Rotation increments this value.
    pub generation: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairRedeemRequest {
    pub enroll: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairRedeemResponse {
    pub protocol_version: u32,
    pub runner_id: Uuid,
    pub bearer: String,
    pub generation: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingResponse {
    pub protocol_version: u32,
    pub request_id: Uuid,
    pub status: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    pub service: String,
    pub protocol_version: u32,
    pub instance_id: Uuid,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunnerCapabilities {
    pub protocol_version: u32,
    pub instance_id: Uuid,
    pub hostname: String,
    pub operating_system: String,
    pub architecture: String,
    pub logical_cpus: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotResponse {
    pub protocol_version: u32,
    pub instance_id: Uuid,
    pub snapshot: queen_core::models::AppSnapshot,
}

/// Instantaneous authoritative ownership held by a live runner core. Unlike a
/// presentation snapshot, this includes pre-snapshot game reservations,
/// durable recovery intents, and challenge-creation safety barriers.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoverInventory {
    pub live_games: usize,
    pub outgoing_challenges: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandRequest {
    pub request_id: Uuid,
    #[serde(flatten)]
    pub command: RunnerCommand,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "command", content = "payload", rename_all = "camelCase")]
pub enum RunnerCommand {
    RegisterEngine {
        root_id: String,
        relative_path: String,
    },
    RemoveEngine {
        engine_id: String,
    },
    UpdateEngineOptions {
        engine_id: String,
        options: Vec<EngineOptionUpdate>,
    },
    RefreshEngineOptions {
        engine_id: String,
    },
    ConfigureOpeningBook {
        request: OpeningBookUpdate,
    },
    ClearEngineOpeningBook {
        engine_id: String,
    },
    AddLichessAccount {
        request: AddAccountRequest,
    },
    UpdateLichessAccountToken {
        #[serde(rename = "accountId")]
        account_id: String,
        token: String,
    },
    UpdateAccountEngine {
        account_id: String,
        engine_id: String,
    },
    RemoveLichessAccount {
        account_id: String,
    },
    StartBot {
        account_id: String,
    },
    StopBot {
        account_id: String,
    },
    StartCampaign {
        settings: CampaignSettings,
    },
    StopCampaign {
        account_id: String,
    },
    CreateChallenge {
        request: ChallengeRequest,
    },
    DismissGameError {
        #[serde(rename = "gameId")]
        game_id: String,
    },
    HandoverInventory,
    GetScorebookStats {
        filter: ScorebookFilter,
    },
    ImportLichessHistory {
        account_id: String,
        max: Option<u32>,
    },
    ListLogSessions {
        filter: LogFilter,
    },
    GetLogPage {
        session_id: String,
        offset: u64,
        limit: u64,
    },
    GetLogOutline {
        session_id: String,
    },
    SearchLogSession {
        session_id: String,
        query: LogQuery,
    },
    SearchLogSessions {
        filter: LogFilter,
        query: LogQuery,
    },
    DeleteLogSession {
        session_id: String,
    },
    ClearLogSessions,
    GetLogsOverview,
    SetLogRetention {
        retention: LogRetention,
    },
    GetDiagnostics {
        filter: queen_core::diagnostics::DiagnosticFilter,
    },
    ClearDiagnostics,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandResponse {
    pub protocol_version: u32,
    pub request_id: Uuid,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ApiError>,
}

impl CommandResponse {
    pub fn success(request_id: Uuid, result: Value) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            request_id,
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    pub fn failure(request_id: Uuid, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            request_id,
            ok: false,
            result: None,
            error: Some(ApiError {
                code: code.into(),
                message: message.into(),
            }),
        }
    }
}

/// Digest of the exact serialized HTTP command body. The client retains these
/// bytes across retries and the runner hashes them before deserialization, so
/// a key can never be reused for a byte-distinct request representation.
pub fn command_body_digest(body: &[u8]) -> [u8; 32] {
    Sha256::digest(body).into()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiError {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventEnvelope {
    pub protocol_version: u32,
    pub instance_id: Uuid,
    pub sequence: u64,
    pub event: CoreEvent,
}

#[cfg(test)]
mod tests {
    use super::{command_body_digest, HandoverInventory, RunnerCommand};

    #[test]
    fn command_digest_binds_the_exact_serialized_bytes() {
        let compact = br#"{"requestId":"same","command":"stopBot"}"#;
        let spaced = br#"{ "requestId": "same", "command": "stopBot" }"#;
        assert_eq!(command_body_digest(compact), command_body_digest(compact));
        assert_ne!(command_body_digest(compact), command_body_digest(spaced));
    }

    #[test]
    fn handover_inventory_wire_contract_is_camel_case() {
        assert_eq!(
            serde_json::to_value(RunnerCommand::HandoverInventory).unwrap(),
            serde_json::json!({ "command": "handoverInventory" })
        );
        assert_eq!(
            serde_json::to_value(HandoverInventory {
                live_games: 2,
                outgoing_challenges: 3,
            })
            .unwrap(),
            serde_json::json!({ "liveGames": 2, "outgoingChallenges": 3 })
        );
    }

    #[test]
    fn operator_recovery_commands_are_additive_camel_case_variants() {
        assert_eq!(
            serde_json::to_value(RunnerCommand::UpdateLichessAccountToken {
                account_id: "bot".into(),
                token: "replacement".into(),
            })
            .unwrap(),
            serde_json::json!({
                "command": "updateLichessAccountToken",
                "payload": { "accountId": "bot", "token": "replacement" }
            })
        );
        assert_eq!(
            serde_json::to_value(RunnerCommand::DismissGameError {
                game_id: "failed-game".into(),
            })
            .unwrap(),
            serde_json::json!({
                "command": "dismissGameError",
                "payload": { "gameId": "failed-game" }
            })
        );
    }
}
