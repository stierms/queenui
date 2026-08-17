import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { OverviewPage } from "./OverviewPage";
import { initialConnectionState } from "../lib/connection";
import { emptySnapshot, type AppSnapshot } from "../types";

const snapshot: AppSnapshot = {
  ...emptySnapshot,
  engines: [
    {
      id: "engine-1",
      name: "Queen",
      path: "q.exe",
      author: null,
      optionCount: 3,
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
  runtimes: [{ accountId: "queenbot", status: "stopped", error: null }],
};

function renderPage(overrides: Partial<Parameters<typeof OverviewPage>[0]>) {
  const props = {
    snapshot,
    connectedCount: 0,
    loading: false,
    busy: new Set<string>(),
    onAddEngine: () => {},
    onAddAccount: () => {},
    onChallenge: () => {},
    onToggle: () => {},
    onAssignEngine: () => {},
    onNavigate: () => {},
    moveSoundsEnabled: false,
    onToggleMoveSounds: () => {},
    boardTheme: "forest" as const,
    pieceSet: "regal" as const,
    onBoardThemeChange: () => {},
    onPieceSetChange: () => {},
    onExportPgn: () => {},
    ...overrides,
  };
  return render(<OverviewPage {...props} />);
}

afterEach(cleanup);

describe("backend availability branch", () => {
  it("shows the unreachable panel before the first-run onboarding", () => {
    // An unreachable backend leaves the snapshot empty, and an empty snapshot
    // used to be read as a fresh install.
    renderPage({
      snapshot: emptySnapshot,
      unavailable: true,
      connection: {
        ...initialConnectionState,
        backendUnavailable: true,
        backendDetail: "runner did not come up",
      },
      onRetry: () => {},
    });

    expect(
      screen.getByRole("heading", { name: "QueenUI can't reach its backend" }),
    ).toBeInTheDocument();
    expect(screen.getByText("runner did not come up")).toBeInTheDocument();
    expect(
      screen.queryByRole("heading", { name: "Put your engine in the chair." }),
    ).not.toBeInTheDocument();
  });

  it("still offers onboarding when the backend answers with nothing configured", () => {
    renderPage({ snapshot: emptySnapshot });
    expect(
      screen.getByRole("heading", { name: "Put your engine in the chair." }),
    ).toBeInTheDocument();
  });

  it("says it is waiting for the new runner rather than offering onboarding", () => {
    /*
     * The same trap one step further in. A backend generation change empties the
     * snapshot on purpose — the fleet it held was the previous runner's — and an
     * empty snapshot still reads as a fresh install here. It is also not the
     * unreachable-backend screen: the service is answering, and the previous
     * runner's games are being played on a machine this app can no longer see.
     */
    renderPage({
      snapshot: emptySnapshot,
      awaitingBackend: true,
      connection: {
        ...initialConnectionState,
        backendGeneration: 2,
        awaitingBackendData: true,
        link: "disconnected",
        detail: "connection refused",
      },
      onRetry: () => {},
    });

    expect(
      screen.getByRole("heading", { name: "Waiting for the game runner" }),
    ).toBeInTheDocument();
    expect(screen.getByText("connection refused")).toBeInTheDocument();
    expect(
      screen.queryByRole("heading", { name: "Put your engine in the chair." }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("heading", {
        name: "QueenUI can't reach its backend",
      }),
    ).not.toBeInTheDocument();
  });

  it("shows the unreachable backend first when both are true", () => {
    // A dead backend is the stronger fact: every command will fail too, and its
    // screen is the one with the detail that explains why nothing works.
    renderPage({
      snapshot: emptySnapshot,
      unavailable: true,
      awaitingBackend: true,
      connection: {
        ...initialConnectionState,
        backendUnavailable: true,
        backendDetail: "ipc bridge is gone",
        awaitingBackendData: true,
      },
      onRetry: () => {},
    });

    expect(
      screen.getByRole("heading", { name: "QueenUI can't reach its backend" }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("heading", { name: "Waiting for the game runner" }),
    ).not.toBeInTheDocument();
  });
});

describe("onboarding copy follows the runner", () => {
  it("describes a local Windows engine in embedded mode", () => {
    renderPage({ snapshot: emptySnapshot });
    expect(screen.getByText(/Windows UCI engine/)).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /Choose UCI engine executable/ }),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        "Engine probing occurs on this PC. Nothing is uploaded.",
      ),
    ).toBeInTheDocument();
  });

  it("names the trusted-engine browse flow in remote mode, not an upload", () => {
    /*
     * The local copy used to tell a remote operator the opposite of what would
     * happen; its replacement then promised an upload, which trusted-engine
     * mode refuses (`/v2/engines/upload` answers `engine_install_disabled`).
     * Browsing an administrator-configured root is the only remote add flow
     * the app has.
     */
    renderPage({ snapshot: emptySnapshot, remoteRunner: true });
    expect(
      screen.getByText(/engine roots the runner's administrator configured/),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /Browse engines on the runner/ }),
    ).toBeInTheDocument();
    expect(screen.queryByText(/Windows UCI engine/)).not.toBeInTheDocument();
    expect(screen.queryByText(/Nothing is uploaded/)).not.toBeInTheDocument();
    expect(screen.queryByText(/upload/i)).not.toBeInTheDocument();
    // The runner's operating system is something QueenUI reads from the
    // runner and shows in Settings, not a guess the first-run screen makes.
    expect(screen.queryByText(/Linux/)).not.toBeInTheDocument();
  });

  it("points the fleet action at the runner instead of a local file", () => {
    // The ghost button opens no add flow in remote mode — it navigates to
    // Engines, where the trusted-engine browser is.
    renderPage({ remoteRunner: true });
    expect(
      screen.getByRole("button", { name: "Runner engines" }),
    ).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Add engine" })).toBeNull();
  });
});

describe("account removal", () => {
  it("asks before deleting the stored Lichess token", async () => {
    const user = userEvent.setup();
    const onRemoveAccount = vi.fn(() => Promise.resolve(true));
    renderPage({ onRemoveAccount });

    await user.click(
      screen.getByRole("button", { name: "Actions for QueenBot" }),
    );
    await user.click(
      await screen.findByRole("menuitem", { name: /Disconnect account/ }),
    );

    expect(
      await screen.findByRole("heading", { name: "Disconnect QueenBot?" }),
    ).toBeInTheDocument();
    // Local mode: the token really is in the Windows credential store.
    expect(
      screen.getByText(/deletes the stored Lichess token from Windows/),
    ).toBeInTheDocument();
    expect(onRemoveAccount).not.toHaveBeenCalled();

    await user.click(
      screen.getByRole("button", { name: "Disconnect and delete token" }),
    );
    await waitFor(() => expect(onRemoveAccount).toHaveBeenCalledTimes(1));
    expect(onRemoveAccount).toHaveBeenCalledWith(
      expect.objectContaining({ id: "queenbot" }),
    );
  });

  it("names the runner, not this PC, as where a remote token is deleted from", async () => {
    /*
     * In remote mode the token was handed to the runner and stored there; the
     * local credential store is not involved. The confirmation used to describe
     * the deletion with no location at all, and the one sentence an operator
     * reads before deleting a secret should say which machine loses it.
     */
    const user = userEvent.setup();
    renderPage({
      onRemoveAccount: vi.fn(() => Promise.resolve(true)),
      remoteRunner: true,
      runnerUrl: "https://runner-host:17789",
    });

    await user.click(
      screen.getByRole("button", { name: "Actions for QueenBot" }),
    );
    await user.click(
      await screen.findByRole("menuitem", { name: /Disconnect account/ }),
    );

    await screen.findByRole("heading", { name: "Disconnect QueenBot?" });
    expect(
      screen.getByText(
        /token from the runner machine \(https:\/\/runner-host:17789\)/,
      ),
    ).toBeInTheDocument();
    expect(screen.queryByText(/Windows Credential Manager/)).toBeNull();
  });

  it("cancels without touching the token", async () => {
    const user = userEvent.setup();
    const onRemoveAccount = vi.fn(() => Promise.resolve(true));
    renderPage({ onRemoveAccount });

    await user.click(
      screen.getByRole("button", { name: "Actions for QueenBot" }),
    );
    await user.click(
      await screen.findByRole("menuitem", { name: /Disconnect account/ }),
    );
    await user.click(await screen.findByRole("button", { name: "Cancel" }));

    await waitFor(() =>
      expect(
        screen.queryByRole("heading", { name: "Disconnect QueenBot?" }),
      ).not.toBeInTheDocument(),
    );
    expect(onRemoveAccount).not.toHaveBeenCalled();
  });

  it("refuses to disconnect a bot that is still connected", async () => {
    const user = userEvent.setup();
    renderPage({
      snapshot: {
        ...snapshot,
        runtimes: [{ accountId: "queenbot", status: "playing", error: null }],
      },
      onRemoveAccount: vi.fn(() => Promise.resolve(true)),
    });

    await user.click(
      screen.getByRole("button", { name: "Actions for QueenBot" }),
    );
    const item = await screen.findByRole("menuitem", {
      name: /Disconnect account/,
    });
    expect(item).toHaveAttribute("data-disabled");
    expect(item).toHaveTextContent("Stop the bot first");
  });
});

describe("the token scope notice on the account card", () => {
  /*
   * The remedy names the row's own Actions menu, not a reconnect. Reconnecting
   * an existing account rewrites its whole profile from the connect dialog —
   * the engine assignment included — while `update_lichess_account_token`
   * changes the secret and nothing else, so the advice on a card points at the
   * action that keeps the card.
   */
  const remedy =
    "Create a new token at lichess.org/account/oauth/token/create with " +
    "Play-bot, Read-challenges and Send-challenges ticked, then replace this " +
    "account's token from its Actions menu on Overview.";

  it("says nothing when the token carries the whole required set", () => {
    // The quiet path. A card that is fine has no notice on it at all — the
    // absence is the signal, and adding a green "all scopes present" strip to
    // every healthy account would make the one broken card harder to spot.
    renderPage({ tokenScopeGaps: {} });
    expect(document.querySelector(".scope-gap")).toBeNull();
    expect(screen.queryByText(/matchmaking will not work/)).toBeNull();
  });

  it("names both missing challenge scopes and keeps the bot's start control", () => {
    renderPage({
      tokenScopeGaps: {
        queenbot: {
          missing: ["challenge:read", "challenge:write"],
          canPlayGames: true,
        },
      },
    });

    const notice = document.querySelector(".scope-gap");
    expect(notice).not.toBeNull();
    expect(notice).toHaveTextContent(
      "QueenBot can play, but matchmaking is off",
    );
    expect(notice).toHaveTextContent(
      "Missing scopes challenge:read, challenge:write — matchmaking will not work with this token.",
    );
    expect(notice).toHaveTextContent(remedy);
    // Non-blocking: the token really does play, so nothing about the card is
    // disabled and the notice is a status rather than an alert.
    expect(notice).toHaveAttribute("role", "status");
    expect(notice).not.toHaveClass("scope-gap-blocking");
    expect(screen.getByRole("button", { name: "Start" })).toBeEnabled();
  });

  it("grades a token without bot:play as blocking, naming that scope", () => {
    renderPage({
      tokenScopeGaps: {
        queenbot: { missing: ["bot:play"], canPlayGames: false },
      },
    });

    const notice = document.querySelector(".scope-gap");
    expect(notice).toHaveTextContent("QueenBot cannot play with this token");
    expect(notice).toHaveTextContent(
      "Missing scope bot:play — QueenUI cannot play games with this token, and matchmaking will not work either.",
    );
    expect(notice).toHaveTextContent(remedy);
    expect(notice).toHaveAttribute("role", "alert");
    expect(notice).toHaveClass("scope-gap-blocking");
  });

  it("stays on the card across a snapshot push", () => {
    /*
     * The fact this reports is a property of a stored token: it is true until
     * someone mints a new one. A live game re-emits snapshots every few
     * hundred milliseconds, and a notice that only survived until the next one
     * would be a toast wearing a card's clothes.
     */
    const gaps = {
      queenbot: { missing: ["challenge:write"], canPlayGames: true },
    };
    const { rerender } = renderPage({ tokenScopeGaps: gaps });
    expect(document.querySelector(".scope-gap")?.textContent).toContain(
      "challenge:write",
    );

    rerender(
      <OverviewPage
        snapshot={{
          ...snapshot,
          runtimes: [{ accountId: "queenbot", status: "playing", error: null }],
        }}
        connectedCount={1}
        loading={false}
        busy={new Set<string>()}
        tokenScopeGaps={gaps}
        onAddEngine={() => {}}
        onAddAccount={() => {}}
        onChallenge={() => {}}
        onToggle={() => {}}
        onAssignEngine={() => {}}
        onNavigate={() => {}}
        moveSoundsEnabled={false}
        onToggleMoveSounds={() => {}}
        boardTheme="forest"
        pieceSet="regal"
        onBoardThemeChange={() => {}}
        onPieceSetChange={() => {}}
        onExportPgn={() => {}}
      />,
    );

    const notice = document.querySelector(".scope-gap");
    expect(notice).toHaveTextContent(
      "Missing scope challenge:write — matchmaking will not work with this token.",
    );
    // The bot is playing now, which is exactly the state that makes the gap
    // easy to forget: games work, the campaign still cannot start one.
    expect(screen.getByRole("button", { name: "Stop" })).toBeInTheDocument();
  });

  it("keeps a runtime error and a scope gap as two separate facts", () => {
    // One is what Lichess refused just now; the other is what the token has
    // never been able to do. Neither replaces the other.
    renderPage({
      snapshot: {
        ...snapshot,
        runtimes: [
          { accountId: "queenbot", status: "error", error: "stream closed" },
        ],
      },
      tokenScopeGaps: {
        queenbot: { missing: ["challenge:read"], canPlayGames: true },
      },
    });

    expect(document.querySelector(".runtime-error")).toHaveTextContent(
      "stream closed",
    );
    expect(document.querySelector(".scope-gap")).toHaveTextContent(
      "challenge:read",
    );
  });
});

describe("live game count", () => {
  const game = {
    id: "g1",
    accountId: "queenbot",
    botUsername: "QueenBot",
    opponent: "Rival",
    botRating: 2400,
    opponentRating: 2380,
    color: "white",
    initialFen: "startpos",
    moves: "",
    whiteTime: 60_000,
    blackTime: 60_000,
    whiteIncrement: 0,
    blackIncrement: 0,
    clockUpdatedAt: 0,
    result: null,
    engineLine: null,
    engineInfo: null,
    engineThinking: false,
    error: null,
  };

  it("counts a game Lichess still reports as created", () => {
    // `created` is the window between challenge acceptance and the first move.
    // The strip counted only `started`, so it read "0 live games" while the
    // sidebar badge, the close guard and the board itself all said one.
    renderPage({
      snapshot: { ...snapshot, games: [{ ...game, status: "created" }] },
    });
    expect(screen.getByText("1 live game")).toBeInTheDocument();
  });

  it("does not count a finished game", () => {
    renderPage({
      snapshot: {
        ...snapshot,
        games: [{ ...game, status: "mate", result: "1-0" }],
      },
    });
    expect(screen.getByText("0 live games")).toBeInTheDocument();
  });
});

describe("stale rendering", () => {
  it("marks the status strip and stops claiming anything is live", () => {
    renderPage({ stale: true, connectedCount: 1 });
    expect(screen.getByText("Last known")).toBeInTheDocument();
    expect(document.querySelector(".strip-dot.live")).toBeNull();
  });

  it("shows the live dot when the connection is healthy", () => {
    renderPage({ connectedCount: 1 });
    expect(screen.queryByText("Last known")).not.toBeInTheDocument();
    expect(document.querySelector(".strip-dot.live")).not.toBeNull();
  });
});
