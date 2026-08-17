import {
  act,
  cleanup,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen, type EventCallback } from "@tauri-apps/api/event";
import { open, save } from "@tauri-apps/plugin-dialog";
import App from "./App";
import { remainingClock } from "./lib/chess";
import type { AppSnapshot, LiveGame } from "./types";

const snapshot: AppSnapshot = {
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
  runtimes: [{ accountId: "queenbot", status: "online", error: null }],
  games: [],
  campaigns: [],
  campaignRuntimes: [
    {
      accountId: "queenbot",
      status: "stopped",
      activeGames: 0,
      pendingChallenges: 0,
      eligibleBots: 0,
      onlineBotsScanned: 312,
      challengesSent: 0,
      lastOpponent: null,
      activity: "Ready",
      error: null,
      nextScanAt: null,
      events: [
        {
          id: "event-1",
          timestamp: 1_700_000_000_000,
          kind: "idle",
          title: "No eligible opponents this scan",
          detail: "312 online · 0 eligible",
        },
      ],
    },
  ],
};

const engineOptionsSnapshot: AppSnapshot = {
  ...snapshot,
  engines: [
    {
      ...snapshot.engines[0],
      optionCount: 3,
      options: [
        {
          name: "Hash",
          optionType: "spin",
          defaultValue: "16",
          value: "16",
          min: 1,
          max: 4096,
          choices: [],
        },
        {
          name: "Ponder",
          optionType: "check",
          defaultValue: "false",
          value: "false",
          min: null,
          max: null,
          choices: [],
        },
        {
          name: "Style",
          optionType: "combo",
          defaultValue: "Normal",
          value: "Normal",
          min: null,
          max: null,
          choices: ["Normal", "Aggressive"],
        },
      ],
    },
  ],
};

/**
 * A `get_snapshot` answer, in the stamped envelope the command returns.
 *
 * The command used to answer with a bare `AppSnapshot` and the frontend
 * attributed it to whichever backend was live when the response landed. It now
 * carries the generation of the backend that produced it — the same envelope the
 * snapshot event uses — so a scripted IPC has to script the envelope. `1` is the
 * generation the first backend is published with, which is what every test here
 * that is not about switching runners means.
 */
function stampedResolve(payload: AppSnapshot, backendGeneration = 1) {
  return Promise.resolve({ backendGeneration, payload });
}

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn((command: string) =>
    command === "get_snapshot"
      ? stampedResolve(snapshot)
      : Promise.resolve({ id: "challenge-1" }),
  ),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => undefined)),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
  save: vi.fn(),
}));

/**
 * Captures the next snapshot listener registration and returns an emitter
 * that delivers a typed snapshot event to it.
 */
function captureSnapshotListener() {
  let callback: EventCallback<AppSnapshot> | undefined;
  vi.mocked(listen).mockImplementationOnce(((_event, handler) => {
    callback = handler as EventCallback<AppSnapshot>;
    return Promise.resolve(() => undefined);
  }) as typeof listen);
  return (payload: AppSnapshot) => {
    callback?.({ event: "queenui://snapshot", id: 1, payload });
  };
}

afterEach(() => {
  cleanup();
  localStorage.clear();
});

