import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { GamesPage } from "./GamesPage";
import { countLiveGames } from "../lib/chess";
import { emptySnapshot, type AppSnapshot, type LiveGame } from "../types";

function game(overrides: Partial<LiveGame> = {}): LiveGame {
  return {
    id: "P7vQ9kLm",
    accountId: "queenbot",
    botUsername: "QueenBot",
    opponent: "TacticalRaven",
    botRating: 2400,
    opponentRating: 2380,
    color: "white",
    initialFen: "startpos",
    moves: "e2e4 e7e5",
    status: "started",
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
    ...overrides,
  };
}

const failed = game({
  id: "F5tD1jRk",
  opponent: "GambitFalcon",
  status: "error",
  error: "Engine process exited during search (exit code 0xC0000005).",
});

function renderPage(
  games: LiveGame[],
  overrides: Partial<Parameters<typeof GamesPage>[0]> = {},
) {
  const onDismissGameError = vi.fn();
  const snapshot: AppSnapshot = { ...emptySnapshot, games };
  render(
    <GamesPage
      snapshot={snapshot}
      busy={new Set<string>()}
      moveSoundsEnabled={false}
      onToggleMoveSounds={() => {}}
      boardTheme="forest"
      pieceSet="regal"
      onBoardThemeChange={() => {}}
      onPieceSetChange={() => {}}
      onExportPgn={() => {}}
      onDismissGameError={onDismissGameError}
      {...overrides}
    />,
  );
  return { onDismissGameError };
}

afterEach(cleanup);

describe("games that died", () => {
  it("shows the failed game rather than an empty page", () => {
    /*
     * The incident, in one assertion. A game whose task failed was excluded by
     * `is_live` — so no board drew it — and pruned away with the finished games
     * a snapshot later, leaving a page that said "No games yet" about a bot
     * that had been playing a minute earlier, with the cause recorded nowhere
     * the operator would look. The backend retains these until they are
     * dismissed; nothing in this page may drop them again.
     */
    renderPage([failed]);

    expect(screen.queryByText("No games yet")).toBeNull();
    const card = screen.getByRole("alert");
    expect(card).toHaveTextContent("QueenBot");
    expect(card).toHaveTextContent("GambitFalcon");
    // The backend's own text, verbatim: it is the only account of what
    // happened that reaches this screen.
    expect(card).toHaveTextContent(
      "Engine process exited during search (exit code 0xC0000005).",
    );
    expect(card).toHaveTextContent("lichess.org/F5tD1jRk");
  });

  it("is not a live game, and does not become one by being displayed", () => {
    /*
     * `countLiveGames` is the app's single definition of "how many games are
     * live" — the sidebar badge, the Overview strip, the close guard and the
     * Challenges capacity panel all quote it. A failed game showing up in it
     * would put a phantom board in the close guard and stop QueenUI from
     * quitting over a game that is already over.
     */
    const games = [game(), failed];
    expect(countLiveGames(games)).toBe(1);

    renderPage(games);

    const filters = screen.getByRole("group", { name: "Game filter" });
    expect(filters).toHaveTextContent("Live 1");
    // And not counted among the boards either: "All" describes the list below
    // it, which a failed game is deliberately not part of.
    expect(filters).toHaveTextContent("All 1");
    expect(document.querySelectorAll(".live-panel")).toHaveLength(1);
  });

  it("stays visible under both filters, including the one the page opens on", async () => {
    /*
     * The page opens on "Live". A failed game is by definition not live, so a
     * failure routed through the filter would be invisible on exactly the
     * screen an operator opens — the disappearance again, one layer up.
     */
    const user = userEvent.setup();
    renderPage([game(), failed]);

    expect(screen.getByRole("alert")).toHaveTextContent("GambitFalcon");
    await user.click(screen.getByRole("button", { name: /^All/ }));
    expect(screen.getByRole("alert")).toHaveTextContent("GambitFalcon");
  });

  it("dismisses one game by name, not the whole list", async () => {
    // Two failures are two separate facts, each read (or not) on its own. The
    // control names its game so both are reachable and neither is ambiguous.
    const user = userEvent.setup();
    const second = game({
      id: "K3xR8dTa",
      opponent: "SilentBishop",
      status: "error",
      error: "Lichess closed the game stream (HTTP 429).",
    });
    const { onDismissGameError } = renderPage([failed, second]);

    await user.click(
      screen.getByRole("button", { name: "Dismiss the failed game K3xR8dTa" }),
    );

    expect(onDismissGameError).toHaveBeenCalledExactlyOnceWith(second);
  });

  it("keeps the card and its verb while the dismissal is in flight", () => {
    /*
     * The card is removed by the next snapshot, never by the click. A dismissal
     * the backend refuses — an older runner answers `dismissGameError` with the
     * "update queen-runner" message — must leave the error text exactly where
     * it was, since the click would otherwise destroy the only copy of it.
     */
    renderPage([failed], { busy: new Set(["dismiss-game-F5tD1jRk"]) });

    expect(
      screen.getByRole("button", { name: "Dismiss the failed game F5tD1jRk" }),
    ).toBeDisabled();
    expect(screen.getByRole("alert")).toHaveTextContent("Dismissing…");
    expect(screen.getByRole("alert")).toHaveTextContent(
      "Engine process exited during search",
    );
  });

  it("says a game with no recorded reason has none, instead of showing a blank", () => {
    renderPage([game({ id: "Q1", status: "error", error: null })]);
    expect(screen.getByRole("alert")).toHaveTextContent(
      "QueenUI recorded no reason for this failure.",
    );
  });

  it("does not invent an opponent for a game that failed before one was known", () => {
    /*
     * The game context starts with an empty opponent and fills it in from
     * Lichess's `gameFull`. A task that died first has none to name, and
     * "QueenBot vs. " reads as a rendering bug rather than as a fact about the
     * game.
     */
    renderPage([game({ id: "Q2", status: "error", opponent: "" })]);
    expect(screen.getByRole("alert")).toHaveTextContent(
      "QueenBot — opponent unknown",
    );
  });
});

describe("the empty page", () => {
  it("still appears when there is genuinely nothing", () => {
    renderPage([]);
    expect(screen.getByText("No games yet")).toBeVisible();
  });

  it("does not offer an archive that would also be empty", () => {
    // With nothing but failures, "Show all games" leads to a blank list and
    // "Completed games are still available in the archive" is false.
    renderPage([failed]);
    expect(screen.queryByText("No live games")).toBeNull();
    expect(screen.queryByRole("button", { name: "Show all games" })).toBeNull();
  });

  it("keeps the live-filter empty panel when there are finished boards to see", () => {
    renderPage([game({ status: "mate", result: "1-0" }), failed]);
    expect(screen.getByText("No live games")).toBeVisible();
    expect(
      screen.getByRole("button", { name: "Show all games" }),
    ).toBeVisible();
  });
});
