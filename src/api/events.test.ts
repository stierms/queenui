import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { listen } from "@tauri-apps/api/event";
import {
  onCloseRequested,
  onHistoryUpdated,
  onLogsUpdated,
  onRunnerConnection,
  onSnapshot,
  RUNNER_CONNECTION_EVENT,
  subscribe,
} from "./events";
import { emptySnapshot } from "../types";

vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

type Resolver = (cleanup: () => void) => void;

/** A `listen` whose promise is resolved by the test, not by the bridge. */
function deferredListen() {
  const resolvers: Resolver[] = [];
  const handlers: Array<(payload: unknown) => void> = [];
  vi.mocked(listen).mockImplementation(((_event: string, handler: unknown) => {
    handlers.push((payload) =>
      (handler as (message: { payload: unknown }) => void)({ payload }),
    );
    return new Promise<() => void>((resolve) => resolvers.push(resolve));
  }) as typeof listen);
  return { resolvers, handlers };
}

beforeEach(() => {
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
});

afterEach(() => {
  vi.clearAllMocks();
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

describe("event subscriptions", () => {
  it("is a no-op outside the Tauri shell rather than throwing", () => {
    // `listen` dereferences the bridge eagerly, so a browser dev preview would
    // take the page down instead of degrading.
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
    const callback = vi.fn();
    expect(() => subscribe("queenui://snapshot", callback)()).not.toThrow();
    expect(vi.mocked(listen)).not.toHaveBeenCalled();
  });

  it("releases a listener that resolves after the subscription was cancelled", async () => {
    // The StrictMode double-mount case: without this the freshly registered
    // Tauri listener is orphaned for the lifetime of the app.
    const { resolvers } = deferredListen();
    const cleanup = vi.fn();
    const unsubscribe = subscribe("queenui://snapshot", vi.fn());

    unsubscribe();
    resolvers[0](cleanup);
    await Promise.resolve();

    expect(cleanup).toHaveBeenCalledTimes(1);
  });

  it("never invokes the callback after cancellation", async () => {
    const { resolvers, handlers } = deferredListen();
    const callback = vi.fn();
    const unsubscribe = subscribe<number>(
      "queenui://close-requested",
      callback,
    );
    resolvers[0](() => {});
    await Promise.resolve();

    handlers[0](7);
    expect(callback).toHaveBeenCalledWith(7);

    unsubscribe();
    handlers[0](9);
    expect(callback).toHaveBeenCalledTimes(1);
  });

  it("releases the listener once the subscription resolves first", async () => {
    const { resolvers } = deferredListen();
    const cleanup = vi.fn();
    const unsubscribe = subscribe("queenui://snapshot", vi.fn());
    resolvers[0](cleanup);
    await Promise.resolve();

    expect(cleanup).not.toHaveBeenCalled();
    unsubscribe();
    expect(cleanup).toHaveBeenCalledTimes(1);
    // Idempotent: a second unsubscribe must not double-release.
    unsubscribe();
    expect(cleanup).toHaveBeenCalledTimes(1);
  });

  it("subscribes each named wrapper to its own event", () => {
    vi.mocked(listen).mockImplementation((() =>
      Promise.resolve(() => undefined)) as typeof listen);
    onSnapshot(vi.fn());
    onCloseRequested(vi.fn());
    onRunnerConnection(vi.fn());

    const events = vi.mocked(listen).mock.calls.map((call) => call[0]);
    expect(events).toEqual([
      "queenui://snapshot",
      "queenui://close-requested",
      RUNNER_CONNECTION_EVENT,
    ]);
  });

  it("unwraps the snapshot envelope and hands over the generation that stamped it", async () => {
    /*
     * Core events are envelopes as of wave 3 (`BackendSnapshotEvent`). Reading
     * the envelope as the snapshot would put an object with no `engines`,
     * `accounts` or `games` into every surface — and, worse, would discard the
     * one field that says which backend produced it.
     */
    const { resolvers, handlers } = deferredListen();
    const callback = vi.fn();
    onSnapshot(callback);
    resolvers[0](() => {});
    await Promise.resolve();

    handlers[0]({ backendGeneration: 7, payload: emptySnapshot });

    expect(callback).toHaveBeenCalledExactlyOnceWith(emptySnapshot, 7);
  });

  it("reports the close-requested count out of its envelope", async () => {
    /*
     * The payload is `{ reportedCount }` — pinned on the Rust side, where
     * `CloseRequestedPayload` serializes to exactly that — and it used to be
     * read as a bare number. The object was then compared with `> 0`, which is
     * always false, so the count could never raise the guard on its own: the one
     * job the payload has is to warn before the snapshot catches up.
     */
    const { resolvers, handlers } = deferredListen();
    const callback = vi.fn();
    onCloseRequested(callback);
    resolvers[0](() => {});
    await Promise.resolve();

    handlers[0]({ reportedCount: 2 });

    expect(callback).toHaveBeenCalledExactlyOnceWith(2);
  });

  it("treats the notification envelopes as the bare 'read it again' they are", async () => {
    // Both carry a generation, and neither carries data. The re-read they
    // trigger goes to whichever backend is live, so there is nothing here that
    // could present a retired backend's state.
    for (const subscribeTo of [onLogsUpdated, onHistoryUpdated]) {
      const { resolvers, handlers } = deferredListen();
      const callback = vi.fn();
      subscribeTo(callback);
      resolvers[0](() => {});
      await Promise.resolve();

      handlers[0]({ backendGeneration: 3, payload: null });

      expect(callback).toHaveBeenCalledExactlyOnceWith();
      vi.clearAllMocks();
    }
  });

  it("listens for the namespaced runner-connection event", () => {
    /*
     * The connection event used to be the one unprefixed event name in the app.
     * The literal is pinned here because the backend emits it; a silent
     * divergence would not fail any other test, it would just stop the
     * staleness model from ever hearing that the link died.
     */
    expect(RUNNER_CONNECTION_EVENT).toBe("queenui://runner-connection");
    expect(RUNNER_CONNECTION_EVENT.startsWith("queenui://")).toBe(true);
  });
});