describe("QueenUI challenge workflow", () => {
  it("wires account disconnection through the exact credential IPC boundary", async () => {
    const user = userEvent.setup();
    vi.mocked(invoke).mockImplementation((command: string) => {
      if (command === "get_snapshot") {
        return stampedResolve({
          ...snapshot,
          runtimes: [{ accountId: "queenbot", status: "stopped", error: null }],
        });
      }
      return Promise.resolve(undefined);
    });
    render(<App />);
    await user.click(
      await screen.findByRole("button", { name: "Actions for QueenBot" }),
    );
    await user.click(
      await screen.findByRole("menuitem", { name: /Disconnect account/ }),
    );
    await user.click(
      screen.getByRole("button", { name: "Disconnect and delete token" }),
    );
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("remove_lichess_account", {
        accountId: "queenbot",
      }),
    );
  });

  it("reports a refused account removal instead of claiming the token is gone", async () => {
    /*
     * The account and its Lichess token are deleted by the runner that owns
     * them, which can refuse — a locked credential store, a runner that is not
     * reachable. Announcing the deletion anyway would tell the operator a live
     * token had been revoked when it had not.
     */
    const user = userEvent.setup();
    vi.mocked(invoke).mockImplementation((command: string) => {
      if (command === "get_snapshot") {
        return stampedResolve({
          ...snapshot,
          runtimes: [{ accountId: "queenbot", status: "stopped", error: null }],
        });
      }
      if (command === "remove_lichess_account") {
        return Promise.reject(new Error("credential store is locked"));
      }
      return Promise.resolve(undefined);
    });
    render(<App />);

    await user.click(
      await screen.findByRole("button", { name: "Actions for QueenBot" }),
    );
    await user.click(
      await screen.findByRole("menuitem", { name: /Disconnect account/ }),
    );
    await user.click(
      screen.getByRole("button", { name: "Disconnect and delete token" }),
    );

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Could not disconnect QueenBot");
    expect(alert).toHaveTextContent("credential store is locked");
    expect(
      screen.queryByText(/QueenBot disconnected and its token deleted/),
    ).toBeNull();
    // An error notice has no expiry, so it cannot erase itself unseen.
    expect(
      screen.getByRole("button", { name: "Dismiss error" }),
    ).toBeInTheDocument();
  });

  it("interpolates only the active clock from the last server update", () => {
    expect(remainingClock(180_000, true, 10_000, 11_250)).toBe(178_750);
    expect(remainingClock(180_000, false, 10_000, 11_250)).toBe(180_000);
  });

  it("creates a challenge from the dashboard", async () => {
    const user = userEvent.setup();
    render(<App />);

    await user.click(screen.getByRole("button", { name: "New challenge" }));

    const dialog = screen.getByRole("dialog", { name: "Create a challenge" });
    expect(dialog).toBeInTheDocument();
    expect(
      within(dialog).getByRole("button", { name: /^3\+2\s*Blitz$/ }),
    ).toHaveClass("selected");

    const opponent = screen.getByPlaceholderText("Lichess username");
    await user.clear(opponent);
    await user.type(opponent, "NewOpponentBot");
    await user.click(screen.getByRole("button", { name: "Send challenge" }));

    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(
      screen.getByText("Challenge sent to NewOpponentBot"),
    ).toBeInTheDocument();
  });

  it("starts continuous matchmaking with a rating range and concurrency", async () => {
    const user = userEvent.setup();
    render(<App />);

    await user.click(screen.getByRole("button", { name: "Challenges" }));
    expect(
      await screen.findByRole("heading", { name: "Automatic challenge mode" }),
    ).toBeInTheDocument();
    expect(
      screen.getByText("No eligible opponents this scan"),
    ).toBeInTheDocument();

    const minimum = screen.getByRole("spinbutton", { name: "Minimum rating" });
    const maximum = screen.getByRole("spinbutton", { name: "Maximum rating" });
    await user.clear(minimum);
    await user.type(minimum, "2000");
    await user.clear(maximum);
    await user.type(maximum, "2400");
    await user.click(screen.getByRole("button", { name: "3" }));
    await user.click(screen.getByRole("button", { name: "Start matchmaking" }));

    await waitFor(() =>
      expect(vi.mocked(invoke)).toHaveBeenCalledWith("start_campaign", {
        settings: expect.objectContaining({
          accountId: "queenbot",
          minRating: 2000,
          maxRating: 2400,
          concurrency: 3,
          clockLimit: 180,
          clockIncrement: 2,
          // Untouched by this test, so it is the form's default — which now
          // agrees with the backend's own (`default_campaign_rated`).
          rated: true,
        }),
      }),
    );
  });

  it("opens global settings and persists presentation preferences", async () => {
    const user = userEvent.setup();
    render(<App />);

    await user.click(screen.getByRole("button", { name: "Settings" }));
    expect(
      await screen.findByRole("heading", { name: "Board and pieces" }),
    ).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Walnut" }));
    await user.click(screen.getByRole("button", { name: /Ink/ }));
    await user.click(
      screen.getByRole("switch", { name: "Move and capture sounds" }),
    );

    await waitFor(() => {
      expect(localStorage.getItem("queenui-board-theme")).toBe("walnut");
      expect(localStorage.getItem("queenui-piece-set")).toBe("ink");
      expect(localStorage.getItem("queenui-move-sounds")).toBe("off");
    });
    // The swatch is now `aria-hidden` (an `aria-label` on a bare div is
    // dropped anyway); the selection is asserted on the visible text that
    // states it, which is what an operator actually reads.
    expect(screen.getByText("Walnut · Ink")).toBeInTheDocument();
    expect(
      screen.getByRole("switch", { name: "Move and capture sounds" }),
    ).not.toBeChecked();
  });

  it("edits saved time-control presets used by challenge pickers", async () => {
    const user = userEvent.setup();
    render(<App />);

    await user.click(screen.getByRole("button", { name: "Settings" }));
    const minutes = await screen.findByRole("spinbutton", {
      name: "Preset 1 minutes",
    });
    expect(minutes).toHaveValue(1);
    expect(
      screen.getByRole("spinbutton", { name: "Preset 1 increment" }),
    ).toHaveValue(1);

    await user.clear(minutes);
    await user.type(minutes, "2");
    await user.tab();

    await waitFor(() =>
      expect(
        JSON.parse(localStorage.getItem("queenui-time-controls") ?? "[]"),
      ).toEqual(expect.arrayContaining([{ limitMinutes: 2, increment: 1 }])),
    );

    await user.click(screen.getByRole("button", { name: "New challenge" }));
    const dialog = screen.getByRole("dialog", { name: "Create a challenge" });
    expect(
      within(dialog).getByRole("button", { name: /^2\+1\s*Bullet$/ }),
    ).toBeInTheDocument();
  });

  it("configures an opening book and persistent UCI options", async () => {
    const user = userEvent.setup();
    vi.mocked(invoke).mockImplementation((command: string) =>
      command === "get_snapshot"
        ? stampedResolve(engineOptionsSnapshot)
        : Promise.resolve(undefined),
    );
    vi.mocked(open).mockResolvedValue("C:\\books\\performance.bin");
    render(<App />);

    await user.click(await screen.findByRole("button", { name: "Engines" }));
    await user.click(screen.getByRole("button", { name: "Configure" }));
    const dialog = screen.getByRole("dialog", { name: "Configure Queen" });
    await user.click(within(dialog).getByRole("button", { name: "Import" }));
    await user.clear(
      within(dialog).getByRole("spinbutton", {
        name: "Maximum book plies",
      }),
    );
    await user.type(
      within(dialog).getByRole("spinbutton", {
        name: "Maximum book plies",
      }),
      "24",
    );
    await user.click(within(dialog).getByRole("button", { name: "25%" }));
    await user.click(
      within(dialog).getByRole("button", { name: "Save book policy" }),
    );

    await waitFor(() =>
      expect(vi.mocked(invoke)).toHaveBeenCalledWith("configure_opening_book", {
        request: {
          engineId: "engine-1",
          path: "C:\\books\\performance.bin",
          enabled: true,
          maxPlies: 24,
          topMovePercent: 25,
        },
      }),
    );

    await user.click(within(dialog).getByRole("tab", { name: /UCI options/ }));
    const hash = within(dialog).getByRole("spinbutton", { name: "Hash" });
    await user.clear(hash);
    await user.type(hash, "512");
    await user.click(within(dialog).getByRole("button", { name: "Ponder" }));
    await user.selectOptions(
      within(dialog).getByRole("combobox", { name: "Style" }),
      "Aggressive",
    );
    await user.click(
      within(dialog).getByRole("button", { name: "Save UCI options" }),
    );

    await waitFor(() =>
      expect(vi.mocked(invoke)).toHaveBeenCalledWith("update_engine_options", {
        engineId: "engine-1",
        options: [
          { name: "Hash", value: "512" },
          { name: "Ponder", value: "true" },
          { name: "Style", value: "Aggressive" },
        ],
      }),
    );
  });

  it("keeps our black engine at the bottom and presents live search telemetry", async () => {
    const user = userEvent.setup();
    vi.mocked(invoke).mockImplementation((command: string) =>
      command === "get_snapshot"
        ? stampedResolve({
            ...snapshot,
            games: [
              {
                id: "live-game-1",
                accountId: "queenbot",
                botUsername: "QueenBot",
                opponent: "OpponentBot",
                // Four fields `LiveGame` requires that this fixture used to
                // omit, which typing the scripted payload surfaced. A started
                // game has no result and no error, and neither rating is known
                // here.
                botRating: null,
                opponentRating: null,
                result: null,
                error: null,
                color: "black",
                initialFen: "startpos",
                moves: "e2e4",
                status: "started",
                whiteTime: 178000,
                blackTime: 180000,
                whiteIncrement: 2000,
                blackIncrement: 2000,
                clockUpdatedAt: Date.now(),
                engineThinking: true,
                engineLine:
                  "info depth 18 score cp 34 nodes 1200000 nps 3000000 pv e7e5 g1f3",
                engineInfo: {
                  depth: 18,
                  selectiveDepth: 24,
                  scoreCp: 34,
                  // Absent, not null: `EngineTelemetry` skips the field it has
                  // no value for, so `null` is a shape this never arrives in.
                  // Typing the scripted payload is what surfaced that.
                  nodes: 1200000,
                  nodesPerSecond: 3000000,
                  timeMs: 400,
                  hashFull: 318,
                  principalVariation: ["e7e5", "g1f3"],
                  raw: "info depth 18 score cp 34 nodes 1200000 nps 3000000 pv e7e5 g1f3",
                },
              },
            ],
          })
        : Promise.resolve({ id: "challenge-1" }),
    );

    render(<App />);

    const board = await screen.findByLabelText("Current chess position");
    expect(board.children).toHaveLength(64);
    expect(
      screen.getByText("Your engine · Black").closest(".player-row"),
    ).toHaveTextContent("QueenBot");
    expect(
      screen.getByText("Opponent · White").closest(".player-row"),
    ).toHaveTextContent("OpponentBot");
    expect(
      screen.getByLabelText("Engine prefers e7 to e5"),
    ).toBeInTheDocument();
    expect(screen.getAllByText("+0.34")).toHaveLength(2);
    expect(
      screen.getByText("18", { selector: ".telemetry-primary strong" }),
    ).toBeInTheDocument();
    // The principal variation renders as figurines; the visually hidden
    // piece letters keep the SAN readable as text content.
    expect(document.querySelector(".pv-line code")).toHaveTextContent("e5 Nf3");

    await user.click(screen.getByLabelText("Board appearance"));
    await user.click(screen.getByRole("button", { name: "Walnut" }));
    await user.click(screen.getByRole("button", { name: /Blueprint/ }));
    expect(screen.getByRole("button", { name: /^Walnut/ })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    expect(screen.getByRole("button", { name: /Blueprint/ })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    expect(board.closest(".live-panel")).toHaveStyle({
      "--board-light": "#dcc4a4",
      "--board-dark": "#7a563f",
    });
    expect(localStorage.getItem("queenui-board-theme")).toBe("walnut");
    expect(localStorage.getItem("queenui-piece-set")).toBe("blueprint");
  });

  it("filters finished games, shows ratings and result, and exports valid PGN", async () => {
    const user = userEvent.setup();
    const finishedGame: LiveGame = {
      id: "draw-game-1",
      accountId: "queenbot",
      botUsername: "QueenBot",
      opponent: "RepeatBot",
      botRating: 2412,
      opponentRating: 2388,
      color: "white",
      initialFen: "startpos",
      moves: "g1f3 g8f6 f3g1 f6g8 g1f3 g8f6 f3g1 f6g8",
      status: "draw",
      result: "1/2-1/2",
      whiteTime: 160000,
      blackTime: 161000,
      whiteIncrement: 2000,
      blackIncrement: 2000,
      clockUpdatedAt: 1_700_000_000_000,
      engineLine: null,
      engineInfo: null,
      error: null,
      engineThinking: false,
    };
    vi.mocked(invoke).mockImplementation((command: string) =>
      command === "get_snapshot"
        ? stampedResolve({ ...snapshot, games: [finishedGame] })
        : Promise.resolve(undefined),
    );
    vi.mocked(save).mockResolvedValue("C:\\games\\draw-game-1.pgn");

    render(<App />);
    await user.click(await screen.findByRole("button", { name: /^Games/ }));
    expect(screen.getByText("No live games")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Show all games" }));
    /*
     * The archive opens in whichever overview was last chosen, and the grid —
     * the default — draws boards, not ratings and export buttons. Detail is the
     * overview that carries a game's full panel without drilling into it, which
     * is what the rest of this test reads.
     */
    await user.click(screen.getByRole("button", { name: "Detail" }));

    expect(screen.getByText("½ – ½")).toBeInTheDocument();
    expect(screen.getByText("Opponent · Black · 2388")).toBeInTheDocument();
    expect(screen.getByText("Your engine · White · 2412")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Export PGN" }));
    await waitFor(() =>
      expect(vi.mocked(invoke)).toHaveBeenCalledWith("write_pgn_file", {
        path: "C:\\games\\draw-game-1.pgn",
        contents: expect.stringContaining('[Result "1/2-1/2"]'),
      }),
    );
  });

  it("keeps live games in their first-seen order when clocks update", async () => {
    const user = userEvent.setup();
    const game = (
      id: string,
      opponent: string,
      clockUpdatedAt: number,
    ): LiveGame => ({
      id,
      accountId: "queenbot",
      botUsername: "QueenBot",
      opponent,
      color: "white",
      initialFen: "startpos",
      moves: "e2e4",
      status: "started",
      whiteTime: 60_000,
      blackTime: 60_000,
      whiteIncrement: 1_000,
      blackIncrement: 1_000,
      clockUpdatedAt,
      botRating: null,
      opponentRating: null,
      result: null,
      engineLine: null,
      engineInfo: null,
      error: null,
      engineThinking: false,
    });
    const first = game("a-game", "FirstBot", 100);
    const second = game("b-game", "SecondBot", 200);

    vi.mocked(invoke).mockImplementation((command: string) =>
      command === "get_snapshot"
        ? stampedResolve({ ...snapshot, games: [first, second] })
        : Promise.resolve(undefined),
    );
    const emitSnapshot = captureSnapshotListener();

    render(<App />);
    await user.click(await screen.findByRole("button", { name: /^Games/ }));

    // Two live games open the surface in the grid, so the order under test is
    // the order of the tiles. Each tile names both players now, one nameplate
    // per board edge; the opponent is the far one, so that is the plate whose
    // names spell out the tile order.
    const displayedOpponents = () =>
      Array.from(
        document.querySelectorAll(".games-grid .tile-plate-top strong"),
      ).map((name) => name.textContent);
    expect(displayedOpponents()).toEqual(["FirstBot", "SecondBot"]);

    act(() =>
      emitSnapshot({
        ...snapshot,
        games: [
          { ...second, clockUpdatedAt: 300 },
          { ...first, clockUpdatedAt: 400 },
        ],
      }),
    );

    expect(displayedOpponents()).toEqual(["FirstBot", "SecondBot"]);
  });

  it("keeps the dialog open and shows an error toast when the backend rejects", async () => {
    const user = userEvent.setup();
    vi.mocked(invoke).mockImplementation((command: string) =>
      command === "get_snapshot"
        ? stampedResolve(snapshot)
        : Promise.reject(new Error("Lichess refused the challenge")),
    );
    render(<App />);

    await user.click(screen.getByRole("button", { name: "New challenge" }));
    await user.type(screen.getByPlaceholderText("Lichess username"), "SomeBot");
    await user.click(screen.getByRole("button", { name: "Send challenge" }));

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Lichess refused the challenge");
    expect(
      screen.getByRole("dialog", { name: "Create a challenge" }),
    ).toBeInTheDocument();
  });

  it("preserves in-progress engine configuration edits across snapshot events", async () => {
    const user = userEvent.setup();
    vi.mocked(invoke).mockImplementation((command: string) =>
      command === "get_snapshot"
        ? stampedResolve(engineOptionsSnapshot)
        : Promise.resolve(undefined),
    );
    const emitSnapshot = captureSnapshotListener();
    render(<App />);

    await user.click(await screen.findByRole("button", { name: "Engines" }));
    await user.click(screen.getByRole("button", { name: "Configure" }));
    const dialog = screen.getByRole("dialog", { name: "Configure Queen" });
    await user.click(within(dialog).getByRole("tab", { name: /UCI options/ }));
    const hash = within(dialog).getByRole("spinbutton", { name: "Hash" });
    await user.clear(hash);
    await user.type(hash, "1024");

    // A live game constantly re-emits snapshots; edits must survive them.
    act(() => emitSnapshot(structuredClone(engineOptionsSnapshot)));

    expect(hash).toHaveValue(1024);
    // Untouched fields still reflect the latest snapshot values.
    expect(within(dialog).getByRole("combobox", { name: "Style" })).toHaveValue(
      "Normal",
    );
  });

  it("disables New challenge until an engine and a bot account exist", async () => {
    vi.mocked(invoke).mockImplementation((command: string) =>
      command === "get_snapshot"
        ? stampedResolve({ ...snapshot, engines: [], accounts: [] })
        : Promise.resolve(undefined),
    );
    render(<App />);

    await screen.findByRole("heading", {
      name: "Put your engine in the chair.",
    });
    const newChallenge = screen.getByRole("button", { name: "New challenge" });
    expect(newChallenge).toBeDisabled();
    // The reason used to be a `title` on a wrapper span — invisible to the
    // keyboard and unannounced. It is visible text now.
    const hint = screen.getByText(
      "Add a UCI engine and connect a Lichess bot account first",
    );
    expect(newChallenge).toHaveAttribute("aria-describedby", hint.id);
  });
});

/**
 * The hint an operator gets while pasting a token.
 *
 * A play-only token connected cleanly, was stored, and was announced as
 * "connected securely"; matchmaking then answered 403 with nothing on screen to
 * explain it. `add_lichess_account` reports the token's OAuth scopes, and this
 * is the only moment QueenUI sees them — the snapshot has no scope field — so
 * the three answers have to be told apart here or not at all.
 */
describe("Lichess token scope hint", () => {
  const account = {
    id: "queenbot",
    username: "QueenBot",
    engineId: "engine-1",
    rating: 2400,
    enabled: false,
  };

  function scriptConnect(result: unknown, state: AppSnapshot = snapshot) {
    vi.mocked(invoke).mockImplementation((command: string) => {
      if (command === "get_snapshot") return stampedResolve(state);
      if (command === "add_lichess_account") return Promise.resolve(result);
      return Promise.resolve(undefined);
    });
  }

  async function connect(user: ReturnType<typeof userEvent.setup>) {
    await user.click(await screen.findByRole("button", { name: "Add bot" }));
    await user.type(
      screen.getByLabelText("Lichess API token"),
      "lip_pasted_token",
    );
    await user.click(
      screen.getByRole("button", { name: "Validate & connect" }),
    );
  }

  /*
   * The toast, addressed by its own class rather than by role. The blocking
   * grade puts an alert on the account card too — deliberately, since both the
   * announcement and the record are alerts — so `getByRole("alert")` is
   * ambiguous in exactly the case that matters most.
   */
  const toastElement = () => document.querySelector(".toast");

  it("stays quiet when the token carries the whole required set", async () => {
    const user = userEvent.setup();
    scriptConnect({
      account,
      scopes: ["bot:play", "challenge:read", "challenge:write"],
      missingForMatchmaking: [],
      canPlayGames: true,
    });
    render(<App />);

    await connect(user);

    expect(
      await screen.findByText("Lichess BOT account connected securely"),
    ).toBeInTheDocument();
    // Nothing is added to a clean flow: no card notice, no scope lecture.
    expect(document.querySelector(".scope-gap")).toBeNull();
    expect(screen.queryByText(/matchmaking will not work/)).toBeNull();
  });

  it("warns on the missing challenge scopes without claiming the connect failed", async () => {
    const user = userEvent.setup();
    scriptConnect({
      account,
      scopes: ["bot:play"],
      missingForMatchmaking: ["challenge:read", "challenge:write"],
      canPlayGames: true,
    });
    render(<App />);

    await connect(user);

    await waitFor(() => expect(toastElement()).not.toBeNull());
    const toast = toastElement();
    expect(toast).toHaveTextContent(
      "QueenBot can play, but matchmaking is off",
    );
    expect(toast).toHaveTextContent(
      "Missing scopes challenge:read, challenge:write — matchmaking will not work with this token.",
    );
    expect(toast).toHaveTextContent(
      "Create a new token at lichess.org/account/oauth/token/create with Play-bot, Read-challenges and Send-challenges ticked",
    );
    // A warning, not a failure and not a receipt: the account was stored, so
    // the dialog closes — and it does not expire on its own.
    expect(toast).toHaveClass("toast-warning");
    expect(
      screen.getByRole("button", { name: "Dismiss warning" }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("dialog", { name: "Connect Lichess BOT" }),
    ).toBeNull();
    expect(screen.queryByText(/connected securely/)).toBeNull();
    // And the same verdict is on the card, where it outlives the toast.
    expect(document.querySelector(".scope-gap")).toHaveTextContent(
      "Missing scopes challenge:read, challenge:write",
    );
  });

  it("reports a token that cannot play at all as an error, not a warning", async () => {
    const user = userEvent.setup();
    scriptConnect({
      account,
      scopes: ["challenge:read", "challenge:write"],
      missingForMatchmaking: ["bot:play"],
      canPlayGames: false,
    });
    render(<App />);

    await connect(user);

    await waitFor(() => expect(toastElement()).not.toBeNull());
    const toast = toastElement();
    expect(toast).toHaveClass("toast-error");
    expect(toast).toHaveAttribute("role", "alert");
    expect(toast).toHaveTextContent("QueenBot cannot play with this token");
    expect(toast).toHaveTextContent(
      "Missing scope bot:play — QueenUI cannot play games with this token, and matchmaking will not work either.",
    );
    // The backend stores the account before it looks at scopes, so the connect
    // did happen — but nothing may say a bot is ready to play.
    expect(screen.queryByText(/connected securely/)).toBeNull();
    const card = document.querySelector(".scope-gap");
    expect(card).toHaveClass("scope-gap-blocking");
    expect(card).toHaveTextContent("Missing scope bot:play");
  });

  it("keeps the notice on the card through snapshot pushes and a restart", async () => {
    const user = userEvent.setup();
    scriptConnect({
      account,
      scopes: ["bot:play", "challenge:read"],
      missingForMatchmaking: ["challenge:write"],
      canPlayGames: true,
    });
    const emitSnapshot = captureSnapshotListener();
    const first = render(<App />);

    await connect(user);
    await screen.findByRole("alert");

    // A live game re-emits snapshots constantly; the token is unchanged.
    act(() =>
      emitSnapshot({
        ...structuredClone(snapshot),
        runtimes: [{ accountId: "queenbot", status: "playing", error: null }],
      }),
    );
    expect(document.querySelector(".scope-gap")).toHaveTextContent(
      "Missing scope challenge:write",
    );

    /*
     * And across a restart. The gap is a property of the stored token, not of
     * this session: holding it in component state would show a clean fleet
     * tomorrow morning over a token that is exactly as short a scope as it was
     * tonight, which is the false all-clear this whole round exists to remove.
     */
    first.unmount();
    render(<App />);
    await waitFor(() =>
      expect(document.querySelector(".scope-gap")).toHaveTextContent(
        "Missing scope challenge:write — matchmaking will not work with this token.",
      ),
    );
  });

  it("takes the notice off the card when a complete token replaces the old one", async () => {
    const user = userEvent.setup();
    scriptConnect({
      account,
      scopes: ["bot:play"],
      missingForMatchmaking: ["challenge:read", "challenge:write"],
      canPlayGames: true,
    });
    render(<App />);

    await connect(user);
    expect(await screen.findByRole("alert")).toBeInTheDocument();
    expect(document.querySelector(".scope-gap")).not.toBeNull();

    scriptConnect({
      account,
      scopes: ["bot:play", "challenge:read", "challenge:write"],
      missingForMatchmaking: [],
      canPlayGames: true,
    });
    await connect(user);

    expect(
      await screen.findByText("Lichess BOT account connected securely"),
    ).toBeInTheDocument();
    expect(document.querySelector(".scope-gap")).toBeNull();
  });

  it("does not warn about a token it just deleted", async () => {
    const user = userEvent.setup();
    // Stopped from the start: disconnecting a running bot is refused, and the
    // point of this test is the removal, not the refusal.
    scriptConnect(
      {
        account,
        scopes: ["bot:play"],
        missingForMatchmaking: ["challenge:read", "challenge:write"],
        canPlayGames: true,
      },
      {
        ...snapshot,
        runtimes: [{ accountId: "queenbot", status: "stopped", error: null }],
      },
    );
    render(<App />);
    await connect(user);
    await waitFor(() => expect(toastElement()).not.toBeNull());

    await user.click(
      await screen.findByRole("button", { name: "Actions for QueenBot" }),
    );
    await user.click(
      await screen.findByRole("menuitem", { name: /Disconnect account/ }),
    );
    await user.click(
      screen.getByRole("button", { name: "Disconnect and delete token" }),
    );

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("remove_lichess_account", {
        accountId: "queenbot",
      }),
    );
    await waitFor(() =>
      expect(document.querySelector(".scope-gap")).toBeNull(),
    );
  });
});

/**
 * Replacing a token without taking the account apart.
 *
 * The only way to fix a revoked, expired or under-scoped token used to be
 * disconnect-and-reconnect: `remove_lichess_account` deletes the secret and
 * drops the account's campaign, and `add_lichess_account` then rebuilds the
 * profile from the connect dialog — including reassigning the engine to
 * whatever its picker happened to be showing. Operators lost settings while
 * fixing a token. `update_lichess_account_token` writes the secret and nothing
 * else, and these tests are about the two things that makes possible: the
 * account survives, and the scope verdict is still learned.
 */
describe("replacing an account's Lichess token", () => {
  const account = {
    id: "queenbot",
    username: "QueenBot",
    engineId: "engine-1",
    rating: 2400,
    enabled: true,
  };

  /** The gap map as it stands on disk, which is where it survives a restart. */
  const storedGaps = () =>
    JSON.parse(localStorage.getItem("queenui-token-scope-gaps") ?? "{}");

  function scriptReplace(answer: unknown) {
    // Reset first: the call history is shared across this file, and one of the
    // assertions below is about commands that were *not* sent.
    vi.mocked(invoke).mockReset();
    vi.mocked(invoke).mockImplementation((command: string) => {
      if (command === "get_snapshot") return stampedResolve(snapshot);
      if (command === "update_lichess_account_token") {
        return answer instanceof Error
          ? Promise.reject(answer)
          : Promise.resolve(answer);
      }
      return Promise.resolve(undefined);
    });
  }

  async function replaceToken(user: ReturnType<typeof userEvent.setup>) {
    await user.click(
      await screen.findByRole("button", { name: "Actions for QueenBot" }),
    );
    await user.click(
      await screen.findByRole("menuitem", { name: /Replace token/ }),
    );
    await user.type(
      await screen.findByLabelText("New Lichess API token"),
      "lip_replacement",
    );
    await user.click(
      screen.getByRole("button", { name: "Validate & replace" }),
    );
  }

  const toastElement = () => document.querySelector(".toast");

  it("is offered while the bot is running, and sends the account beside the token", async () => {
    /*
     * The fleet snapshot has QueenBot online. Requiring a stop first — as the
     * disconnect entry and the engine picker both do — would make this useless
     * in the case it exists for: a token revoked under a bot that is playing.
     * Replacing it touches the stored secret only, and a game already running
     * holds the client it started with.
     */
    const user = userEvent.setup();
    scriptReplace({
      account,
      scopes: ["bot:play", "challenge:read", "challenge:write"],
      missingForMatchmaking: [],
      canPlayGames: true,
    });
    render(<App />);

    await user.click(
      await screen.findByRole("button", { name: "Actions for QueenBot" }),
    );
    const item = await screen.findByRole("menuitem", { name: /Replace token/ });
    expect(item).not.toHaveAttribute("aria-disabled", "true");
    await user.click(item);
    await user.type(
      await screen.findByLabelText("New Lichess API token"),
      "lip_replacement",
    );
    await user.click(
      screen.getByRole("button", { name: "Validate & replace" }),
    );

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("update_lichess_account_token", {
        accountId: "queenbot",
        token: "lip_replacement",
      }),
    );
    // Nothing else was touched: no disconnect, no re-add, no restart.
    const called = vi.mocked(invoke).mock.calls.map(([name]) => name);
    expect(called).not.toContain("remove_lichess_account");
    expect(called).not.toContain("add_lichess_account");
    expect(called).not.toContain("stop_bot");
  });

  it("reports the replacement in terms of what it did not change", async () => {
    const user = userEvent.setup();
    scriptReplace({
      account,
      scopes: ["bot:play", "challenge:read", "challenge:write"],
      missingForMatchmaking: [],
      canPlayGames: true,
    });
    render(<App />);

    await replaceToken(user);

    expect(await screen.findByText(/token replaced/)).toHaveTextContent(
      "QueenBot's token replaced — games and matchmaking already running keep the old token; the new one is used from the next start.",
    );
    // The dialog promised the running games would be left alone; the receipt
    // says the same thing rather than implying the swap took effect at once.
    expect(
      screen.queryByRole("dialog", { name: /Replace QueenBot/ }),
    ).toBeNull();
  });

  it("takes the card notice off an account whose new token is complete", async () => {
    /*
     * The persisted gap map describes the token the account is *currently*
     * holding, so a replacement has to be able to clear it. Seeded from disk
     * here exactly as a restart would seed it: this is the warning an operator
     * has been looking at since yesterday, and the replacement answers it.
     */
    const user = userEvent.setup();
    localStorage.setItem(
      "queenui-token-scope-gaps",
      JSON.stringify({
        queenbot: { missing: ["challenge:write"], canPlayGames: true },
      }),
    );
    scriptReplace({
      account,
      scopes: ["bot:play", "challenge:read", "challenge:write"],
      missingForMatchmaking: [],
      canPlayGames: true,
    });
    render(<App />);

    await waitFor(() =>
      expect(document.querySelector(".scope-gap")).toHaveTextContent(
        "Missing scope challenge:write",
      ),
    );

    await replaceToken(user);

    await waitFor(() =>
      expect(document.querySelector(".scope-gap")).toBeNull(),
    );
    // And on disk, so it does not come back at the next launch.
    await waitFor(() => expect(storedGaps()).toStrictEqual({}));
  });

  it("records a gap the replacement introduces, instead of a success tick", async () => {
    /*
     * The other direction, and the reason a replacement answers with the scope
     * envelope at all: a token minted in a hurry with only the play box ticked
     * silently ends matchmaking. Announcing that as "token replaced" would hide
     * it until a campaign answered 403.
     */
    const user = userEvent.setup();
    scriptReplace({
      account,
      scopes: ["bot:play"],
      missingForMatchmaking: ["challenge:read", "challenge:write"],
      canPlayGames: true,
    });
    render(<App />);

    await replaceToken(user);

    await waitFor(() => expect(toastElement()).not.toBeNull());
    expect(toastElement()).toHaveClass("toast-warning");
    expect(toastElement()).toHaveTextContent(
      "QueenBot can play, but matchmaking is off",
    );
    expect(screen.queryByText(/token replaced/)).toBeNull();
    expect(document.querySelector(".scope-gap")).toHaveTextContent(
      "Missing scopes challenge:read, challenge:write",
    );
    await waitFor(() =>
      expect(storedGaps()).toStrictEqual({
        queenbot: {
          missing: ["challenge:read", "challenge:write"],
          canPlayGames: true,
        },
      }),
    );
  });

  it("points a scope gap at this dialog rather than at a reconnect", async () => {
    /*
     * The remedy sentence used to end "then connect the account again", which
     * was the only route there was and the destructive one. Now that a token
     * can be swapped in place, the advice names that instead.
     */
    const user = userEvent.setup();
    scriptReplace({
      account,
      scopes: ["challenge:read", "challenge:write"],
      missingForMatchmaking: ["bot:play"],
      canPlayGames: false,
    });
    render(<App />);

    await replaceToken(user);

    await waitFor(() => expect(toastElement()).not.toBeNull());
    expect(toastElement()).toHaveTextContent(
      "then replace this account's token from its Actions menu on Overview.",
    );
    expect(toastElement()).not.toHaveTextContent("connect the account again");
  });

  it("quotes the wrong-account refusal verbatim and keeps the dialog open", async () => {
    /*
     * The backend validates the pasted token against Lichess and refuses it
     * when it belongs to someone else, rather than repointing the profile at
     * whoever the token turns out to be. Nothing was stored, so nothing may be
     * recorded about it either — and the dialog stays up on the paste that
     * failed, because closing it would look exactly like a replacement that
     * worked.
     */
    const user = userEvent.setup();
    localStorage.setItem(
      "queenui-token-scope-gaps",
      JSON.stringify({
        queenbot: { missing: ["challenge:write"], canPlayGames: true },
      }),
    );
    scriptReplace(
      new Error(
        "The Lichess token belongs to @OtherBot (otherbot), but the selected account is @QueenBot (queenbot).",
      ),
    );
    render(<App />);

    await replaceToken(user);

    await waitFor(() => expect(toastElement()).toHaveClass("toast-error"));
    expect(toastElement()).toHaveTextContent(
      "Could not replace QueenBot's token — The Lichess token belongs to @OtherBot (otherbot), but the selected account is @QueenBot (queenbot).",
    );
    expect(
      screen.getByRole("dialog", { name: /Replace QueenBot/ }),
    ).toBeInTheDocument();
    // The old token is still in place, so its verdict is still the truth.
    expect(storedGaps()).toStrictEqual({
      queenbot: { missing: ["challenge:write"], canPlayGames: true },
    });
    expect(document.querySelector(".scope-gap")).toHaveTextContent(
      "Missing scope challenge:write",
    );
  });

  it("says a runner is too old for the command instead of claiming a swap", async () => {
    const user = userEvent.setup();
    scriptReplace(
      new Error(
        "The connected runner does not support updateLichessAccountToken. Update queen-runner and try again.",
      ),
    );
    render(<App />);

    await replaceToken(user);

    await waitFor(() => expect(toastElement()).toHaveClass("toast-error"));
    expect(toastElement()).toHaveTextContent(
      "The connected runner does not support updateLichessAccountToken. Update queen-runner and try again.",
    );
    expect(screen.queryByText(/token replaced/)).toBeNull();
  });
});

/**
 * Games that died, and the button that forgets one.
 *
 * A failed game used to be excluded by `is_live` and then pruned away with the
 * finished games, so a board vanished mid-game and every screen agreed nothing
 * had happened. They are retained now until `dismiss_game_error` removes them,
 * which makes the dismissal the only way one leaves — and makes a dismissal
 * that silently failed the same bug all over again.
 */
describe("retained game errors", () => {
  const failedGame: LiveGame = {
    id: "F5tD1jRk",
    accountId: "queenbot",
    botUsername: "QueenBot",
    opponent: "GambitFalcon",
    botRating: 2400,
    opponentRating: 2361,
    color: "white",
    initialFen: "startpos",
    moves: "e2e4 e7e5",
    status: "error",
    whiteTime: 60_000,
    blackTime: 60_000,
    whiteIncrement: 0,
    blackIncrement: 0,
    clockUpdatedAt: 0,
    result: null,
    engineLine: null,
    engineInfo: null,
    engineThinking: false,
    error: "Engine process exited during search (exit code 0xC0000005).",
  };

  function scriptDismiss(failure?: Error) {
    vi.mocked(invoke).mockReset();
    vi.mocked(invoke).mockImplementation((command: string) => {
      if (command === "get_snapshot") {
        return stampedResolve({ ...snapshot, games: [failedGame] });
      }
      if (command === "dismiss_game_error" && failure) {
        return Promise.reject(failure);
      }
      return Promise.resolve(undefined);
    });
  }

  it("dismisses the game the operator pointed at", async () => {
    const user = userEvent.setup();
    scriptDismiss();
    render(<App />);

    await user.click(screen.getByRole("button", { name: "Games" }));
    await user.click(
      await screen.findByRole("button", {
        name: "Dismiss the failed game F5tD1jRk",
      }),
    );

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("dismiss_game_error", {
        gameId: "F5tD1jRk",
      }),
    );
  });

  it("keeps the failure on screen when the runner refuses to forget it", async () => {
    /*
     * An older runner answers this command with a refusal, and the card is the
     * only place the error text exists on this screen. Removing it on the click
     * rather than on the next snapshot would destroy the evidence while leaving
     * the game retained on the runner — wrong in both directions at once.
     */
    const user = userEvent.setup();
    scriptDismiss(
      new Error(
        "The connected runner does not support dismissGameError. Update queen-runner and try again.",
      ),
    );
    render(<App />);

    await user.click(screen.getByRole("button", { name: "Games" }));
    await user.click(
      await screen.findByRole("button", {
        name: "Dismiss the failed game F5tD1jRk",
      }),
    );

    await waitFor(() =>
      expect(document.querySelector(".toast")).toHaveClass("toast-error"),
    );
    expect(document.querySelector(".toast")).toHaveTextContent(
      "Could not dismiss the error for game F5tD1jRk — The connected runner does not support dismissGameError. Update queen-runner and try again.",
    );
    expect(
      screen.getByRole("button", { name: "Dismiss the failed game F5tD1jRk" }),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        "Engine process exited during search (exit code 0xC0000005).",
      ),
    ).toBeInTheDocument();
  });

  it("does not let a dead game light the live badge", async () => {
    /*
     * The sidebar badge quotes `countLiveGames`, which is also what the close
     * guard uses. A retained failure counted as live would keep a badge lit
     * over a game that has stopped, and block a quit on it.
     */
    scriptDismiss();
    render(<App />);

    const games = await screen.findByRole("button", { name: /^Games/ });
    expect(games).not.toHaveTextContent("1");
    expect(games).not.toHaveTextContent("live game");
  });
});
