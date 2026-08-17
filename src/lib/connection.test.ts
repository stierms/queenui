import { describe, expect, it } from "vitest";
import {
  connectionReducer,
  connectionSummary,
  hasNoData,
  initialConnectionState,
  isStale,
  type ConnectionState,
} from "./connection";
import type { RunnerConnectionEvent } from "../types";

/**
 * A connection event from generation 1 unless the test says otherwise, which is
 * every test below that predates generations: one backend, one generation, and
 * the invariants they pin are the ones that hold *within* it.
 */
function event(
  payload: Partial<RunnerConnectionEvent> = {},
): RunnerConnectionEvent {
  return {
    backendGeneration: 1,
    state: "reconnecting",
    attempt: 1,
    lastOkTs: null,
    detail: null,
    ...payload,
  };
}

/**
 * A snapshot from the same generation those events describe.
 *
 * One helper for both sources on purpose: the `get_snapshot` response arrives in
 * the same stamped envelope the event does, so the reducer has one snapshot rule
 * rather than a stamped one and an unstamped special case.
 */
function snapshot(atMs: number, generation = 1) {
  return { type: "snapshot", atMs, generation } as const;
}

function fold(actions: Parameters<typeof connectionReducer>[1][]) {
  return actions.reduce(connectionReducer, initialConnectionState);
}

