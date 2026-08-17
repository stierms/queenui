import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen, type EventCallback } from "@tauri-apps/api/event";
import App from "./App";
import {
  CLOSE_REQUESTED_EVENT,
  RUNNER_CONNECTION_EVENT,
  SNAPSHOT_EVENT,
} from "./api/events";
import type { AppSnapshot, LiveGame, RunnerConnectionEvent } from "./types";

/*
 * The three failure modes the operator-safety round exists to close:
 *   1. a stale close-guard flag raising an "abandon your games" dialog nobody
 *      asked for, over a live board (MUST-1);
 *   2. an unreachable backend rendering the first-run onboarding screen
 *      (MUST-2);
 *   3. a dead runner rendering as a live game with ticking clocks (MUST-3).
 */

const baseSnapshot: AppSnapshot = {
  engines: [
    {
      id: "engine-1",
      name: "Queen",
      path: "C:\\queen.exe",
      author: null,
      optionCount: 12,
      options: [],
      openingBook: null,
    },
  ],
  accounts: [
    {
      id: "queenbot",
      username: "QueenBot",
      engineId: "engine-1",
      rating: 2400,
      enabled: true,
    },
  ],
  runtimes: [{ accountId: "queenbot", status: "playing", error: null }],
  games: [],
  campaigns: [],
  campaignRuntimes: [],
};

function liveGame(overrides: Partial<LiveGame> = {}): LiveGame {
  return {
    id: "live-1",
    accountId: "queenbot",
    botUsername: "QueenBot",
    opponent: "OpponentBot",
    color: "white",
    initialFen: "startpos",
    moves: "e2e4 e7e5",
    status: "started",
    whiteTime: 60_000,
    blackTime: 60_000,
    whiteIncrement: 1_000,
    blackIncrement: 1_000,
    clockUpdatedAt: Date.now(),
    botRating: null,
    opponentRating: null,
    result: null,
    engineLine: null,
    error: null,
    engineThinking: true,
    engineInfo: {
      depth: 12,
      scoreCp: 20,
      principalVariation: ["g1f3", "b8c6"],
      raw: "info depth 12",
    },
    ...overrides,
  };
}

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn() }));

/**
 * Captures every registered Tauri listener by event name, so a test can push a
 * snapshot, a close request or a runner-connection transition at will. Mirrors
 * the real bridge closely enough that `api/events`' race-safe unsubscribe path
 * still runs — including the wire envelopes, which is the whole point of
 * pushing raw payloads through `listen` rather than calling the hooks directly.
 */
function captureListeners() {
  const handlers = new Map<string, EventCallback<unknown>>();
  vi.mocked(listen).mockImplementation(((event: string, handler: unknown) => {
    handlers.set(event, handler as EventCallback<unknown>);
    return Promise.resolve(() => handlers.delete(event));
  }) as typeof listen);
  return {
    emit(event: string, payload: unknown) {
      act(() => {
        handlers.get(event)?.({ event, id: 1, payload });
      });
    },
    /**
     * A snapshot in its envelope, stamped with the backend that produced it.
     * Generation 1 is the backend the initial fetch loaded from, so a test that
     * is not about switching runners keeps talking to the same one.
     */
    emitSnapshot(snapshot: AppSnapshot, backendGeneration = 1) {
      this.emit(SNAPSHOT_EVENT, { backendGeneration, payload: snapshot });
    },
    /** The blocked close, in the `{ reportedCount }` shape the Rust emits. */
    emitCloseRequested(reportedCount: number) {
      this.emit(CLOSE_REQUESTED_EVENT, { reportedCount });
    },
    /** A link state, defaulted to generation 1 like the snapshots above. */
    emitConnection(
      event: Partial<RunnerConnectionEvent> & {
        state: RunnerConnectionEvent["state"];
      },
    ) {
      this.emit(RUNNER_CONNECTION_EVENT, {
        backendGeneration: 1,
        attempt: 0,
        lastOkTs: null,
        detail: null,
        ...event,
      });
    },
    has: (event: string) => handlers.has(event),
  };
}

