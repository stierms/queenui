import type { RunnerConnectionEvent, RunnerConnectionState } from "../types";

/**
 * Connection honesty.
 *
 * QueenUI models "loading" and "failed" everywhere, and used to model *stale*
 * nowhere — so a dead remote runner rendered as a live game whose clocks kept
 * counting down, which is the one failure mode this app must not have. This
 * module is the whole staleness model: a pure reducer over the two signals
 * that can actually prove something, plus the derived questions the UI asks.
 *
 * Deliberately *not* modelled: a "no snapshot for N seconds" timeout. Snapshots
 * are emitted on change, and an engine thinking for three minutes legitimately
 * emits none — a silence timer would invent disconnections that never happened,
 * which is the same dishonesty in the other direction. Staleness is claimed
 * only when the backend says the link is degraded, when the initial/retry
 * snapshot load fails, or when the backend that produced what is on screen has
 * been replaced (see `backendGeneration`). Ordinary action failures remain
 * operation-specific notices because an API rejection is not proof that
 * displayed data is stale.
 *
 * The third of those is wave 3's addition, and it closes a verified bug rather
 * than a hypothetical: every backend event now carries the generation of the
 * backend that produced it, because a live runner switch replaces the backend
 * without replacing what this app is showing. Switching to an unreachable remote
 * runner left the *previous* runner's accounts, engines and games on screen,
 * attributed to the new one, with the link banner the only hint that anything
 * was wrong. Data stamped with a retired generation is now dropped, and until
 * the current backend has delivered a snapshot of its own the app says it is
 * waiting rather than borrowing the last one.
 *
 * Wave 4 finished the job by stamping the one snapshot that had no stamp: the
 * `get_snapshot` *command result*, which was attributed to whichever backend was
 * live when it landed. Being a command result is not proof of currency — the call
 * is awaited across an IPC hop and a switch does not wait for it — and leaving it
 * unstamped also left the app at generation 0 whenever the fetch was the only
 * thing it had heard, which made the next backend look like a first one rather
 * than a replacement. There is one snapshot rule here now, and no exception to it.
 */

/**
 * `"unknown"` until the backend speaks.
 *
 * The embedded runner has no link to report and normally says nothing at all.
 * The one exception is `"embedded"`, emitted once when a live switch away from
 * a remote runner completes — the event that retires whatever that runner had
 * last claimed.
 */
export type RunnerLink = RunnerConnectionState | "unknown";

export type ConnectionState = {
  link: RunnerLink;
  /** Retry counter from the backend; 0 whenever the link is healthy. */
  attempt: number;
  /** Epoch ms of the last event that reached the backend from the runner. */
  lastOkAtMs: number | null;
  /** Backend-supplied cause of the degradation, when there is one. */
  detail: string | null;
  /** Epoch ms of the most recent snapshot this app actually applied. */
  lastSnapshotAtMs: number | null;
  /**
   * The QueenUI backend itself could not be reached. Distinct from a degraded
   * runner link: nothing at all is arriving, and every command will fail.
   */
  backendUnavailable: boolean;
  backendDetail: string | null;
  /**
   * The generation of the backend this app is attached to, as stamped on what it
   * receives; `0` until the first stamped message arrives, which is a number no
   * backend ever carries.
   *
   * `BackendState::next_generation` hands out a fresh number every time a
   * backend is published — a runner switch, a same-endpoint pairing that adopts
   * a rotated bearer, a recovery from the unavailable slot — and every event that
   * backend emits carries it, as does every `get_snapshot` answer it serves. It
   * is monotonic, so a lower number is always a message from a backend this app
   * has already left.
   */
  backendGeneration: number;
  /**
   * True when a different backend was published and *none of its data has
   * arrived yet*.
   *
   * This is the state the wrong-backend-snapshot bug lived in: a switch to an
   * unreachable remote runner left the previous runner's fleet on screen, with
   * every surface presenting it as the current runner's. There is nothing
   * truthful to show until the new backend's first snapshot lands, and this
   * says so rather than borrowing the last one.
   */
  awaitingBackendData: boolean;
};

export const initialConnectionState: ConnectionState = {
  link: "unknown",
  attempt: 0,
  lastOkAtMs: null,
  detail: null,
  lastSnapshotAtMs: null,
  backendUnavailable: false,
  backendDetail: null,
  backendGeneration: 0,
  awaitingBackendData: false,
};

export type ConnectionAction =
  /** A `runner-connection` event; authoritative about the link. */
  | { type: "runner-event"; payload: RunnerConnectionEvent }
  /**
   * A snapshot was applied — proof the backend is answering.
   *
   * `generation` is the stamp of the backend that produced it, and every
   * snapshot has one: the `get_snapshot` fetch answers with the same envelope
   * the event carries (`BackendSnapshotEvent`). It used to arrive unstamped and
   * was attributed to whichever backend was live when the response landed, which
   * is only sound while dispatch and delivery cannot straddle a switch — and a
   * live switch does not wait for an in-flight command. One stamp, one rule.
   */
  | { type: "snapshot"; atMs: number; generation: number }
  /** The initial/retry snapshot call rejected; the backend cannot be reached. */
  | { type: "backend-failed"; atMs: number; detail: string }
  /** A backend call succeeded without carrying a snapshot. */
  | { type: "backend-ok" };

