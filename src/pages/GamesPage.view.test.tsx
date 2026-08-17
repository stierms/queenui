import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { GamesPage } from "./GamesPage";
import { gamesOverviewStorageKey } from "../lib/gameView";
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

/** `count` live games, each against a differently named opponent. */
function liveGames(count: number) {
  return Array.from({ length: count }, (_, index) =>
    game({ id: `game-${index}`, opponent: `Rival${index}` }),
  );
}

function page(games: LiveGame[], stale = false) {
  const snapshot: AppSnapshot = { ...emptySnapshot, games };
  return (
    <GamesPage
      snapshot={snapshot}
      busy={new Set<string>()}
      moveSoundsEnabled={false}
      onToggleMoveSounds={() => {}}
      boardTheme="forest"
      pieceSet="regal"
      stale={stale}
      onBoardThemeChange={() => {}}
      onPieceSetChange={() => {}}
      onExportPgn={() => {}}
      onDismissGameError={() => {}}
    />
  );
}

const tiles = () => document.querySelectorAll(".game-tile");
const panels = () => document.querySelectorAll(".live-panel");
const grid = () => document.querySelector(".games-grid");
const detail = () => document.querySelector(".games-detail");
/** The overview control — absent while a game is drilled into. */
const viewControl = () => screen.queryByRole("group", { name: "Board view" });
/** The way out of a focused game, beyond Escape. */
const backButton = () =>
  screen.queryByRole("button", { name: "Back to all games" });
const overviewButton = (label: "Grid" | "Detail") =>
  screen.getByRole("button", { name: label });
const pressed = (label: "Grid" | "Detail") =>
  overviewButton(label).getAttribute("aria-pressed");
/**
 * The affordance that drills into one game: a tile in Grid, a board in Detail.
 * Deliberately one helper, because it is deliberately one accessible name —
 * choosing a game is the same act in both overviews.
 */
const chooseGame = (opponent: string) =>
  screen.getByRole("button", { name: `Focus on QueenBot versus ${opponent}` });

beforeEach(() => localStorage.clear());
afterEach(cleanup);

describe("the overview control", () => {
  it("offers the two overviews and no way to focus without a game", () => {
    /*
     * The regression this round is about. A third "Focus" segment had no game
     * to focus, so it invented one — the first live board — and an operator who
     * pressed it was looking at a game they had not chosen. Focus is reachable
     * only by naming a game now, so the control cannot offer it at all.
     */
    render(page(liveGames(2)));

    const control = viewControl();
    expect(control).not.toBeNull();
    const segments = within(control!);
    expect(
      segments.getAllByRole("button").map((button) => button.textContent),
    ).toEqual(["Grid", "Detail"]);
    expect(segments.queryByRole("button", { name: "Focus" })).toBeNull();
    expect(backButton()).toBeNull();
    expect(panels()).toHaveLength(0);
  });

  it("draws every game as a tile in Grid", () => {
    render(page(liveGames(4)));

    expect(tiles()).toHaveLength(4);
    expect(pressed("Grid")).toBe("true");
    expect(pressed("Detail")).toBe("false");
  });

  it("draws every game at full depth in Detail", async () => {
    const user = userEvent.setup();
    render(page(liveGames(4)));

    await user.click(overviewButton("Detail"));

    expect(detail()).not.toBeNull();
    expect(panels()).toHaveLength(4);
    expect(tiles()).toHaveLength(0);
    // Full detail, not tiles in a column: every row carries the engine
    // telemetry the grid leaves to the focus view.
    expect(screen.getAllByText("Engine analysis")).toHaveLength(4);
    expect(pressed("Detail")).toBe("true");
    expect(pressed("Grid")).toBe("false");
  });

  it("keeps the live/all filter in both overviews", async () => {
    const user = userEvent.setup();
    render(
      page([
        ...liveGames(2),
        game({
          id: "done",
          opponent: "Retired",
          status: "mate",
          result: "1-0",
        }),
      ]),
    );

    expect(tiles()).toHaveLength(2);
    await user.click(screen.getByRole("button", { name: /^All/ }));
    expect(tiles()).toHaveLength(3);

    await user.click(overviewButton("Detail"));
    expect(panels()).toHaveLength(3);
    await user.click(screen.getByRole("button", { name: /^Live/ }));
    expect(panels()).toHaveLength(2);
  });
});