/**
 * A `get_snapshot` answer, in the stamped envelope the command returns.
 *
 * The command used to answer with a bare `AppSnapshot`, attributed to whichever
 * backend was live when the response landed. It now carries the generation of the
 * backend that produced it — the same envelope the snapshot event uses — so a
 * scripted IPC has to script the envelope. `1` is the generation the first
 * backend is published with, which is what every test here means.
 */
function stampedResolve(payload: AppSnapshot, backendGeneration = 1) {
  return Promise.resolve({ backendGeneration, payload });
}

function backend(snapshot: AppSnapshot | (() => AppSnapshot)) {
  vi.mocked(invoke).mockImplementation((command: string) => {
    if (command === "get_snapshot") {
      return stampedResolve(
        typeof snapshot === "function" ? snapshot() : snapshot,
      );
    }
    if (command === "get_runner_settings") {
      return Promise.resolve({
        mode: "embedded",
        url: null,
        paired: false,
        activeMode: "embedded",
        source: "saved",
        restartRequired: false,
        allowInsecureRemoteHttp: false,
      });
    }
    return Promise.resolve(undefined);
  });
}

beforeEach(() => {
  // `api/events` only subscribes inside the Tauri shell.
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
});

afterEach(() => {
  cleanup();
  localStorage.clear();
  vi.clearAllMocks();
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

describe("close guard lifecycle", () => {
  it("raises the guard when the backend blocks a close during a game", async () => {
    const events = captureListeners();
    backend({ ...baseSnapshot, games: [liveGame()] });
    render(<App />);
    await screen.findByLabelText("Current chess position");

    events.emitCloseRequested(1);

    expect(
      await screen.findByText("A game is still being played"),
    ).toBeInTheDocument();
  });

  it("retires the guard when the last game ends, instead of leaving the flag set", async () => {
    // The bug: the flag cleared only on "Keep playing", so a game that ended
    // while the operator hesitated left it true forever — and the next game to
    // start raised an "abandon your games" dialog on its own.
    const events = captureListeners();
    backend({ ...baseSnapshot, games: [liveGame()] });
    render(<App />);
    await screen.findByLabelText("Current chess position");

    events.emitCloseRequested(1);
    expect(
      await screen.findByText("A game is still being played"),
    ).toBeInTheDocument();

    // The game finishes while the dialog is open.
    events.emitSnapshot({
      ...baseSnapshot,
      games: [liveGame({ status: "resign", result: "1-0" })],
    });
    await waitFor(() =>
      expect(
        screen.queryByText("A game is still being played"),
      ).not.toBeInTheDocument(),
    );

    // A brand-new game must not resurrect it.
    events.emitSnapshot({
      ...baseSnapshot,
      games: [liveGame({ id: "live-2", opponent: "NextBot" })],
    });
    await screen.findByLabelText("Current chess position");
    expect(
      screen.queryByText("A game is still being played"),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByText(/games are still being played/),
    ).not.toBeInTheDocument();
  });

  it("raises the guard from the event payload when the snapshot is still behind", async () => {
    // The payload is documented as the mitigation for a lagging snapshot; it
    // used to be discarded.
    const events = captureListeners();
    backend(baseSnapshot);
    render(<App />);
    await screen.findByRole("heading", { name: "Bot fleet" });

    events.emitCloseRequested(2);

    expect(
      await screen.findByText("2 games are still being played"),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/details have not arrived yet/),
    ).toBeInTheDocument();
  });

  it("keeps playing without closing when the operator declines", async () => {
    const user = userEvent.setup();
    const events = captureListeners();
    backend({ ...baseSnapshot, games: [liveGame()] });
    render(<App />);
    await screen.findByLabelText("Current chess position");

    events.emitCloseRequested(1);
    await user.click(
      await screen.findByRole("button", { name: "Keep playing" }),
    );

    expect(
      screen.queryByText("A game is still being played"),
    ).not.toBeInTheDocument();
    expect(vi.mocked(invoke)).not.toHaveBeenCalledWith(
      "confirm_close",
      expect.anything(),
    );
  });
});

describe("backend unavailable", () => {
  it("says the backend is unreachable instead of offering first-run setup", async () => {
    vi.mocked(listen).mockImplementation((() =>
      Promise.resolve(() => undefined)) as typeof listen);
    vi.mocked(invoke).mockRejectedValue(new Error("ipc bridge is gone"));
    render(<App />);

    expect(
      await screen.findByRole("heading", {
        name: "QueenUI can't reach its backend",
      }),
    ).toBeInTheDocument();
    // Named twice on purpose: once in the shell-wide banner, once in the page
    // body that replaced the onboarding flow.
    expect(screen.getAllByText("ipc bridge is gone")).toHaveLength(2);
    expect(
      screen.queryByRole("heading", { name: "Put your engine in the chair." }),
    ).not.toBeInTheDocument();
  });

  it("retries the fetch on demand and shows the recovered fleet", async () => {
    const user = userEvent.setup();
    vi.mocked(listen).mockImplementation((() =>
      Promise.resolve(() => undefined)) as typeof listen);
    let healthy = false;
    vi.mocked(invoke).mockImplementation((command: string) => {
      if (command === "get_snapshot") {
        return healthy
          ? stampedResolve(baseSnapshot)
          : Promise.reject(new Error("still down"));
      }
      return Promise.resolve(undefined);
    });
    render(<App />);

    await screen.findByRole("heading", {
      name: "QueenUI can't reach its backend",
    });
    healthy = true;
    await user.click(screen.getAllByRole("button", { name: /Try again/ })[0]);

    expect(
      await screen.findByRole("heading", { name: "Bot fleet" }),
    ).toBeInTheDocument();
  });
});

describe("interrupted runner switch", () => {
  it("carries the backend's recovery instruction into the action failure whole", async () => {
    /*
     * A switch that was abandoned mid-flight leaves the backend slot holding no
     * runner, so the *next* thing the operator touches is what tells them —
     * every command fails with this one sentence, which names the remedy
     * (Settings ▸ Save runner). The notice may add the intent it belongs to,
     * because "the runner switch was interrupted" does not say which of several
     * concurrent actions just died; it may not replace or contradict the
     * remedy, which is the only route back.
     */
    const user = userEvent.setup();
    const interrupted =
      "The runner switch was interrupted; save runner settings again to recover the backend";
    vi.mocked(listen).mockImplementation((() =>
      Promise.resolve(() => undefined)) as typeof listen);
    vi.mocked(invoke).mockImplementation((command: string) => {
      if (command === "get_snapshot") return stampedResolve(baseSnapshot);
      if (command === "stop_bot") return Promise.reject(interrupted);
      return Promise.resolve(undefined);
    });
    render(<App />);

    await user.click(await screen.findByRole("button", { name: "Stop" }));

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(interrupted);
    expect(alert).toHaveTextContent("Could not stop QueenBot");
    // Neither the outcome the operator asked for nor a remedy of the app's own.
    expect(screen.queryByText("QueenBot stopped")).toBeNull();
    expect(alert).not.toHaveTextContent(/restart/i);
  });
});

describe("runner connection honesty", () => {
  const degraded: RunnerConnectionEvent = {
    backendGeneration: 1,
    state: "reconnecting",
    attempt: 3,
    lastOkTs: 1_700_000_000_000,
    detail: "ssh tunnel closed",
  };

  it("subscribes to the runner-connection event", async () => {
    const events = captureListeners();
    backend(baseSnapshot);
    render(<App />);
    await screen.findByRole("heading", { name: "Bot fleet" });
    expect(events.has(RUNNER_CONNECTION_EVENT)).toBe(true);
  });

  it("stops calling a frozen game live and freezes its clocks", async () => {
    const events = captureListeners();
    const startedAt = Date.now() - 30_000;
    backend({
      ...baseSnapshot,
      games: [liveGame({ whiteTime: 45_000, clockUpdatedAt: startedAt })],
    });
    render(<App />);
    await screen.findByLabelText("Current chess position");

    // Live: the panel asserts liveness and the arrow shows the engine's pick.
    expect(screen.getByText("Live on Lichess")).toBeInTheDocument();
    expect(
      screen.getByLabelText("Engine prefers g1 to f3"),
    ).toBeInTheDocument();

    events.emit(RUNNER_CONNECTION_EVENT, degraded);

    expect(
      await screen.findByText(/Reconnecting to the game runner \(attempt 3\)/),
    ).toBeInTheDocument();
    expect(screen.getByText("ssh tunnel closed")).toBeInTheDocument();
    expect(screen.queryByText("Live on Lichess")).not.toBeInTheDocument();
    expect(
      screen.getByText("Not live · waiting for the runner"),
    ).toBeInTheDocument();
    // The best-move arrow and the thinking pulse both stop asserting "now".
    expect(
      screen.queryByLabelText("Engine prefers g1 to f3"),
    ).not.toBeInTheDocument();
    expect(screen.queryByText("Thinking")).not.toBeInTheDocument();
    expect(screen.getByText("Last seen")).toBeInTheDocument();
    // The board itself is marked, so no surface renders it as current.
    expect(
      screen.getByLabelText(
        "Chess position, frozen at the last update from the runner",
      ),
    ).toBeInTheDocument();
    // The clock shows the last server value, not an extrapolation.
    const clock = document.querySelector("time.clock-frozen");
    expect(clock).not.toBeNull();
    expect(clock).toHaveTextContent("00:45");
  });

  it("clears the disconnected banner when the switch to this computer lands", async () => {
    /*
     * The remote-to-embedded live switch is a control-plane switch: the runner
     * this app was reconnecting to is simply not its backend any more. The
     * event says so, and until it was consumed the shell kept a banner about a
     * link that no longer existed — over a local engine, with nothing on screen
     * able to clear it short of a restart.
     *
     * The switch is now two events in a fixed order, and this test used to pin
     * the wrong one. The embedded backend's *snapshot* comes first, stamped with
     * the new generation — "truthful same-generation data lands before the
     * connection state can clear frontend staleness", in
     * `set_runner_settings_inner` — and the connection event follows. Emitting
     * the connection event alone was a switch that delivered no data, so what
     * the cleared banner uncovered was the dead runner's own boards, presented
     * as this computer's. The games below are the embedded backend's, which is
     * the only reason anything here may be called live.
     */
    const events = captureListeners();
    backend({ ...baseSnapshot, games: [liveGame()] });
    render(<App />);
    await screen.findByLabelText("Current chess position");

    events.emitConnection({
      state: "disconnected",
      attempt: 9,
      lastOkTs: 1_700_000_000_000,
      detail: "runner process exited",
    });
    expect(
      await screen.findByText("Disconnected from the game runner"),
    ).toBeInTheDocument();
    await screen.findByText("Not live · waiting for the runner");

    events.emitSnapshot(
      { ...baseSnapshot, games: [liveGame({ opponent: "LocalBot" })] },
      2,
    );
    events.emitConnection({ backendGeneration: 2, state: "embedded" });

    await waitFor(() =>
      expect(
        screen.queryByText("Disconnected from the game runner"),
      ).not.toBeInTheDocument(),
    );
    expect(screen.queryByText("runner process exited")).toBeNull();
    // The remote runner's last word must not be re-dated onto a local engine.
    expect(screen.queryByText(/^Last update/)).toBeNull();
    // And no surface may still call the games frozen.
    expect(await screen.findByText("Live on Lichess")).toBeInTheDocument();
    expect(
      screen.queryByLabelText(
        "Chess position, frozen at the last update from the runner",
      ),
    ).toBeNull();
    // What is live is the new backend's game, not the old runner's.
    expect(screen.getByText("LocalBot")).toBeInTheDocument();
  });

  it("does not present the previous runner's fleet as the runner it switched to", async () => {
    /*
     * The wrong-backend-snapshot bug, end to end: remote A is showing a fleet,
     * the operator switches to remote B, and B cannot be reached. B publishes a
     * new backend generation and reports its link; it sends no snapshot, because
     * it has nothing to send. Everything on screen is A's — A's accounts, A's
     * engines, A's live boards — and the shell used to keep rendering them under
     * B's banner, as though B were the runner playing them.
     */
    const events = captureListeners();
    backend({ ...baseSnapshot, games: [liveGame()] });
    render(<App />);
    await screen.findByLabelText("Current chess position");
    events.emitConnection({ state: "connected" });
    // Named on every surface at once: the fleet row and the live board.
    expect(screen.getAllByText("QueenBot").length).toBeGreaterThan(0);

    events.emitConnection({
      backendGeneration: 2,
      state: "disconnected",
      attempt: 1,
      detail: "connection refused",
    });

    // The screen says what it is doing, and shows nothing it cannot attribute.
    expect(
      await screen.findByRole("heading", {
        name: "Waiting for the game runner",
      }),
    ).toBeInTheDocument();
    expect(screen.getAllByText("connection refused").length).toBeGreaterThan(0);
    expect(screen.queryAllByText("QueenBot")).toHaveLength(0);
    expect(screen.queryByLabelText("Current chess position")).toBeNull();
    /*
     * And no age on an empty screen: `lastSnapshotAtMs` still timestamps the
     * snapshot the *previous* backend sent, which is no longer displayed
     * anywhere, so dating this screen from it would be the retired runner's last
     * word wearing a different field's name.
     */
    expect(screen.queryByText(/^Last update/)).toBeNull();
    expect(
      screen.queryByLabelText(
        "Chess position, frozen at the last update from the runner",
      ),
    ).toBeNull();
    // Nor the other way this emptiness used to be misread (MUST-2's cousin).
    expect(
      screen.queryByRole("heading", { name: "Put your engine in the chair." }),
    ).toBeNull();
    expect(
      screen.queryByRole("heading", {
        name: "QueenUI can't reach its backend",
      }),
    ).toBeNull();

    // B's first snapshot is what makes a fleet showable again — and it is B's.
    events.emitSnapshot(
      {
        ...baseSnapshot,
        accounts: [
          { ...baseSnapshot.accounts[0], id: "otherbot", username: "OtherBot" },
        ],
        runtimes: [{ accountId: "otherbot", status: "playing", error: null }],
      },
      2,
    );

    expect((await screen.findAllByText("OtherBot")).length).toBeGreaterThan(0);
    expect(screen.queryAllByText("QueenBot")).toHaveLength(0);
    expect(
      screen.queryByRole("heading", { name: "Waiting for the game runner" }),
    ).toBeNull();
  });

  it("ignores the retired runner's last word after the switch", async () => {
    /*
     * Cancellation and delivery race on the Rust side, so the backend this app
     * has left can still get one more event through. Neither its data nor its
     * link state may touch the screen: a "disconnected" from the runner that is
     * no longer this app's backend would freeze boards that are not frozen, and
     * its snapshot would repopulate the fleet that was just dropped.
     */
    const events = captureListeners();
    backend({ ...baseSnapshot, games: [liveGame()] });
    render(<App />);
    await screen.findByLabelText("Current chess position");
    events.emitConnection({ state: "connected" });

    events.emitSnapshot(
      { ...baseSnapshot, games: [liveGame({ opponent: "LocalBot" })] },
      2,
    );
    events.emitConnection({ backendGeneration: 2, state: "embedded" });
    await screen.findByText("LocalBot");

    events.emitConnection({
      backendGeneration: 1,
      state: "disconnected",
      attempt: 4,
      detail: "runner process exited",
    });
    events.emitSnapshot(
      { ...baseSnapshot, games: [liveGame({ opponent: "GhostBot" })] },
      1,
    );

    expect(screen.queryByText("Disconnected from the game runner")).toBeNull();
    expect(screen.queryByText("runner process exited")).toBeNull();
    expect(screen.queryByText("GhostBot")).toBeNull();
    expect(screen.getByText("LocalBot")).toBeInTheDocument();
    expect(screen.getByText("Live on Lichess")).toBeInTheDocument();
  });

  it("clears the frozen state when the runner comes back", async () => {
    const events = captureListeners();
    backend({ ...baseSnapshot, games: [liveGame()] });
    render(<App />);
    await screen.findByLabelText("Current chess position");

    events.emit(RUNNER_CONNECTION_EVENT, degraded);
    await screen.findByText("Not live · waiting for the runner");
    events.emitConnection({ state: "connected", lastOkTs: Date.now() });

    expect(await screen.findByText("Live on Lichess")).toBeInTheDocument();
    expect(
      screen.queryByText(/Reconnecting to the game runner/),
    ).not.toBeInTheDocument();
  });
});