function sameState(a: ConnectionState, b: ConnectionState) {
  return (
    a.link === b.link &&
    a.attempt === b.attempt &&
    a.lastOkAtMs === b.lastOkAtMs &&
    a.detail === b.detail &&
    a.lastSnapshotAtMs === b.lastSnapshotAtMs &&
    a.backendUnavailable === b.backendUnavailable &&
    a.backendDetail === b.backendDetail &&
    a.backendGeneration === b.backendGeneration &&
    a.awaitingBackendData === b.awaitingBackendData
  );
}

/**
 * True when a stamped message describes a backend this app has already left —
 * an event, or a `get_snapshot` response that outlived its dispatch.
 *
 * The one rule that makes a retired backend's data harmless. Exported because
 * it is asked twice about the same message — once about the connection state
 * below, and once by `useSnapshot` about the payload itself, which must not be
 * applied either.
 */
export function isStaleGeneration(
  state: ConnectionState,
  generation: number,
): boolean {
  return generation < state.backendGeneration;
}

/**
 * Attaches the state to a newly published backend, dropping every claim the
 * previous one made.
 *
 * Same reasoning as the `embedded` link event below, generalized: `attempt`
 * counted retries against a backend this app no longer talks to, `lastOkAtMs`
 * timestamped that backend's last word, and `detail` named a degradation of its
 * link. None of them describe the backend now in place. `lastSnapshotAtMs` is
 * this app's own fact — when it last applied *anything* — and `hasNoData` reads
 * it to tell a failed first load from a later failure, so it survives.
 */
function attachTo(state: ConnectionState, generation: number): ConnectionState {
  return {
    ...initialConnectionState,
    lastSnapshotAtMs: state.lastSnapshotAtMs,
    backendGeneration: generation,
    /*
     * A first generation is not a *change* of backend. `0` is this app's "no
     * backend has spoken yet" sentinel and never a number any backend carries —
     * `next_generation` hands out 1 first — so the first stamp this app sees,
     * whether it arrives on an event or on the initial `get_snapshot` response,
     * is the backend it was loading from all along. Calling that a handover would
     * declare "nothing has arrived from it" about the very data delivering the
     * stamp. Only a generation that replaces a known one means this app holds
     * another backend's data.
     */
    awaitingBackendData: state.backendGeneration > 0,
  };
}

/**
 * Folds one signal into the connection state.
 *
 * Returns the *identical* object when nothing changed. The backend re-emits the
 * connection event every five seconds while degraded, and a fresh object each
 * time would re-render every board on screen for no new information.
 */
export function connectionReducer(
  state: ConnectionState,
  action: ConnectionAction,
): ConnectionState {
  const next = reduce(state, action);
  return sameState(state, next) ? state : next;
}

