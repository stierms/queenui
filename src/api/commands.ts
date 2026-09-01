import { invoke } from "@tauri-apps/api/core";
import type {
  AddAccountResult,
  BackendSnapshotEvent,
  CampaignSettings,
  ChallengeRequest,
  DiagnosticEntry,
  DiagnosticFilter,
  ImportReport,
  ExportMode,
  LogFilter,
  LogPage,
  LogQuery,
  LogMatch,
  LogRetention,
  LogSearchBlock,
  LogSessionMatches,
  LogSessionSummary,
  LogsOverview,
  OpeningBookRequest,
  RunnerConnectionTest,
  RunnerSettingsView,
  ScorebookFilter,
  ScorebookStats,
  EngineOptionUpdate,
  EngineBrowseRequest,
  EngineBrowseResponse,
  EngineRoot,
  OpeningBookAsset,
} from "../types";

export function getRunnerSettings(): Promise<RunnerSettingsView> {
  return invoke<RunnerSettingsView>("get_runner_settings");
}

/**
 * No `token` and no `allowInsecureRemoteHttp` parameter, deliberately: both
 * commands reject a non-empty bearer ("Direct bearer entry is retired; use the
 * one-time runner pairing flow") and refuse cleartext remote HTTP outright, and
 * the bearer plus certificate pin are only ever created by the pairing flow.
 * Leaving the arguments out of the signature is what makes it impossible for a
 * caller to build a submission the backend is guaranteed to refuse.
 *
 * `acknowledgedRunner` is the exception that proves the rule: it is the one
 * argument that turns a refusal into a save. Leaving a remote runner — for this
 * computer or for a different runner — is refused while that runner is playing,
 * while it still owns outgoing challenges, or while the backend could not reach
 * it to find out, and the refusal names what has to be acknowledged. Passing it
 * is only ever the answer to a question the backend asked, so it stays optional
 * and unset by default: a caller that has not been refused anything has nothing
 * to acknowledge.
 *
 * It is a URL rather than a flag, and that is the whole point of it. A boolean
 * acknowledged *a switch*, so it survived the question it answered: the operator
 * confirmed runner A's games, the resend found runner C in place — a re-pairing,
 * a recovery, any republished backend — and the same `true` waved C's games
 * through unread. The backend now compares this against the live remote's own
 * canonical base URL (`verify_remote_handover`), so an acknowledgement of A is
 * not an acknowledgement of C: it re-refuses, naming C. Callers must therefore
 * send the URL from the refusal they are answering *now*, never a remembered
 * one.
 */
export function setRunnerSettings(
  mode: "embedded" | "remote",
  url?: string,
  acknowledgedRunner?: string,
): Promise<RunnerSettingsView> {
  return invoke<RunnerSettingsView>("set_runner_settings", {
    mode,
    url,
    acknowledgedRunner,
  });
}

/** Uses the stored pairing for `url`; see `setRunnerSettings` above. */
export function testRunnerConnection(
  url: string,
): Promise<RunnerConnectionTest> {
  return invoke<RunnerConnectionTest>("test_runner_connection", { url });
}

/**
 * The whole application state, in the same stamped envelope the snapshot *event*
 * carries.
 *
 * The fetch used to answer with a bare `AppSnapshot`, and the frontend had to
 * attribute it to whichever backend was live when the response landed — sound
 * only for as long as "when it was dispatched" and "when it was read" cannot
 * straddle a runner switch. They can: the call is awaited across an IPC hop and
 * a possible remote round trip, and a live switch publishes a different backend
 * without waiting for it. So the backend stamps the answer with the generation
 * of the backend that actually produced it (`get_snapshot_inner`), and a
 * response from a retired one is dropped exactly like a retired event —
 * `useSnapshot` runs both through the same rule.
 */
export function getSnapshot(): Promise<BackendSnapshotEvent> {
  return invoke<BackendSnapshotEvent>("get_snapshot");
}

export function addEngine(path: string): Promise<void> {
  return invoke<void>("add_engine", { path });
}

export function listEngineRoots(): Promise<EngineRoot[]> {
  return invoke<EngineRoot[]>("list_engine_roots");
}

export function browseEngineRoot(
  request: EngineBrowseRequest,
): Promise<EngineBrowseResponse> {
  return invoke<EngineBrowseResponse>("browse_engine_root", { request });
}

export function listOpeningBookAssets(): Promise<OpeningBookAsset[]> {
  return invoke<OpeningBookAsset[]>("list_opening_book_assets");
}

export function registerEngine(
  rootId: string,
  relativePath: string,
): Promise<void> {
  return invoke<void>("register_engine", { rootId, relativePath });
}

export function removeEngine(engineId: string): Promise<void> {
  return invoke<void>("remove_engine", { engineId });
}

export function updateEngineOptions(
  engineId: string,
  options: EngineOptionUpdate[],
): Promise<void> {
  return invoke<void>("update_engine_options", { engineId, options });
}

export function refreshEngineOptions(engineId: string): Promise<void> {
  return invoke<void>("refresh_engine_options", { engineId });
}

export function configureOpeningBook(
  engineId: string,
  book: OpeningBookRequest,
): Promise<void> {
  return invoke<void>("configure_opening_book", {
    request: { engineId, ...book },
  });
}

export function clearEngineOpeningBook(engineId: string): Promise<void> {
  return invoke<void>("clear_engine_opening_book", { engineId });
}