describe("connection reducer", () => {
  it("starts with no claim about the link", () => {
    expect(initialConnectionState.link).toBe("unknown");
    expect(isStale(initialConnectionState)).toBe(false);
    expect(hasNoData(initialConnectionState)).toBe(false);
    expect(connectionSummary(initialConnectionState).headline).toBeNull();
  });

  it("never claims staleness in embedded mode, where no runner events arrive", () => {
    // Snapshots alone must not synthesize a link state: an engine thinking for
    // three minutes emits none, and a silence timer would invent an outage.
    const state = fold([snapshot(1_000), snapshot(400_000)]);
    expect(state.link).toBe("unknown");
    expect(isStale(state)).toBe(false);
    expect(state.lastSnapshotAtMs).toBe(400_000);
  });

  it("goes stale on a reconnecting event and recovers on a connected one", () => {
    const degraded = fold([
      snapshot(1_000),
      {
        type: "runner-event",
        payload: event({ attempt: 2, lastOkTs: 900, detail: "stream closed" }),
      },
    ]);
    expect(degraded.link).toBe("reconnecting");
    expect(degraded.attempt).toBe(2);
    expect(degraded.lastOkAtMs).toBe(900);
    expect(isStale(degraded)).toBe(true);

    const recovered = connectionReducer(degraded, {
      type: "runner-event",
      payload: event({ state: "connected", attempt: 0, lastOkTs: 5_000 }),
    });
    expect(recovered.link).toBe("connected");
    expect(recovered.attempt).toBe(0);
    expect(recovered.detail).toBeNull();
    expect(isStale(recovered)).toBe(false);
  });

  it("treats disconnected as stale too", () => {
    const state = connectionReducer(initialConnectionState, {
      type: "runner-event",
      payload: event({ state: "disconnected", attempt: 9 }),
    });
    expect(isStale(state)).toBe(true);
    expect(connectionSummary(state).tone).toBe("error");
    expect(connectionSummary(state).headline).toMatch(/Disconnected/);
  });

  it("keeps the last known good timestamp when a degraded event omits it", () => {
    const state = fold([
      {
        type: "runner-event",
        payload: event({ state: "connected", lastOkTs: 7_000 }),
      },
      { type: "runner-event", payload: event({ lastOkTs: null }) },
    ]);
    expect(state.lastOkAtMs).toBe(7_000);
  });

  it("returns the identical object for a repeated degraded event", () => {
    // The backend re-emits every five seconds while degraded; a fresh object
    // each time would re-render every board on screen for no new information.
    const degraded = connectionReducer(initialConnectionState, {
      type: "runner-event",
      payload: event({ attempt: 3, detail: "connection refused" }),
    });
    const again = connectionReducer(degraded, {
      type: "runner-event",
      payload: event({ attempt: 3, detail: "connection refused" }),
    });
    expect(again).toBe(degraded);
  });

  it("returns to the embedded ground state when a live switch lands", () => {
    // `set_runner_settings` emits this once a remote-to-embedded switch has
    // actually completed. The remote runner is not this app's backend any more,
    // so every claim about that link — including the banner and every frozen
    // affordance derived from it — has to go.
    const state = fold([
      snapshot(1_000),
      {
        type: "runner-event",
        payload: event({ attempt: 4, lastOkTs: 900, detail: "stream closed" }),
      },
      {
        type: "runner-event",
        payload: event({ state: "embedded", attempt: 0 }),
      },
    ]);
    expect(state.link).toBe("embedded");
    expect(state.attempt).toBe(0);
    expect(state.detail).toBeNull();
    // Not 900: that timestamped the last word of a runner this app no longer
    // talks to, and the banner renders it as "Last update …".
    expect(state.lastOkAtMs).toBeNull();
    expect(isStale(state)).toBe(false);
    expect(connectionSummary(state).headline).toBeNull();
    // This app's own fact about what it has loaded, not the runner's.
    expect(state.lastSnapshotAtMs).toBe(1_000);
  });

  it("clears a disconnected link and an unreachable backend on the same event", () => {
    for (const link of ["reconnecting", "disconnected"] as const) {
      const state = fold([
        { type: "runner-event", payload: event({ state: link, attempt: 7 }) },
        { type: "backend-failed", atMs: 10, detail: "ipc bridge missing" },
        { type: "runner-event", payload: event({ state: "embedded" }) },
      ]);
      expect(state.link).toBe("embedded");
      expect(state.backendUnavailable).toBe(false);
      expect(state.backendDetail).toBeNull();
      expect(isStale(state)).toBe(false);
      expect(hasNoData(state)).toBe(false);
      expect(connectionSummary(state).headline).toBeNull();
    }
  });

  it("ignores what an embedded event carries, since no value of it is true", () => {
    // A retry count and a degradation detail describe a link the embedded
    // runner does not have. Folding them in would let a malformed event
    // resurrect the banner the switch just earned the right to clear.
    const state = connectionReducer(initialConnectionState, {
      type: "runner-event",
      payload: event({
        state: "embedded",
        attempt: 3,
        lastOkTs: 5_000,
        detail: "stream closed",
      }),
    });
    expect(state.attempt).toBe(0);
    expect(state.detail).toBeNull();
    expect(state.lastOkAtMs).toBeNull();
    expect(isStale(state)).toBe(false);
  });

  it("does not date a later failure from the runner it stopped using", () => {
    /*
     * Where keeping the old `lastOkAtMs` actually shows: the banner ages the
     * screen from `lastOkAtMs ?? lastSnapshotAtMs`. Carry the retired remote
     * runner's timestamp past the switch and the next backend failure reports
     * "Last update" from a machine this app has not spoken to since — instead
     * of from the last snapshot it really applied.
     */
    const state = fold([
      {
        type: "runner-event",
        payload: event({ state: "connected", lastOkTs: 1_000 }),
      },
      snapshot(2_000),
      { type: "runner-event", payload: event({ state: "embedded" }) },
      snapshot(3_000),
      { type: "backend-failed", atMs: 4_000, detail: "ipc bridge missing" },
    ]);
    expect(state.lastOkAtMs).toBeNull();
    expect(state.lastSnapshotAtMs).toBe(3_000);
    expect(connectionSummary(state).headline).toBe(
      "Can't reach the QueenUI backend",
    );
  });

  it("returns the identical object for a repeated embedded event", () => {
    const embedded = connectionReducer(initialConnectionState, {
      type: "runner-event",
      payload: event({ state: "embedded", attempt: 0 }),
    });
    const again = connectionReducer(embedded, {
      type: "runner-event",
      payload: event({ state: "embedded", attempt: 0 }),
    });
    expect(again).toBe(embedded);
  });

  it("does not let a snapshot clear a degraded link", () => {
    // In remote mode the backend keeps serving its last known state, so a
    // snapshot proves the backend is up — not that the runner is reachable.
    const state = fold([
      { type: "runner-event", payload: event({ attempt: 4 }) },
      snapshot(12_000),
    ]);
    expect(state.link).toBe("reconnecting");
    expect(state.attempt).toBe(4);
    expect(state.lastSnapshotAtMs).toBe(12_000);
    expect(isStale(state)).toBe(true);
  });

  it("still refuses to let a snapshot clear any degraded link, switch event or not", () => {
    /*
     * The guard rail on the embedded event above. Both signals now arrive
     * together when a live switch lands — the connection event, then a fresh
     * snapshot from the new backend — and only the *event* is allowed to end a
     * degradation. A snapshot that could do it would resurrect the original
     * dishonesty: a dead remote runner's boards rendered as live, on the
     * strength of the backend answering with its own last known state.
     */
    for (const link of ["reconnecting", "disconnected"] as const) {
      const state = fold([
        { type: "runner-event", payload: event({ state: link, attempt: 4 }) },
        snapshot(12_000),
        { type: "backend-ok" },
        snapshot(13_000),
      ]);
      expect(state.link).toBe(link);
      expect(state.attempt).toBe(4);
      expect(isStale(state)).toBe(true);
      expect(connectionSummary(state).headline).not.toBeNull();
    }
  });

  it("reports an unreachable backend, and distinguishes it from a bad link", () => {
    const dead = connectionReducer(initialConnectionState, {
      type: "backend-failed",
      atMs: 100,
      detail: "ipc bridge missing",
    });
    expect(dead.backendUnavailable).toBe(true);
    expect(isStale(dead)).toBe(true);
    // Nothing was ever loaded: the screen must say so, not offer onboarding.
    expect(hasNoData(dead)).toBe(true);
    expect(connectionSummary(dead).headline).toMatch(/backend/i);
    expect(connectionSummary(dead).detail).toBe("ipc bridge missing");
  });

  it("stops reporting no-data once any snapshot has been applied", () => {
    const state = fold([
      snapshot(500),
      { type: "backend-failed", atMs: 900, detail: "gone" },
    ]);
    expect(state.backendUnavailable).toBe(true);
    expect(hasNoData(state)).toBe(false);
  });

  it("clears an unreachable backend when it answers again", () => {
    const recovered = fold([
      { type: "backend-failed", atMs: 1, detail: "gone" },
      { type: "backend-ok" },
    ]);
    expect(recovered.backendUnavailable).toBe(false);
    expect(recovered.backendDetail).toBeNull();
    expect(isStale(recovered)).toBe(false);
  });

  it("counts a runner event as proof the backend itself is alive", () => {
    const state = fold([
      { type: "backend-failed", atMs: 1, detail: "gone" },
      { type: "runner-event", payload: event({ state: "connected" }) },
    ]);
    expect(state.backendUnavailable).toBe(false);
    expect(isStale(state)).toBe(false);
  });

  it("names the attempt count only once retrying has actually repeated", () => {
    const first: ConnectionState = connectionReducer(initialConnectionState, {
      type: "runner-event",
      payload: event({ attempt: 1 }),
    });
    expect(connectionSummary(first).headline).toBe(
      "Reconnecting to the game runner",
    );
    const later = connectionReducer(first, {
      type: "runner-event",
      payload: event({ attempt: 6 }),
    });
    expect(connectionSummary(later).headline).toBe(
      "Reconnecting to the game runner (attempt 6)",
    );
  });
});