describe("which view the games surface opens in", () => {
  it("focuses the one game there is to watch", () => {
    render(page(liveGames(1)));

    expect(panels()).toHaveLength(1);
    expect(grid()).toBeNull();
    expect(detail()).toBeNull();
    /*
     * And it is visibly a drilled-in state: the overview control is replaced by
     * the way out of it, rather than left standing with neither button pressed
     * — which is the "so which one am I in?" screen this round removes.
     */
    expect(viewControl()).toBeNull();
    expect(backButton()).toBeVisible();
  });

  it("opens several live games in the remembered overview, one tile each", () => {
    render(page(liveGames(4)));

    expect(tiles()).toHaveLength(4);
    expect(panels()).toHaveLength(0);
    expect(backButton()).toBeNull();
  });

  it("opens the archive in the remembered overview when nothing is live", async () => {
    /*
     * Nothing live is nothing to focus, so the page opens in an overview — the
     * one the operator last chose, showing what the filter admits. The archive
     * is still a click away rather than switched to for them: "Live" is what
     * they asked for, and a page that quietly answered a different question
     * would be lying about the boards it is showing.
     */
    const user = userEvent.setup();
    localStorage.setItem(gamesOverviewStorageKey, "detail");
    render(
      page([
        game({ id: "done-1", status: "mate", result: "1-0" }),
        game({ id: "done-2", status: "resign", result: "0-1" }),
      ]),
    );

    expect(pressed("Detail")).toBe("true");
    expect(backButton()).toBeNull();
    await user.click(screen.getByRole("button", { name: "Show all games" }));

    expect(detail()).not.toBeNull();
    expect(panels()).toHaveLength(2);
    expect(grid()).toBeNull();
  });

  it("keeps a failed game out of the grid and in its card", () => {
    render(
      page([
        ...liveGames(2),
        game({
          id: "F5tD1jRk",
          opponent: "GambitFalcon",
          status: "error",
          error: "Engine process exited during search.",
        }),
      ]),
    );

    expect(tiles()).toHaveLength(2);
    expect(screen.getByRole("alert")).toHaveTextContent("GambitFalcon");
  });
});

describe("the overview the page remembers", () => {
  it("opens in the overview last chosen, not in the default one", () => {
    localStorage.setItem(gamesOverviewStorageKey, "detail");
    render(page(liveGames(4)));

    expect(detail()).not.toBeNull();
    expect(panels()).toHaveLength(4);
    expect(grid()).toBeNull();
  });

  it("survives leaving the page and coming back", async () => {
    /*
     * Which overview this is has to outlive the visit, for the same reason the
     * collapsed widgets do: an operator who works in Detail is saying how they
     * read this page, and re-deciding it for them on the next visit is a
     * control that does not work. It is also what "back" means, so a forgotten
     * choice would send Escape somewhere they never were.
     */
    const user = userEvent.setup();
    const { unmount } = render(page(liveGames(4)));
    await user.click(overviewButton("Detail"));
    expect(panels()).toHaveLength(4);
    unmount();

    render(page(liveGames(4)));

    expect(detail()).not.toBeNull();
    expect(panels()).toHaveLength(4);
    expect(grid()).toBeNull();
  });

  it("falls back to the grid with nothing stored, or nonsense stored", () => {
    localStorage.setItem(gamesOverviewStorageKey, "sideways");
    const junk = render(page(liveGames(4)));
    expect(tiles()).toHaveLength(4);
    junk.unmount();

    localStorage.clear();
    render(page(liveGames(4)));

    expect(tiles()).toHaveLength(4);
    expect(detail()).toBeNull();
  });
});