/**
 * Connects a Lichess BOT token, and reports what that token can do.
 *
 * The answer used to be typed `void`, which threw away the only moment QueenUI
 * ever learns a token's OAuth scopes: Lichess returns them on the validation
 * call and nowhere else, and the snapshot carries no scope data at all. A
 * play-only token was therefore stored with the same receipt a full token got,
 * and the next word on the subject was an opaque 403 from matchmaking.
 */
export function addLichessAccount(
  token: string,
  engineId: string,
): Promise<AddAccountResult> {
  return invoke<AddAccountResult>("add_lichess_account", {
    request: { token, engineId },
  });
}

/**
 * Swaps the stored token of an account that is already connected.
 *
 * The answer is the same envelope `add_lichess_account` returns, and for the
 * same reason: Lichess reports a token's scopes on the validation call and
 * nowhere else, so a replacement is the only other moment QueenUI can learn
 * what an account's token is capable of. A replacement that quietly drops
 * `challenge:write` has to be as visible as a connect that does.
 *
 * `accountId` is not redundant with the token. The backend validates the token
 * against Lichess and refuses it when it belongs to a different account
 * ("The Lichess token belongs to @X (x), but the selected account is @Y (y).")
 * rather than repointing the profile at whoever the token turns out to be.
 *
 * The command rewrites the secret and nothing else — no config write, no
 * restart, no bot stopped. Tasks already running hold the token they started
 * with, so the new one first applies at the next game or campaign start.
 */
export function updateLichessAccountToken(
  accountId: string,
  token: string,
): Promise<AddAccountResult> {
  return invoke<AddAccountResult>("update_lichess_account_token", {
    accountId,
    token,
  });
}

export function updateAccountEngine(
  accountId: string,
  engineId: string,
): Promise<void> {
  return invoke<void>("update_account_engine", { accountId, engineId });
}

/**
 * Forgets a retained game error once the operator has seen it.
 *
 * A game whose task failed used to be pruned with the finished games, so a
 * board could die and be gone from every screen in the same second, with the
 * cause recorded nowhere the operator would look. Failed games now stay in the
 * snapshot until this call removes them (the backend caps the retained set at
 * 32 and evicts oldest-first), which means dismissal is the *only* way one
 * leaves — and the only thing that knows a failure has been read is the
 * operator.
 */
export function dismissGameError(gameId: string): Promise<void> {
  return invoke<void>("dismiss_game_error", { gameId });
}

export function startBot(accountId: string): Promise<void> {
  return invoke<void>("start_bot", { accountId });
}

export function stopBot(accountId: string): Promise<void> {
  return invoke<void>("stop_bot", { accountId });
}

export function createChallenge(request: ChallengeRequest): Promise<void> {
  return invoke<void>("create_challenge", { request });
}

export function startCampaign(settings: CampaignSettings): Promise<void> {
  return invoke<void>("start_campaign", { settings });
}

export function stopCampaign(accountId: string): Promise<void> {
  return invoke<void>("stop_campaign", { accountId });
}

export function writePgnFile(path: string, contents: string): Promise<void> {
  return invoke<void>("write_pgn_file", { path, contents });
}

export function getScorebookStats(
  filter: ScorebookFilter,
): Promise<ScorebookStats> {
  return invoke<ScorebookStats>("get_scorebook_stats", { filter });
}

export function importLichessHistory(
  accountId: string,
  max?: number,
): Promise<ImportReport> {
  return invoke<ImportReport>("import_lichess_history", { accountId, max });
}

export function listLogSessions(
  filter: LogFilter,
): Promise<LogSessionSummary[]> {
  return invoke<LogSessionSummary[]>("list_log_sessions", { filter });
}

export function getLogPage(
  sessionId: string,
  offset: number,
  limit: number,
): Promise<LogPage> {
  return invoke<LogPage>("get_log_page", { sessionId, offset, limit });
}

export function getLogOutline(sessionId: string): Promise<LogSearchBlock[]> {
  return invoke<LogSearchBlock[]>("get_log_outline", { sessionId });
}

export function searchLogSession(
  sessionId: string,
  query: LogQuery,
): Promise<LogMatch[]> {
  return invoke<LogMatch[]>("search_log_session", { sessionId, query });
}

export function searchLogSessions(
  filter: LogFilter,
  query: LogQuery,
): Promise<LogSessionMatches[]> {
  return invoke<LogSessionMatches[]>("search_log_sessions", { filter, query });
}

export function exportLogSession(
  sessionId: string,
  path: string,
  mode: ExportMode,
): Promise<void> {
  return invoke<void>("export_log_session", { sessionId, path, mode });
}

export function deleteLogSession(sessionId: string): Promise<void> {
  return invoke<void>("delete_log_session", { sessionId });
}

export function clearLogSessions(): Promise<number> {
  return invoke<number>("clear_log_sessions");
}

export function getLogsOverview(): Promise<LogsOverview> {
  return invoke<LogsOverview>("get_logs_overview");
}

export function setLogRetention(retention: LogRetention): Promise<void> {
  return invoke<void>("set_log_retention", { retention });
}

export function getDiagnostics(
  filter: DiagnosticFilter,
): Promise<DiagnosticEntry[]> {
  return invoke<DiagnosticEntry[]>("get_diagnostics", { filter });
}

export function clearDiagnostics(): Promise<void> {
  return invoke<void>("clear_diagnostics");
}

/** Closes the window after the operator accepted abandoning live games. */
export function confirmClose(): Promise<void> {
  return invoke<void>("confirm_close");
}
