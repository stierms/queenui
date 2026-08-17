import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { getSnapshot } from "../api/commands";
import { onRunnerConnection, onSnapshot } from "../api/events";
import { useSnapshot } from "./useSnapshot";
import {
  emptySnapshot,
  type AppSnapshot,
  type RunnerConnectionEvent,
} from "../types";

vi.mock("../api/commands", () => ({ getSnapshot: vi.fn() }));
vi.mock("../api/events", () => ({
  onSnapshot: vi.fn(() => () => {}),
  onRunnerConnection: vi.fn(() => () => {}),
}));

const fromFetch: AppSnapshot = {
  ...emptySnapshot,
  engines: [
    {
      id: "slow",
      name: "Slow fetch",
      path: "p",
      author: null,
      optionCount: 0,
      options: [],
      openingBook: null,
    },
  ],
};
const fromEvent: AppSnapshot = {
  ...emptySnapshot,
  engines: [
    {
      id: "fresh",
      name: "Fresh event",
      path: "p",
      author: null,
      optionCount: 0,
      options: [],
      openingBook: null,
    },
  ],
};

/**
 * The stamped envelope `get_snapshot` answers with, as of wave 4.
 *
 * The fetch used to answer with a bare snapshot and be attributed to whichever
 * backend was live when it landed. It now carries the generation of the backend
 * that produced it, exactly as the event does, so both go through one rule — and
 * the default here is `1` for the same reason the event emitter's is: a test that
 * is not about generations reads as one backend talking.
 */
function fetched(payload: AppSnapshot, backendGeneration = 1) {
  return { backendGeneration, payload };
}

/** A `get_snapshot` call the test settles by hand, to hold it in flight. */
function deferredFetch() {
  let resolveFetch: (value: ReturnType<typeof fetched>) => void = () => {};
  const promise = new Promise<ReturnType<typeof fetched>>((resolve) => {
    resolveFetch = resolve;
  });
  return {
    mockFetch: () => vi.mocked(getSnapshot).mockReturnValue(promise),
    resolveFetch: (value: ReturnType<typeof fetched>) => resolveFetch(value),
  };
}

/**
 * Hands the test the callback the hook registered for each subscription.
 *
 * Both emitters carry a backend generation, because both events do. `1` is the
 * default so that a test which is not about generations reads as one backend
 * talking, which is what every one of them meant before generations existed.
 */
function captureSubscriptions() {
  let snapshotCallback:
    ((value: AppSnapshot, generation: number) => void) | undefined;
  let connectionCallback: ((value: unknown) => void) | undefined;
  vi.mocked(onSnapshot).mockImplementation((callback) => {
    snapshotCallback = callback;
    return () => {};
  });
  vi.mocked(onRunnerConnection).mockImplementation((callback) => {
    connectionCallback = callback as (value: unknown) => void;
    return () => {};
  });
  return {
    emitSnapshot: (value: AppSnapshot, generation = 1) =>
      act(() => {
        snapshotCallback?.(value, generation);
      }),
    emitConnection: (
      value: Partial<RunnerConnectionEvent> & {
        state: RunnerConnectionEvent["state"];
      },
    ) =>
      act(() => {
        connectionCallback?.({
          backendGeneration: 1,
          attempt: 0,
          lastOkTs: null,
          detail: null,
          ...value,
        });
      }),
  };
}

beforeEach(() => {
  vi.mocked(onSnapshot).mockImplementation(() => () => {});
  vi.mocked(onRunnerConnection).mockImplementation(() => () => {});
});

afterEach(() => vi.clearAllMocks());