describe("the view never changes on its own", () => {
  it("stays in the grid when every game but one ends", () => {
    /*
     * The guard this asserts is that the view is a `useState` initializer and
     * nothing else. Recomputing it from the current live count — in an effect,
     * or inline — would drop an operator scanning four boards into a focused
     * one the moment three of them finished.
     */
    const { rerender } = render(page(liveGames(4)));
    expect(tiles()).toHaveLength(4);

    rerender(page(liveGames(1)));

    expect(grid()).not.toBeNull();
    expect(panels()).toHaveLength(0);
  });

  it("stays focused when more games start", () => {
    const { rerender } = render(page(liveGames(1)));
    expect(panels()).toHaveLength(1);

    rerender(page(liveGames(4)));

    expect(panels()).toHaveLength(1);
    expect(grid()).toBeNull();
  });

  it("keeps the focused board when its game finishes under it", () => {
    const [live] = liveGames(1);
    const { rerender } = render(page([live]));

    rerender(page([{ ...live, status: "mate", result: "1-0" }]));

    expect(panels()).toHaveLength(1);
    expect(screen.getByText(/Finished/)).toBeVisible();
  });

  it("stays in Detail when a board arrives under it", async () => {
    const user = userEvent.setup();
    const { rerender } = render(page(liveGames(2)));
    await user.click(overviewButton("Detail"));
    expect(panels()).toHaveLength(2);

    rerender(page(liveGames(4)));

    expect(detail()).not.toBeNull();
    expect(panels()).toHaveLength(4);
    expect(tiles()).toHaveLength(0);
  });
});

describe("choosing a game, and getting back out of it", () => {
  it("focuses the game whose tile was clicked", async () => {
    const user = userEvent.setup();
    render(page(liveGames(3)));

    await user.click(chooseGame("Rival2"));

    expect(panels()).toHaveLength(1);
    expect(grid()).toBeNull();
    expect(
      screen.getByRole("heading", { level: 2, name: /Rival2/ }),
    ).toBeVisible();
  });

  it("focuses the game whose board was chosen in Detail", async () => {
    // Detail rows are boards too, and a board is how a game is chosen. Without
    // this the whole overview would be a dead end: everything on screen except
    // the one segmented control, and no way in from the game itself.
    const user = userEvent.setup();
    render(page(liveGames(3)));
    await user.click(overviewButton("Detail"));

    await user.click(chooseGame("Rival2"));

    expect(detail()).toBeNull();
    expect(panels()).toHaveLength(1);
    expect(
      screen.getByRole("heading", { level: 2, name: /Rival2/ }),
    ).toBeVisible();
  });

  it("returns to the grid it was entered from, by key and by button", async () => {
    const user = userEvent.setup();
    render(page(liveGames(3)));

    await user.click(chooseGame("Rival1"));
    await user.keyboard("{Escape}");
    expect(tiles()).toHaveLength(3);

    await user.click(chooseGame("Rival0"));
    await user.click(backButton()!);

    expect(tiles()).toHaveLength(3);
    expect(pressed("Grid")).toBe("true");
  });

  it("returns to Detail when Detail is where the game was chosen", async () => {
    /*
     * Both ways out land on the overview the operator was reading, because
     * neither of them decides anything: leaving focus stops focusing, and what
     * is underneath is the remembered overview. A back button that always went
     * to the grid would throw away a Detail operator's place on every game they
     * looked at.
     */
    const user = userEvent.setup();
    render(page(liveGames(3)));
    await user.click(overviewButton("Detail"));

    await user.click(chooseGame("Rival1"));
    await user.click(backButton()!);
    expect(detail()).not.toBeNull();
    expect(panels()).toHaveLength(3);
    expect(tiles()).toHaveLength(0);

    await user.click(chooseGame("Rival1"));
    await user.keyboard("{Escape}");
    expect(detail()).not.toBeNull();
  });

  it("leaves a single game's focus rather than trapping the operator in it", async () => {
    /*
     * This used to do nothing, on the grounds that a grid of one tile is the
     * same board smaller. But the overview carries what focus cannot — the
     * filter counts, the failed cards, whatever else is in the archive — and an
     * exit that is sometimes absent is a trap; the back button beside it would
     * have to vanish too, which is how a control stops being believed.
     */
    const user = userEvent.setup();
    render(page(liveGames(1)));

    await user.keyboard("{Escape}");

    expect(tiles()).toHaveLength(1);
    expect(panels()).toHaveLength(0);
    expect(viewControl()).not.toBeNull();
    expect(backButton()).toBeNull();
  });
});

