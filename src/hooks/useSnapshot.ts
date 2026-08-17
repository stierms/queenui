import { useCallback, useEffect, useReducer, useRef, useState } from "react";
import { getSnapshot } from "../api/commands";
import { onRunnerConnection, onSnapshot } from "../api/events";
import { errorText } from "../lib/errors";
import {
  connectionReducer,
  hasNoData,
  initialConnectionState,
  isStale,
  isStaleGeneration,
  type ConnectionAction,
  type ConnectionState,
} from "../lib/connection";
import { emptySnapshot, type AppSnapshot } from "../types";

export type SnapshotFeed = {
  /** The last snapshot successfully applied — the last *good* state, always. */
  snapshot: AppSnapshot;
  /** The very first fetch is still in flight. */
  loading: boolean;
  /** Link and backend health; drives every stale/frozen affordance. */
  connection: ConnectionState;
  /** Shorthand: what is on screen may no longer match reality. */
  stale: boolean;
  /**
   * Nothing was ever loaded and the backend is unreachable. The screen must
   * say so; it must not fall through to the first-run onboarding flow.
   */
  unavailable: boolean;
  /**
   * A different backend is running and none of its data has arrived yet. The
   * screen is empty because the previous backend's data was dropped, not
   * because nothing is configured — so this must not reach onboarding either.
   */
  awaitingBackend: boolean;
  /** Re-run the initial fetch after a failure. */
  retry: () => void;
};

/**
 * The snapshot and the connection state, folded by one reducer.
 *
 * They used to be separate (`useState` for the snapshot, `useReducer` for the
 * connection), which is fine until the two have to agree: a snapshot stamped
 * with a retired backend generation must be dropped *and* leave the connection
 * state untouched, and a connection event announcing a new backend must clear
 * the snapshot it invalidates. Neither decision can be made from inside an
 * event callback that closes over last render's state, and two dispatches
 * cannot coordinate. One reducer, one place the rule is applied.
 */
type Feed = { snapshot: AppSnapshot; connection: ConnectionState };

type FeedAction =
  // Every connection signal unchanged, minus the snapshot action, which the
  // feed restates below with the payload the reducer above never needed.
  | Exclude<ConnectionAction, { type: "snapshot" }>
  /**
   * A snapshot, stamped by the backend that produced it. One shape for both
   * sources now: the `get_snapshot` response arrives in the same stamped
   * envelope the event does, so there is no longer an unstamped case to
   * attribute to whatever happens to be live.
   *
   * `apply` is the one thing that still distinguishes them, and it is a
   * freshness question rather than an attribution one: the fetch passes `false`
   * once an event has already been applied, because a slow response must not
   * clobber fresher event data — though it is still proof the backend answered.
   * An event, which is by definition the backend's latest word, omits it.
   */
  | {
      type: "snapshot";
      atMs: number;
      generation: number;
      snapshot: AppSnapshot;
      apply?: boolean;
    };

function feedReducer(state: Feed, action: FeedAction): Feed {
  const connection = connectionReducer(state.connection, action);
  if (action.type !== "snapshot") {
    /*
     * An invariant, not a transition: while a backend is running that this app
     * holds no data for, there is no snapshot. What it held belongs to the
     * previous one and every surface would present it as the new runner's —
     * which is the wrong-backend-snapshot bug — so it goes.
     */
    const snapshot = connection.awaitingBackendData
      ? emptySnapshot
      : state.snapshot;
    return connection === state.connection && snapshot === state.snapshot
      ? state
      : { snapshot, connection };
  }
  // Stamped with a generation this app has already left: neither the payload nor
  // the timestamp says anything about the backend that is running. True of a
  // late `get_snapshot` response as much as of a late event — a switch does not
  // wait for an in-flight command, so the answer to one can outlive its backend.
  if (isStaleGeneration(state.connection, action.generation)) return state;
  /*
   * "An event already overtook this fetch" only settles which is fresher while
   * both came from the same backend. A response stamped *newer* than the event
   * that overtook it is the newer backend's own state, and skipping it would
   * leave the connection attached to that generation while the snapshot on
   * screen belongs to the previous one — the wrong-backend-snapshot bug, from
   * the other side.
   */
  const apply =
    action.generation > state.connection.backendGeneration ||
    (action.apply ?? true);
  const snapshot = apply ? action.snapshot : state.snapshot;
  return connection === state.connection && snapshot === state.snapshot
    ? state
    : { snapshot, connection };
}

/**
 * Live application snapshot: subscribes to backend snapshot events and seeds
 * the state with an initial `get_snapshot` fetch.
 *
 * The initial fetch is ignored once any event from the same backend has been
 * applied, so a slow fetch can never clobber fresher event data.
 *
 * The fetch is stamped like every event, so it is subject to the same generation
 * rule: a response produced by a backend this app has already left is dropped
 * rather than attributed to the one that replaced it, and the *first* response is
 * how the app adopts its starting generation when no event has arrived yet.
 *
 * On failure the hook keeps the last snapshot it had rather than emptying it,
 * and reports the failure through `connection` instead of only firing a
 * five-second toast — an empty snapshot is indistinguishable from a fresh
 * install, and used to be rendered as one.
 *
 * The one case where it *does* empty the snapshot is a backend generation
 * change: that data is not this backend's, and keeping it would be the same
 * dishonesty in a subtler form. `awaitingBackend` is how the screen says so.
 */
export function useSnapshot(
  enabled: boolean,
  initial: AppSnapshot,
  onLoadError: (error: unknown) => void,
): SnapshotFeed {
  const [loading, setLoading] = useState(enabled);
  const [attempt, setAttempt] = useState(0);
  const [{ snapshot, connection }, dispatch] = useReducer(feedReducer, {
    snapshot: initial,
    connection: initialConnectionState,
  });
  const onLoadErrorRef = useRef(onLoadError);
  useEffect(() => {
    onLoadErrorRef.current = onLoadError;
  }, [onLoadError]);

  useEffect(() => {
    if (!enabled) return;
    let mounted = true;
    let eventApplied = false;
    const unsubscribeSnapshot = onSnapshot((value, generation) => {
      eventApplied = true;
      dispatch({
        type: "snapshot",
        atMs: Date.now(),
        generation,
        snapshot: value,
      });
    });
    const unsubscribeConnection = onRunnerConnection((payload) =>
      dispatch({ type: "runner-event", payload }),
    );
    getSnapshot()
      .then(({ backendGeneration, payload }) => {
        if (!mounted) return;
        dispatch({
          type: "snapshot",
          atMs: Date.now(),
          generation: backendGeneration,
          snapshot: payload,
          apply: !eventApplied,
        });
      })
      .catch((error) => {
        // Keep the raw failure inspectable; callers surface a humanized copy.
        console.error("Initial get_snapshot failed:", error);
        if (!mounted) return;
        dispatch({
          type: "backend-failed",
          atMs: Date.now(),
          detail: errorText(error),
        });
        onLoadErrorRef.current(error);
      })
      .finally(() => {
        if (mounted) setLoading(false);
      });
    return () => {
      mounted = false;
      unsubscribeSnapshot();
      unsubscribeConnection();
    };
  }, [enabled, attempt]);

  const retry = useCallback(() => {
    setLoading(true);
    setAttempt((value) => value + 1);
  }, []);

  return {
    snapshot,
    loading,
    connection,
    stale: isStale(connection),
    unavailable: hasNoData(connection),
    awaitingBackend: connection.awaitingBackendData,
    retry,
  };
}