describe("useSnapshot", () => {
  it("does not subscribe or fetch when disabled", () => {
    renderHook(() => useSnapshot(false, fromFetch, vi.fn()));
    expect(vi.mocked(getSnapshot)).not.toHaveBeenCalled();
    expect(vi.mocked(onSnapshot)).not.toHaveBeenCalled();
  });

  it("seeds from the initial fetch", async () => {
    vi.mocked(getSnapshot).mockResolvedValue(fetched(fromFetch));
    const { result } = renderHook(() =>
      useSnapshot(true, emptySnapshot, vi.fn()),
    );
    expect(result.current.loading).toBe(true);
    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(result.current.snapshot).toEqual(fromFetch);
  });

  it("never lets a slow fetch clobber fresher event data", async () => {
    // The hook's docstring claimed this; nothing verified it. Both stamped with
    // the same generation, which is what makes this a freshness question rather
    // than an attribution one — see the two tests that split them below.
    const { mockFetch, resolveFetch } = deferredFetch();
    mockFetch();
    const events = captureSubscriptions();
    const { result } = renderHook(() =>
      useSnapshot(true, emptySnapshot, vi.fn()),
    );

    events.emitSnapshot(fromEvent);
    expect(result.current.snapshot).toEqual(fromEvent);

    await act(async () => {
      resolveFetch(fetched(fromFetch));
      await Promise.resolve();
    });
    expect(result.current.snapshot).toEqual(fromEvent);
  });

  it("reports an unreachable backend and keeps the last good snapshot", async () => {
    const onLoadError = vi.fn();
    vi.mocked(getSnapshot).mockRejectedValue(new Error("no bridge"));
    const { result } = renderHook(() =>
      useSnapshot(true, emptySnapshot, onLoadError),
    );

    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(onLoadError).toHaveBeenCalledTimes(1);
    expect(result.current.unavailable).toBe(true);
    expect(result.current.stale).toBe(true);
    expect(result.current.connection.backendDetail).toBe("no bridge");
    // Emptiness must never be presented as a fresh install.
    expect(result.current.snapshot).toEqual(emptySnapshot);
  });

  it("stops reporting unavailable once a later event arrives", async () => {
    const events = captureSubscriptions();
    vi.mocked(getSnapshot).mockRejectedValue(new Error("no bridge"));
    const { result } = renderHook(() =>
      useSnapshot(true, emptySnapshot, vi.fn()),
    );
    await waitFor(() => expect(result.current.unavailable).toBe(true));

    events.emitSnapshot(fromEvent);

    expect(result.current.unavailable).toBe(false);
    expect(result.current.stale).toBe(false);
    expect(result.current.snapshot).toEqual(fromEvent);
  });

  it("re-fetches on retry", async () => {
    vi.mocked(getSnapshot)
      .mockRejectedValueOnce(new Error("down"))
      .mockResolvedValueOnce(fetched(fromFetch));
    const { result } = renderHook(() =>
      useSnapshot(true, emptySnapshot, vi.fn()),
    );
    await waitFor(() => expect(result.current.unavailable).toBe(true));

    act(() => result.current.retry());

    await waitFor(() => expect(result.current.snapshot).toEqual(fromFetch));
    expect(result.current.unavailable).toBe(false);
    expect(vi.mocked(getSnapshot)).toHaveBeenCalledTimes(2);
  });

  it("goes stale on a degraded runner connection without dropping data", async () => {
    const events = captureSubscriptions();
    vi.mocked(getSnapshot).mockResolvedValue(fetched(fromFetch));
    const { result } = renderHook(() =>
      useSnapshot(true, emptySnapshot, vi.fn()),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    events.emitConnection({
      state: "reconnecting",
      attempt: 2,
      lastOkTs: 1_000,
      detail: "tunnel closed",
    });

    expect(result.current.stale).toBe(true);
    expect(result.current.unavailable).toBe(false);
    expect(result.current.snapshot).toEqual(fromFetch);
    expect(result.current.connection.attempt).toBe(2);
  });

  it("drops a snapshot stamped with a backend it has already left", async () => {
    /*
     * The verified bug this exists for: the retired runner's last snapshot
     * arriving after the switch, and being applied as the new backend's state.
     * `fromEvent` here is generation 2's fleet; `fromFetch` is the one before
     * it, and it must not come back.
     */
    const events = captureSubscriptions();
    vi.mocked(getSnapshot).mockResolvedValue(fetched(emptySnapshot));
    const { result } = renderHook(() =>
      useSnapshot(true, emptySnapshot, vi.fn()),
    );
    await waitFor(() => expect(result.current.loading).toBe(false));

    events.emitSnapshot(fromEvent, 2);
    expect(result.current.snapshot).toEqual(fromEvent);

    events.emitSnapshot(fromFetch, 1);

    expect(result.current.snapshot).toEqual(fromEvent);
    expect(result.current.connection.backendGeneration).toBe(2);
  });

  it("empties the screen when a different backend is published, and says why", async () => {
    /*
     * Switching to a remote runner that cannot be reached. The new backend
     * reports its link and nothing else, so there is no snapshot of its own —
     * and the fleet on screen is the *previous* runner's, which is precisely
     * what used to be presented as the new one's.
     */
    const events = captureSubscriptions();
    vi.mocked(getSnapshot).mockResolvedValue(fetched(fromFetch));
    const { result } = renderHook(() =>
      useSnapshot(true, emptySnapshot, vi.fn()),
    );
    await waitFor(() => expect(result.current.snapshot).toEqual(fromFetch));

    events.emitConnection({ state: "connected" });
    events.emitConnection({
      backendGeneration: 2,
      state: "disconnected",
      detail: "connection refused",
    });

    expect(result.current.snapshot).toEqual(emptySnapshot);
    expect(result.current.awaitingBackend).toBe(true);
    expect(result.current.stale).toBe(true);
    // Not the never-loaded state: the service is answering, and this is not the
    // screen that says it cannot be reached.
    expect(result.current.unavailable).toBe(false);
  });

  it("shows the new backend's fleet as soon as its first snapshot arrives", async () => {
    const events = captureSubscriptions();
    vi.mocked(getSnapshot).mockResolvedValue(fetched(fromFetch));
    const { result } = renderHook(() =>
      useSnapshot(true, emptySnapshot, vi.fn()),
    );
    await waitFor(() => expect(result.current.snapshot).toEqual(fromFetch));

    events.emitConnection({ state: "connected" });
    events.emitConnection({ backendGeneration: 2, state: "connected" });
    expect(result.current.awaitingBackend).toBe(true);

    events.emitSnapshot(fromEvent, 2);

    expect(result.current.snapshot).toEqual(fromEvent);
    expect(result.current.awaitingBackend).toBe(false);
    expect(result.current.stale).toBe(false);
  });

  it("recovers from the waiting state through the retry fetch", async () => {
    /*
     * An embedded backend emits snapshots only when something changes, so a
     * generation announced by a connection event alone can have nothing more to
     * say. The retry is a command, dispatched to whichever backend is live, so
     * it is a real answer from the new one rather than a re-run of a
     * subscription that is idle by design.
     */
    const events = captureSubscriptions();
    vi.mocked(getSnapshot).mockResolvedValue(fetched(fromFetch));
    const { result } = renderHook(() =>
      useSnapshot(true, emptySnapshot, vi.fn()),
    );
    await waitFor(() => expect(result.current.snapshot).toEqual(fromFetch));

    events.emitConnection({ state: "connected" });
    events.emitConnection({ backendGeneration: 2, state: "embedded" });
    expect(result.current.awaitingBackend).toBe(true);
    expect(result.current.snapshot).toEqual(emptySnapshot);

    vi.mocked(getSnapshot).mockResolvedValue(fetched(fromEvent, 2));
    act(() => result.current.retry());

    await waitFor(() => expect(result.current.snapshot).toEqual(fromEvent));
    expect(result.current.awaitingBackend).toBe(false);
    expect(result.current.stale).toBe(false);
  });

  it("adopts the backend the very first fetch names, so the next switch is one", async () => {
    /*
     * The bootstrap the un-stamped fetch could not do.
     *
     * An embedded backend emits snapshots only when something changes and no
     * connection events at all, so `get_snapshot` is routinely the *only* thing
     * this app has heard — and with the response unstamped that left the app at
     * generation 0, its "nobody has spoken yet" sentinel. The switch away then
     * presented as a first generation rather than a replacement, and a first
     * generation is deliberately not treated as a handover: the previous
     * backend's fleet stayed on screen, attributed to the runner now being
     * dispatched to. That is the wrong-backend-snapshot bug, reached through the
     * one path that had no stamp.
     */
    const events = captureSubscriptions();
    vi.mocked(getSnapshot).mockResolvedValue(fetched(fromFetch, 1));
    const { result } = renderHook(() =>
      useSnapshot(true, emptySnapshot, vi.fn()),
    );
    await waitFor(() => expect(result.current.snapshot).toEqual(fromFetch));
    expect(result.current.connection.backendGeneration).toBe(1);
    // The fetch is not a handover: it is the backend it was loading from.
    expect(result.current.awaitingBackend).toBe(false);

    // No snapshot event has ever arrived; the switch is announced alone.
    events.emitConnection({
      backendGeneration: 2,
      state: "disconnected",
      detail: "connection refused",
    });

    expect(result.current.awaitingBackend).toBe(true);
    expect(result.current.snapshot).toEqual(emptySnapshot);
    expect(result.current.stale).toBe(true);
  });

  it("drops a fetch response that outlived the backend it was dispatched to", async () => {
    /*
     * The response was in flight when the switch happened. It is a *command*
     * result, which is exactly why it used to be trusted as current — but the
     * call is awaited across an IPC hop and a possible remote round trip, and a
     * live switch does not wait for it.
     *
     * No snapshot event is emitted here, on purpose: `apply` (the slow-fetch
     * guard) is therefore true, so the generation rule is the only thing that can
     * reject this payload. And the switch is announced by a connection event
     * alone — the unreachable-runner case — so applying it would put the retired
     * runner's fleet back on screen and stop the app saying it is waiting.
     */
    const { mockFetch, resolveFetch } = deferredFetch();
    mockFetch();
    const events = captureSubscriptions();
    const { result } = renderHook(() =>
      useSnapshot(true, emptySnapshot, vi.fn()),
    );

    events.emitConnection({ state: "connected" });
    events.emitConnection({
      backendGeneration: 2,
      state: "disconnected",
      detail: "connection refused",
    });
    expect(result.current.awaitingBackend).toBe(true);

    await act(async () => {
      resolveFetch(fetched(fromFetch, 1));
      await Promise.resolve();
    });

    expect(result.current.snapshot).toEqual(emptySnapshot);
    expect(result.current.awaitingBackend).toBe(true);
    expect(result.current.connection.backendGeneration).toBe(2);
    // Not proof the backend answered, either: the backend that answered is gone.
    expect(result.current.connection.lastSnapshotAtMs).toBeNull();
  });

  it("applies a fetch response an event overtook when it is the newer backend's", async () => {
    /*
     * The guard rail on the slow-fetch rule. "An event already landed" settles
     * which is fresher only while both came from the same backend: a late event
     * from the generation the fetch was dispatched *past* would otherwise win, and
     * skipping the response would leave the connection attached to generation 2
     * while the fleet on screen is generation 1's — the same bug, from the other
     * side.
     */
    const { mockFetch, resolveFetch } = deferredFetch();
    mockFetch();
    const events = captureSubscriptions();
    const { result } = renderHook(() =>
      useSnapshot(true, emptySnapshot, vi.fn()),
    );

    // A retired backend's last snapshot, delivered while the fetch is in flight.
    events.emitSnapshot(fromEvent, 1);
    expect(result.current.snapshot).toEqual(fromEvent);

    await act(async () => {
      resolveFetch(fetched(fromFetch, 2));
      await Promise.resolve();
    });

    expect(result.current.snapshot).toEqual(fromFetch);
    expect(result.current.connection.backendGeneration).toBe(2);
    expect(result.current.awaitingBackend).toBe(false);
  });
});