describe("what a tile carries", () => {
  it("draws the eval bar from the engine's last score, our perspective", () => {
    render(
      page([
        game({
          id: "white-side",
          opponent: "WhiteSide",
          engineInfo: {
            depth: 20,
            scoreCp: 120,
            principalVariation: [],
            raw: "info",
          },
        }),
        game({
          id: "black-side",
          opponent: "BlackSide",
          color: "black",
          engineInfo: {
            depth: 20,
            scoreCp: 120,
            principalVariation: [],
            raw: "info",
          },
        }),
      ]),
    );

    const bars = screen.getAllByRole("img", {
      name: "Evaluation for our engine +1.20",
    });
    expect(bars).toHaveLength(2);
    const fill = (bar: HTMLElement) =>
      Number.parseFloat(bar.querySelector<HTMLElement>("i")!.style.width);
    // The fill is always WHITE's share, so the same +1.20 reads as most of the
    // bar for our white board and as little of it for our black one.
    expect(fill(bars[0])).toBeGreaterThan(50);
    expect(fill(bars[1])).toBeLessThan(50);
  });

  it("colours the clock chip of a side under time pressure", () => {
    render(
      page([
        game({ id: "pressed", opponent: "Pressed", whiteTime: 8_000 }),
        game({ id: "calm", opponent: "Calm" }),
      ]),
    );

    const [pressed, calm] = Array.from(tiles());
    // White is to move after 1...e5 and we are white on both boards, so the
    // clock under pressure is the one in our own — the near — nameplate.
    const ourClock = (tile: Element) =>
      tile.querySelector(".tile-plate-bottom time")!;
    expect(ourClock(pressed)).toHaveClass("clock-low");
    expect(ourClock(pressed)).toHaveClass("clock-critical");
    expect(ourClock(calm)).not.toHaveClass("clock-low");
    expect(ourClock(calm)).toHaveClass("active-clock");
  });

  /**
   * Two boards: one where our engine has white, one where it has black. Every
   * nameplate claim is asserted on both, because a tile that happens to be
   * right for one colour and mirrored for the other is the bug this pair of
   * plates exists to fix.
   */
  function bothColours(extra: Partial<LiveGame> = {}) {
    return page([
      game({
        id: "we-are-white",
        opponent: "SableRook",
        opponentRating: 2380,
        color: "white",
        ...extra,
      }),
      game({
        id: "we-are-black",
        opponent: "IvoryKnight",
        opponentRating: 2205,
        color: "black",
        ...extra,
      }),
    ]);
  }

  /** What one edge's nameplate says. */
  function plate(tile: Element, edge: "top" | "bottom") {
    const row = tile.querySelector(`.tile-plate-${edge}`)!;
    return {
      name: row.querySelector("strong")!.textContent,
      rating: row.querySelector("small")?.textContent ?? null,
      colour: row.querySelector(".tile-color")!.className,
      clock: row.querySelector("time")!.textContent,
      ours: row.classList.contains("tile-plate-ours"),
    };
  }

  it("names both players, each on the board edge they play", () => {
    render(bothColours());

    const [asWhite, asBlack] = Array.from(tiles());
    /*
     * Own-perspective boards, so our engine is the near player on both and its
     * plate is the bottom one whichever colour it happens to have. Before this
     * the tile named the opponent alone, and our own account and rating — the
     * thing an operator is watching a fleet of — appeared nowhere.
     */
    expect(plate(asWhite, "bottom")).toMatchObject({
      name: "QueenBot",
      rating: "2400",
      ours: true,
    });
    expect(plate(asWhite, "top")).toMatchObject({
      name: "SableRook",
      rating: "2380",
      ours: false,
    });
    expect(plate(asBlack, "bottom")).toMatchObject({
      name: "QueenBot",
      rating: "2400",
      ours: true,
    });
    expect(plate(asBlack, "top")).toMatchObject({
      name: "IvoryKnight",
      rating: "2205",
      ours: false,
    });
    /*
     * And the plate named for an edge is really on it. The class is what every
     * other assertion here selects by, so without this a plate could be marked
     * for the far edge and rendered under the board — a nameplate on the
     * opposite side from the pieces it names, which is the whole failure.
     */
    const edgeOrder = (tile: Element) =>
      Array.from(tile.children)
        .map((child) =>
          child.classList.contains("tile-plate-top")
            ? "top"
            : child.classList.contains("tile-plate-bottom")
              ? "bottom"
              : child.classList.contains("board-surface")
                ? "board"
                : null,
        )
        .filter(Boolean);
    expect(edgeOrder(asWhite)).toEqual(["top", "board", "bottom"]);
    expect(edgeOrder(asBlack)).toEqual(["top", "board", "bottom"]);
  });

  it("marks which colour each of the two is playing", () => {
    render(bothColours());

    const [asWhite, asBlack] = Array.from(tiles());
    expect(plate(asWhite, "bottom").colour).toContain("tile-color-white");
    expect(plate(asWhite, "top").colour).toContain("tile-color-black");
    // Mirrored on the board we have black, both plates at once: a tile that
    // swapped only one of them would still be wrong about the game.
    expect(plate(asBlack, "bottom").colour).toContain("tile-color-black");
    expect(plate(asBlack, "top").colour).toContain("tile-color-white");
    /*
     * A dot says nothing to a screen reader, so each plate restates its
     * player's part and colour in text. This is the only announced form of the
     * thing the dots show.
     */
    expect(screen.getByText("Your engine · White")).toBeInTheDocument();
    expect(screen.getByText("Opponent · Black")).toBeInTheDocument();
    expect(screen.getByText("Your engine · Black")).toBeInTheDocument();
    expect(screen.getByText("Opponent · White")).toBeInTheDocument();
  });

  it("pairs each clock with the player whose clock it is", () => {
    /*
     * Distinct clocks and `clockUpdatedAt: 0`, so nothing interpolates and the
     * digits on screen are exactly the two numbers the snapshot carries. Which
     * plate each one lands in is the whole assertion.
     */
    render(
      bothColours({ whiteTime: 61_000, blackTime: 125_000, clockUpdatedAt: 0 }),
    );

    const [asWhite, asBlack] = Array.from(tiles());
    expect(plate(asWhite, "bottom").clock).toBe("01:01");
    expect(plate(asWhite, "top").clock).toBe("02:05");
    expect(plate(asBlack, "bottom").clock).toBe("02:05");
    expect(plate(asBlack, "top").clock).toBe("01:01");
    // And white is to move after 1...e5, so the running chip is ours on the
    // board we have white and the opponent's on the board we have black.
    const clock = (tile: Element, edge: "top" | "bottom") =>
      tile.querySelector(`.tile-plate-${edge} time`)!;
    expect(clock(asWhite, "bottom")).toHaveClass("active-clock");
    expect(clock(asWhite, "top")).not.toHaveClass("active-clock");
    expect(clock(asBlack, "top")).toHaveClass("active-clock");
    expect(clock(asBlack, "bottom")).not.toHaveClass("active-clock");
  });

  it("says the opponent is unknown rather than leaving their plate blank", () => {
    // The game context fills the opponent in from Lichess's `gameFull`; a board
    // that arrived before it has nobody to name and must not pretend otherwise.
    render(
      page([game({ opponent: "  ", opponentRating: null }), ...liveGames(1)]),
    );

    expect(plate(Array.from(tiles())[0], "top")).toMatchObject({
      name: "Opponent unknown",
      rating: null,
    });
  });

  it("says so when a running game has reported an engine problem", () => {
    /*
     * `game.error` on a game that is still running is printed in full by the
     * panel. A tile cannot show the text, but a board that looks untroubled
     * while the engine behind it is not is how the one game that needs an
     * operator hides in a grid of twelve.
     */
    render(
      page([
        game({ id: "sick", error: "Engine stalled for 40 s." }),
        ...liveGames(1),
      ]),
    );

    expect(screen.getByText("Engine problem")).toBeVisible();
    expect(screen.getByTitle("Engine stalled for 40 s.")).toBeVisible();
  });

  it("keeps a frozen snapshot's tiles from claiming a live game", () => {
    render(page(liveGames(2), true));

    expect(screen.getAllByText("Not live")).toHaveLength(2);
    expect(document.querySelectorAll(".board-frozen")).toHaveLength(2);
    // Not one tile claims a live game; the only "Live" left on the page is the
    // filter, which counts what the snapshot says rather than what it can see.
    expect(document.querySelectorAll(".game-tile .live-eyebrow")).toHaveLength(
      0,
    );
  });
});