function reduce(
  state: ConnectionState,
  action: ConnectionAction,
): ConnectionState {
  switch (action.type) {
    case "runner-event": {
      const {
        backendGeneration,
        state: link,
        attempt,
        lastOkTs,
        detail,
      } = action.payload;
      /*
       * A link state from a backend this app has already left. The backend
       * cancels a replaced backend's event task, but cancellation and delivery
       * race, so the last words of a runner that is no longer this app's can
       * still arrive — and folding them in would re-degrade a healthy new
       * backend, or clear a real degradation of it.
       */
      if (isStaleGeneration(state, backendGeneration)) return state;
      /*
       * The one event that announces a new backend while carrying none of its
       * data. Everything the old one claimed is retired, and the app now holds
       * nothing that belongs to what is running.
       */
      const attached =
        backendGeneration > state.backendGeneration
          ? attachTo(state, backendGeneration)
          : state;
      /*
       * The one signal that ends a degradation instead of describing one: the
       * runner it was about is no longer this app's backend, so the whole
       * remote link is retired rather than folded in. `set_runner_settings`
       * emits it after a remote-to-embedded switch has actually landed, which
       * is why it may clear what a *snapshot* must not (see below).
       *
       * Nothing the old runner reported survives: `attempt` counted retries
       * against a runner this app no longer talks to, and `lastOkAtMs`
       * timestamped that runner's last word — which the banner would render as
       * "Last update 14:02" over a local engine that was never late. Payload
       * fields are ignored for the same reason rather than trusted: no value of
       * them describes an embedded runner.
       *
       * `lastSnapshotAtMs` is not the runner's fact but this app's own — when it
       * last applied a snapshot — and `hasNoData` reads it to tell a failed
       * first load from a later failure, so the switch leaves it alone.
       */
      if (link === "embedded") {
        return {
          ...initialConnectionState,
          link,
          lastSnapshotAtMs: attached.lastSnapshotAtMs,
          backendGeneration: attached.backendGeneration,
          awaitingBackendData: attached.awaitingBackendData,
        };
      }
      return {
        ...attached,
        link,
        attempt: link === "connected" ? 0 : attempt,
        // A degraded event may not know when the link was last healthy; the
        // value we already hold is still the best answer in that case — unless
        // it belonged to the backend this event just replaced, which `attachTo`
        // has already dropped.
        lastOkAtMs: lastOkTs ?? attached.lastOkAtMs,
        detail: link === "connected" ? null : detail,
        // Runner events arrive over the backend IPC bridge, so receiving one
        // is itself proof the backend is up.
        backendUnavailable: false,
        backendDetail: null,
      };
    }
    case "snapshot": {
      const { generation } = action;
      // Data from a backend this app has left, which is precisely the payload
      // that used to be presented as the current runner's.
      if (isStaleGeneration(state, generation)) return state;
      /*
       * A snapshot can be the *first* thing a new backend says: the
       * remote-to-embedded switch emits the new backend's snapshot before its
       * connection event, on purpose, so that truthful same-generation data is
       * already in place when the link state lands ("Truthful same-generation
       * data lands before the connection state can clear frontend staleness",
       * `set_runner_settings_inner`). Adopting the generation here is what makes
       * that ordering work instead of blanking the data it just delivered.
       *
       * It also retires the previous backend's link claim — not a weakening of
       * the rule below but the same rule read precisely: what a *same*
       * generation snapshot cannot do is clear a degradation of its own link.
       * A degradation of a backend that no longer exists is not this one's.
       */
      const attached =
        generation > state.backendGeneration
          ? attachTo(state, generation)
          : state;
      return {
        ...attached,
        lastSnapshotAtMs: action.atMs,
        backendUnavailable: false,
        backendDetail: null,
        // The arrival that ends "a different backend is running and nothing of
        // it has reached this app".
        awaitingBackendData: false,
        // A snapshot does NOT clear a degraded link: in remote mode the
        // backend keeps serving its last known state, and the connection
        // event is the only thing that knows whether it is current.
      };
    }
    case "backend-ok":
      return { ...state, backendUnavailable: false, backendDetail: null };
    case "backend-failed":
      return {
        ...state,
        backendUnavailable: true,
        backendDetail: action.detail,
      };
  }
}

/** True when what is on screen may no longer match reality. */
export function isStale(state: ConnectionState) {
  return (
    state.backendUnavailable ||
    // Nothing on screen belongs to the backend that is running, which is the
    // strongest form of "does not match reality" there is.
    state.awaitingBackendData ||
    state.link === "reconnecting" ||
    state.link === "disconnected"
  );
}

/**
 * True when the app has never managed to load anything: the backend is
 * unreachable and no snapshot was ever applied. This is the state that used to
 * render the first-run onboarding screen.
 *
 * Not the same question as `awaitingBackendData`, and the two must not be
 * merged: this one says the QueenUI backend never answered, so every command
 * will fail too; that one says the backend is answering as a *different*
 * generation whose first snapshot has not arrived. Both leave the screen with
 * nothing to show, for reasons that need opposite sentences.
 */
export function hasNoData(state: ConnectionState) {
  return state.backendUnavailable && state.lastSnapshotAtMs === null;
}

export type ConnectionSummary = {
  /** `null` when everything is healthy and no banner should render. */
  headline: string | null;
  detail: string | null;
  tone: "warning" | "error";
};

/**
 * Banner copy for the current state. Names the consequence rather than the
 * mechanism, in the house style of the close guard.
 */
export function connectionSummary(state: ConnectionState): ConnectionSummary {
  if (state.backendUnavailable) {
    return {
      headline: "Can't reach the QueenUI backend",
      detail:
        state.backendDetail ??
        "Commands will fail until it answers again. Anything shown is the last state it reported.",
      tone: "error",
    };
  }
  /*
   * Before the link branches, and not because it matters less: with no data
   * from this backend at all, "boards and clocks below are frozen at the
   * runner's last update" would describe boards that are not on screen — and
   * whose last update came from a different runner. What is true is that the
   * screen is empty on purpose.
   */
  if (state.awaitingBackendData) {
    return {
      headline: "Waiting for the game runner",
      detail:
        state.detail ??
        "QueenUI is dispatching to a different runner now and nothing has arrived from it yet. Whatever was on screen belonged to the previous one, so it is not being shown as current.",
      tone: state.link === "disconnected" ? "error" : "warning",
    };
  }
  if (state.link === "disconnected") {
    return {
      headline: "Disconnected from the game runner",
      detail:
        state.detail ??
        "Boards and clocks below are frozen at the runner's last update. Games on Lichess keep playing.",
      tone: "error",
    };
  }
  if (state.link === "reconnecting") {
    return {
      headline:
        state.attempt > 1
          ? `Reconnecting to the game runner (attempt ${state.attempt})`
          : "Reconnecting to the game runner",
      detail:
        state.detail ??
        "Boards and clocks below are frozen at the runner's last update. Games on Lichess keep playing.",
      tone: "warning",
    };
  }
  return { headline: null, detail: null, tone: "warning" };
}
