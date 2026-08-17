import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { LiveGamePanel } from "./LiveGamePanel";
import type { LiveGame } from "../types";

const blackToMoveFen =
  "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1";

function makeGame(overrides: Partial<LiveGame> = {}): LiveGame {
  return {
    id: "fen-game",
    accountId: "acct",
    botUsername: "QueenBot",
    opponent: "Rival",
    color: "white",
    initialFen: "startpos",
    moves: "",
    status: "started",
    whiteTime: 60_000,
    blackTime: 60_000,
    whiteIncrement: 0,
    blackIncrement: 0,
    clockUpdatedAt: Date.now(),
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

function panel(game: LiveGame) {
  return (
    <LiveGamePanel
      game={game}
      engineName="Queen"
      moveSoundsEnabled={false}
      onToggleMoveSounds={() => {}}
      boardTheme="forest"
      pieceSet="regal"
      onBoardThemeChange={() => {}}
      onPieceSetChange={() => {}}
      onExportPgn={() => {}}
    />
  );
}

function renderPanel(game: LiveGame) {
  return render(panel(game));
}

function clockFor(labelText: string) {
  return screen
    .getByText(labelText)
    .closest(".player-row")
    ?.querySelector("time");
}

afterEach(cleanup);

describe("LiveGamePanel clocks", () => {
  it("activates the clock of the side to move from a black-to-move FEN", () => {
    renderPanel(makeGame({ initialFen: blackToMoveFen }));
    expect(clockFor("Opponent · Black")).toHaveClass("active-clock");
    expect(clockFor("Your engine · White")).not.toHaveClass("active-clock");
  });

  it("switches the active clock after the reply move", () => {
    renderPanel(makeGame({ initialFen: blackToMoveFen, moves: "e7e5" }));
    expect(clockFor("Your engine · White")).toHaveClass("active-clock");
    expect(clockFor("Opponent · Black")).not.toHaveClass("active-clock");
  });
});

describe("LiveGamePanel evaluation continuity", () => {
  const telemetry = (
    overrides: Partial<NonNullable<LiveGame["engineInfo"]>>,
  ): LiveGame["engineInfo"] => ({
    principalVariation: [],
    raw: "info",
    ...overrides,
  });

  const searched = makeGame({
    engineInfo: telemetry({ depth: 20, scoreCp: 120 }),
  });

  function evalScore(container: HTMLElement) {
    return container.querySelector(".eval-score")?.textContent;
  }

  function evalHeight(container: HTMLElement) {
    return container.querySelector<HTMLElement>(".eval-fill")?.style.height;
  }

  it("keeps the last score while the next search has no telemetry yet", () => {
    const { container, rerender } = renderPanel(searched);
    const height = evalHeight(container);
    expect(evalScore(container)).toBe("+1.20");

    // The backend clears telemetry the moment our turn starts a new search.
    rerender(
      panel(makeGame({ ...searched, engineInfo: null, engineThinking: true })),
    );
    expect(evalScore(container)).toBe("+1.20");
    expect(evalHeight(container)).toBe(height);
    // The telemetry tile reads from the same retained evaluation.
    expect(
      screen.getByText("Evaluation").parentElement?.querySelector("strong")
        ?.textContent,
    ).toBe("+1.20");
  });

  it("keeps the last score when a scoreless info line arrives", () => {
    const { container, rerender } = renderPanel(searched);
    rerender(
      panel(
        makeGame({
          ...searched,
          engineInfo: telemetry({ depth: 3 }),
          engineThinking: true,
        }),
      ),
    );
    expect(evalScore(container)).toBe("+1.20");
  });

  it("adopts the next scored evaluation as soon as it arrives", () => {
    const { container, rerender } = renderPanel(searched);
    rerender(
      panel(
        makeGame({
          ...searched,
          engineInfo: telemetry({ depth: 24, mateIn: 3 }),
        }),
      ),
    );
    expect(evalScore(container)).toBe("M3");
  });

  it("shows the neutral evaluation before the engine has ever scored", () => {
    const { container } = renderPanel(makeGame({ engineThinking: true }));
    expect(evalScore(container)).toBe("0.00");
  });
});