describe("backend generations", () => {
  it("adopts the first generation it sees without calling itself out of date", () => {
    // Generation 1 is not a handover: the initial `get_snapshot` fetch is
    // already loading from that backend, so declaring "nothing has arrived from
    // it" would blank the very data it is about.
    const state = fold([
      { type: "runner-event", payload: event({ state: "connected" }) },
    ]);
    expect(state.backendGeneration).toBe(1);
    expect(state.awaitingBackendData).toBe(false);
    expect(isStale(state)).toBe(false);
  });

  it("ignores a snapshot stamped with a generation it has already left", () => {
    /*
     * The wrong-backend-snapshot bug, at the reducer. A replaced backend's event
     * task is cancelled, but cancellation and delivery race, so the last
     * snapshot of a runner this app no longer talks to can still arrive — and it
     * used to be applied, timestamped as current, and rendered as the new
     * runner's fleet.
     */
    const current = fold([
      snapshot(1_000),
      { type: "runner-event", payload: event({ backendGeneration: 2 }) },
      snapshot(2_000, 2),
    ]);
    expect(current.backendGeneration).toBe(2);
    expect(current.lastSnapshotAtMs).toBe(2_000);

    const late = connectionReducer(current, snapshot(3_000, 1));
    // The identical object: nothing about the stale generation is news.
    expect(late).toBe(current);
    expect(late.lastSnapshotAtMs).toBe(2_000);
  });

  it("ignores a link state from a generation it has already left", () => {
    // The same race in the other event: the retired runner's "disconnected"
    // must not degrade the backend that replaced it.
    const healthy = fold([
      { type: "runner-event", payload: event({ state: "connected" }) },
      snapshot(1_000),
      {
        type: "runner-event",
        payload: event({ backendGeneration: 2, state: "connected" }),
      },
      snapshot(2_000, 2),
    ]);
    const late = connectionReducer(healthy, {
      type: "runner-event",
      payload: event({
        backendGeneration: 1,
        state: "disconnected",
        attempt: 9,
        detail: "runner process exited",
      }),
    });
    expect(late).toBe(healthy);
    expect(late.link).toBe("connected");
    expect(isStale(late)).toBe(false);
  });

  it("has no data for a backend announced by a connection event alone", () => {
    /*
     * The switch to an unreachable remote runner: the new backend's event loop
     * reports the link and nothing else, so there is no snapshot of its own —
     * and the one on screen belongs to the runner before it.
     */
    const state = fold([
      snapshot(1_000),
      { type: "runner-event", payload: event({ state: "connected" }) },
      {
        type: "runner-event",
        payload: event({
          backendGeneration: 2,
          state: "disconnected",
          attempt: 1,
          detail: "connection refused",
        }),
      },
    ]);
    expect(state.awaitingBackendData).toBe(true);
    expect(isStale(state)).toBe(true);
    // The new backend's own account of the link survives; the old one's does not.
    expect(state.link).toBe("disconnected");
    expect(state.detail).toBe("connection refused");
    expect(state.attempt).toBe(1);
    /*
     * Not the frozen-boards sentence: there are no boards on screen to be frozen
     * and none of them would be this runner's. And not `hasNoData`, which is the
     * unreachable-*backend* state where every command fails too.
     */
    const summary = connectionSummary(state);
    expect(summary.headline).toBe("Waiting for the game runner");
    expect(summary.detail).toBe("connection refused");
    expect(summary.tone).toBe("error");
    expect(hasNoData(state)).toBe(false);
  });

  it("stops waiting on the first snapshot the new backend sends", () => {
    const state = fold([
      snapshot(1_000),
      { type: "runner-event", payload: event({ backendGeneration: 2 }) },
      snapshot(5_000, 2),
    ]);
    expect(state.awaitingBackendData).toBe(false);
    expect(state.lastSnapshotAtMs).toBe(5_000);
    expect(connectionSummary(state).headline).not.toBe(
      "Waiting for the game runner",
    );
  });

  it("keeps waiting while the runner it switched to says nothing", () => {
    // Repeated degradation of the *new* backend is not data from it.
    const state = fold([
      snapshot(1_000),
      { type: "runner-event", payload: event({ state: "connected" }) },
      { type: "runner-event", payload: event({ backendGeneration: 2 }) },
      { type: "runner-event", payload: event({ backendGeneration: 2 }) },
      { type: "backend-ok" },
    ]);
    expect(state.awaitingBackendData).toBe(true);
    expect(isStale(state)).toBe(true);
  });

  it("adopts the generation the fetch response is stamped with", () => {
    /*
     * The bootstrap, and the hole the un-stamped fetch left in it.
     *
     * `get_snapshot` used to carry no generation and be attributed to whatever
     * backend was live when it landed, which left `backendGeneration` at 0 — the
     * "nobody has spoken yet" sentinel. An embedded backend emits no events until
     * something changes, so the fetch could be the only thing this app had ever
     * heard, and the *next* generation then looked like a first one: `attachTo`
     * declines to call a first generation a handover, so the switch kept the
     * previous backend's fleet on screen and attributed it to the runner now
     * being dispatched to. Exactly the bug the stamp exists to stop, reached
     * through the one path that had no stamp.
     *
     * With the response stamped, the fetch is how the app learns which backend it
     * is attached to, and the switch after it is a replacement of a known one.
     */
    const fetched = fold([snapshot(1_000)]);
    expect(fetched.backendGeneration).toBe(1);
    // Not a handover: this is the backend the fetch was already loading from.
    expect(fetched.awaitingBackendData).toBe(false);
    expect(fetched.lastSnapshotAtMs).toBe(1_000);
    expect(isStale(fetched)).toBe(false);

    const switched = connectionReducer(fetched, {
      type: "runner-event",
      payload: event({
        backendGeneration: 2,
        state: "disconnected",
        detail: "connection refused",
      }),
    });
    expect(switched.awaitingBackendData).toBe(true);
    expect(isStale(switched)).toBe(true);
    expect(connectionSummary(switched).headline).toBe(
      "Waiting for the game runner",
    );
  });

  it("rejects a fetch response that outlived the backend it was dispatched to", () => {
    /*
     * The other half of stamping it: a command is dispatched to whichever backend
     * is live at that moment, but it is awaited across an IPC hop and possibly a
     * remote round trip, and a live switch does not wait for it. So the response
     * can describe a backend this app has already left — and being a *command*
     * result was precisely the reason it used to be trusted as current.
     *
     * The switch here is announced by a connection event alone, which is the
     * unreachable-runner case: there is no newer snapshot to hide the mistake
     * behind, so applying the late response would put the retired runner's fleet
     * on screen and stop the app saying it is waiting.
     */
    const current = fold([
      snapshot(1_000),
      {
        type: "runner-event",
        payload: event({ backendGeneration: 2, state: "connected" }),
      },
    ]);
    expect(current.awaitingBackendData).toBe(true);

    const late = connectionReducer(current, snapshot(2_000, 1));

    // The identical object: nothing about the stale generation is news.
    expect(late).toBe(current);
    expect(late.awaitingBackendData).toBe(true);
    expect(late.lastSnapshotAtMs).toBe(1_000);
  });

  it("still lets the retry fetch recover the waiting screen", () => {
    /*
     * The reason the retry works at all, and what stamping does not take away: a
     * command reaches the backend that is live, so its response is stamped with
     * that generation and adopts it — rather than waiting on a subscription an
     * idle embedded backend has nothing to say on.
     */
    const state = fold([
      snapshot(1_000),
      { type: "runner-event", payload: event({ backendGeneration: 4 }) },
      snapshot(2_000, 4),
    ]);
    expect(state.awaitingBackendData).toBe(false);
    expect(state.backendGeneration).toBe(4);
    expect(state.lastSnapshotAtMs).toBe(2_000);
  });

  it("lets a new generation's snapshot retire the previous one's degradation", () => {
    /*
     * The ordering the backend chose deliberately: a remote-to-embedded switch
     * emits the new backend's snapshot *before* its connection event, so that
     * truthful same-generation data is in place when the link state lands. The
     * degradation being cleared belongs to a backend that no longer exists —
     * which is not the rule below, where a snapshot may never clear a
     * degradation of its *own* link.
     */
    const state = fold([
      {
        type: "runner-event",
        payload: event({
          state: "disconnected",
          attempt: 9,
          lastOkTs: 900,
          detail: "runner process exited",
        }),
      },
      snapshot(2_000, 2),
    ]);
    expect(state.link).toBe("unknown");
    expect(state.attempt).toBe(0);
    expect(state.detail).toBeNull();
    // The retired runner's last word must not be re-dated onto the new backend.
    expect(state.lastOkAtMs).toBeNull();
    expect(state.awaitingBackendData).toBe(false);
    expect(isStale(state)).toBe(false);
    expect(connectionSummary(state).headline).toBeNull();

    // Then the embedded event lands, and there is nothing left to wait for.
    const landed = connectionReducer(state, {
      type: "runner-event",
      payload: event({ backendGeneration: 2, state: "embedded" }),
    });
    expect(landed.link).toBe("embedded");
    expect(landed.awaitingBackendData).toBe(false);
    expect(isStale(landed)).toBe(false);
  });

  it("still refuses to let a same-generation snapshot clear a degradation", () => {
    // The guard rail on the test above: the generation is what tells a different
    // backend from the same backend serving its last known state.
    const state = fold([
      { type: "runner-event", payload: event({ backendGeneration: 2 }) },
      snapshot(1_000, 2),
      snapshot(2_000, 2),
    ]);
    expect(state.link).toBe("reconnecting");
    expect(state.attempt).toBe(1);
    expect(isStale(state)).toBe(true);
  });

  it("says nothing about the waiting state once it is over, even mid-degradation", () => {
    // A degraded *current* backend is the frozen-boards case again, and it must
    // get its own sentence back once its data has arrived.
    const state = fold([
      snapshot(1_000),
      { type: "runner-event", payload: event({ backendGeneration: 2 }) },
      snapshot(2_000, 2),
    ]);
    expect(state.awaitingBackendData).toBe(false);
    expect(connectionSummary(state).headline).toBe(
      "Reconnecting to the game runner",
    );
  });
});
