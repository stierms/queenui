import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { CloseGuard } from "./CloseGuard";
import type { LiveGame } from "../types";

function makeGame(overrides: Partial<LiveGame> = {}): LiveGame {
  return {
    id: "P7vQ9kLm",
    accountId: "acct",
    botUsername: "QueenBot",
    opponent: "TacticalRaven",
    color: "white",
    initialFen: "startpos",
    moves: "",
    status: "started",
    whiteTime: 92_000,
    blackTime: 145_000,
    whiteIncrement: 2000,
    blackIncrement: 2000,
    clockUpdatedAt: 0,
    botRating: null,
    opponentRating: null,
    result: null,
    engineLine: null,
    engineInfo: null,
    error: null,
    engineThinking: false,
    ...overrides,
  };
}

function renderGuard(games: LiveGame[], pending = false) {
  const onKeepPlaying = vi.fn();
  const onClose = vi.fn();
  render(
    <CloseGuard
      games={games}
      pending={pending}
      onKeepPlaying={onKeepPlaying}
      onClose={onClose}
    />,
  );
  return { onKeepPlaying, onClose };
}

afterEach(cleanup);

describe("CloseGuard", () => {
  it("names the consequence rather than asking a bare are-you-sure", () => {
    renderGuard([makeGame(), makeGame({ id: "K3xR8dTa" })]);
    expect(
      screen.getByRole("heading", { name: "2 games are still being played" }),
    ).toBeInTheDocument();
    expect(screen.getByText(/lost on\s+time/)).toBeInTheDocument();
  });

  it("reads naturally for a single game", () => {
    renderGuard([makeGame()]);
    expect(
      screen.getByRole("heading", { name: "A game is still being played" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Close and abandon the game" }),
    ).toBeInTheDocument();
  });

  it("lists each game with our clock and whose move it is", () => {
    renderGuard([
      makeGame(),
      makeGame({
        id: "K3xR8dTa",
        opponent: "SilentBishop",
        color: "black",
        moves: "e2e4",
      }),
    ]);
    const rows = screen.getAllByRole("listitem");

    // We are white with no moves played, so it is our move and our clock runs.
    expect(within(rows[0]).getByText(/your move/)).toBeInTheDocument();
    expect(within(rows[0]).getByText("01:32")).toBeInTheDocument();
    // Black after 1. e4 is also to move.
    expect(within(rows[1]).getByText("SilentBishop")).toBeInTheDocument();
    expect(within(rows[1]).getByText(/Black · your move/)).toBeInTheDocument();
  });

  it("keeps playing by default and only closes on the explicit choice", async () => {
    const user = userEvent.setup();
    const { onKeepPlaying, onClose } = renderGuard([makeGame()]);

    // The safe action holds focus, so Enter or Escape never abandons a game.
    expect(screen.getByRole("button", { name: "Keep playing" })).toHaveFocus();
    await user.keyboard("{Escape}");
    expect(onKeepPlaying).toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: /Close and abandon/ }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("disables closing while the request is in flight", () => {
    renderGuard([makeGame()], true);
    expect(
      screen.getByRole("button", { name: /Close and abandon/ }),
    ).toBeDisabled();
  });
});