describe("the focus view's collapsed widgets", () => {
  async function collapseBoth(user: ReturnType<typeof userEvent.setup>) {
    await user.click(
      screen.getByRole("button", { name: "Collapse engine analysis" }),
    );
    await user.click(screen.getByRole("button", { name: "Collapse moves" }));
  }

  it("puts a widget away and keeps its heading", async () => {
    const user = userEvent.setup();
    render(page(liveGames(1)));

    expect(screen.getByText("Principal variation")).toBeVisible();
    await collapseBoth(user);

    expect(screen.queryByText("Principal variation")).toBeNull();
    expect(screen.queryByText("Waiting for the first move")).toBeNull();
    // The engine's name and the move count still report.
    expect(screen.getByText("Engine analysis")).toBeVisible();
    expect(screen.getByText("2 plies")).toBeVisible();
  });

  it("remembers the choice for every game, not for the game it was made on", async () => {
    /*
     * The whole point of a global memory. Collapsing the analysis on one board
     * and finding it open again on the next is a control that does not work,
     * and a per-game record is how that happens.
     */
    const user = userEvent.setup();
    render(page(liveGames(3)));

    await user.click(chooseGame("Rival0"));
    await collapseBoth(user);
    await user.keyboard("{Escape}");
    await user.click(chooseGame("Rival2"));

    expect(
      screen.getByRole("heading", { level: 2, name: /Rival2/ }),
    ).toBeVisible();
    expect(screen.queryByText("Principal variation")).toBeNull();
    expect(
      screen.getByRole("button", { name: "Expand engine analysis" }),
    ).toBeVisible();
  });

  it("survives leaving the page and coming back", async () => {
    const user = userEvent.setup();
    const { unmount } = render(page(liveGames(1)));
    await collapseBoth(user);
    unmount();

    render(page(liveGames(1)));

    expect(screen.queryByText("Principal variation")).toBeNull();
    expect(
      screen.getByRole("button", { name: "Expand moves" }),
    ).toHaveAttribute("aria-expanded", "false");
  });

  it("opens a widget again from the same control", async () => {
    const user = userEvent.setup();
    render(page(liveGames(1)));
    await collapseBoth(user);

    await user.click(
      screen.getByRole("button", { name: "Expand engine analysis" }),
    );

    expect(screen.getByText("Principal variation")).toBeVisible();
    expect(screen.queryByText("Waiting for the first move")).toBeNull();
  });
});
