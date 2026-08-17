import { listen } from "@tauri-apps/api/event";
import type {
  AppSnapshot,
  BackendNotificationEvent,
  BackendSnapshotEvent,
  DiagnosticEntry,
  RunnerConnectionEvent,
} from "../types";

/**
 * True when running inside the Tauri shell. A browser dev preview has no IPC
 * bridge, and `listen` dereferences it eagerly — subscribing there throws
 * rather than rejecting, so every subscription checks first and degrades to a
 * no-op instead of taking the page down.
 */
function hasTauriBridge() {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

const noop = () => {};

/**
 * Subscribe to a backend event.
 *
 * Race-safe: if the subscription is cancelled before the underlying `listen`
 * promise resolves (for example a StrictMode double-mount), the freshly
 * registered Tauri listener is released immediately instead of being orphaned,
 * and the callback is never invoked after cancellation.
 *
 * This body used to be copied once per event, with the contract restated in a
 * comment each time; the subtlety it encodes is now maintained in one place.
 */
export function subscribe<T>(
  event: string,
  callback: (payload: T) => void,
): () => void {
  if (!hasTauriBridge()) return noop;
  let active = true;
  let unlisten: (() => void) | undefined;
  void listen<T>(event, (message) => {
    if (active) callback(message.payload);
  }).then((cleanup) => {
    if (active) unlisten = cleanup;
    else cleanup();
  });
  return () => {
    active = false;
    unlisten?.();
    unlisten = undefined;
  };
}

export const SNAPSHOT_EVENT = "queenui://snapshot";

/**
 * Subscribe to backend snapshot events.
 *
 * Every core event is now an envelope stamped with the generation of the
 * backend that produced it (`BackendSnapshotEvent`, `emit_core_event` in
 * `src-tauri/src/lib.rs`). The stamp is not decoration: a live runner switch
 * publishes a *different* backend, and until wave 3 the payload of the old one
 * was indistinguishable from the new one's — which is how an unreachable remote
 * runner ended up presenting the previous runner's fleet as its own.
 *
 * The generation is handed to the caller rather than filtered here, because the
 * decision it feeds ("is this the backend this app is attached to, and do we
 * hold any of its data yet?") is exactly the connection state — see
 * `connectionReducer`, which is where the rule is stated once.
 */
export function onSnapshot(
  callback: (snapshot: AppSnapshot, backendGeneration: number) => void,
) {
  return subscribe<BackendSnapshotEvent>(SNAPSHOT_EVENT, (event) =>
    callback(event.payload, event.backendGeneration),
  );
}

export const LOGS_UPDATED_EVENT = "queenui://logs-updated";

/**
 * Subscribe to engine-log session changes (a session opened, closed, was
 * deleted, or retention pruned some).
 *
 * The envelope carries a backend generation too, and it is deliberately not
 * consulted: this event carries no data of its own — it says "read it again" —
 * and the read it triggers is dispatched to whichever backend is live when it
 * runs. A notification from a backend that has since been replaced can
 * therefore cost a redundant, *correct* re-read; it cannot present a retired
 * backend's data as current, which is the failure the stamp exists to stop.
 */
export function onLogsUpdated(callback: () => void) {
  return subscribe<BackendNotificationEvent>(LOGS_UPDATED_EVENT, () =>
    callback(),
  );
}

export const DIAGNOSTIC_EVENT = "queenui://diagnostic";

/**
 * Subscribe to individual app diagnostics as they are recorded, so the Logs
 * page can append without polling.
 */
export function onDiagnostic(callback: (entry: DiagnosticEntry) => void) {
  return subscribe<DiagnosticEntry>(DIAGNOSTIC_EVENT, callback);
}

export const CLOSE_REQUESTED_EVENT = "queenui://close-requested";

/**
 * The close-requested wire shape.
 *
 * Hand-written, unlike every other event payload, because the Rust
 * `CloseRequestedPayload` (`src-tauri/src/lib.rs`) carries no `ts_rs::TS`
 * derive, so the generator never emits it. The Rust side pins the JSON in a
 * test (`serde_json::to_value(CloseRequestedPayload { reported_count: 3 })`
 * equals `{"reportedCount": 3}`); this mirrors that and nothing else.
 */
type CloseRequestedPayload = { reportedCount: number };

/**
 * Subscribe to a blocked window close. The backend only asks when games are
 * still being played; the payload is how many, so the warning can be raised
 * even if the snapshot is momentarily behind.
 *
 * That count arrives as `{ reportedCount }` and used to be read as a bare
 * number, which is not a shape this event has ever had. The object was passed
 * through as the count, so `reportedCount > 0` was false and the guard could
 * only be raised by games the snapshot already showed — disabling the one
 * mitigation the payload exists for.
 */
export function onCloseRequested(callback: (liveGames: number) => void) {
  return subscribe<CloseRequestedPayload>(CLOSE_REQUESTED_EVENT, (payload) =>
    callback(payload.reportedCount),
  );
}

export const HISTORY_UPDATED_EVENT = "queenui://history-updated";

/**
 * Subscribe to backend game-history updates (a finished game was recorded or a
 * Lichess import completed). Generation-agnostic for the same reason
 * `onLogsUpdated` is: it carries no data, only the instruction to re-read.
 */
export function onHistoryUpdated(callback: () => void) {
  return subscribe<BackendNotificationEvent>(HISTORY_UPDATED_EVENT, () =>
    callback(),
  );
}

/**
 * Health of the remote runner's event stream.
 *
 * The backend emits this on every transition and every five seconds while
 * degraded. It is namespaced like every other event QueenUI listens to; the
 * previous event name was the unprefixed `runner-connection`.
 *
 * The event carries `backendGeneration` as of wave 3, so a link state can be
 * attributed to the backend it describes. It is the only event that announces a
 * *new* backend without carrying any of its data, which is what puts the app
 * into "connecting, nothing of this one has arrived yet".
 */
export const RUNNER_CONNECTION_EVENT = "queenui://runner-connection";

export function onRunnerConnection(
  callback: (connection: RunnerConnectionEvent) => void,
) {
  return subscribe<RunnerConnectionEvent>(RUNNER_CONNECTION_EVENT, callback);
}
