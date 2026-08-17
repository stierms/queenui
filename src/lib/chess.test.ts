import { describe, expect, it } from "vitest";
import {
  countLiveGames,
  liveGamesOnly,
  moveRows,
  pgnForGame,
  positionFor,
  sideToMove,
} from "./chess";
import type { LiveGame } from "../types";

const blackToMoveFen =
  "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1";

function makeGame(overrides: Partial<LiveGame> = {}): LiveGame {
  return {
    id: "test-game",
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
    clockUpdatedAt: 1_700_000_000_000,
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

describe("counting live games", () => {
  const games = [
    makeGame({ id: "a", accountId: "one", status: "started" }),
    makeGame({ id: "b", accountId: "one", status: "created" }),
    makeGame({ id: "c", accountId: "one", status: "mate" }),
    makeGame({ id: "d", accountId: "two", status: "started" }),
  ];

  it("counts both live statuses and no finished game", () => {
    // `created` is the window between acceptance and the first move; the app
    // and `runtime.rs`'s `is_live` both count it.
    expect(countLiveGames(games)).toBe(3);
    expect(liveGamesOnly(games).map((game) => game.id)).toEqual([
      "a",
      "b",
      "d",
    ]);
  });

  it("narrows to one account when asked", () => {
    expect(countLiveGames(games, "one")).toBe(2);
    expect(countLiveGames(games, "two")).toBe(1);
    expect(countLiveGames(games, "three")).toBe(0);
  });
});

describe("pgnForGame", () => {
  it("writes SetUp and FEN headers for custom starting positions", () => {
    const pgn = pgnForGame(
      makeGame({ initialFen: blackToMoveFen, moves: "e7e5" }),
    );
    expect(pgn).toContain('[SetUp "1"]');
    expect(pgn).toContain(`[FEN "${blackToMoveFen}"]`);
    expect(pgn).toContain("e5");
  });

  it("renders promotion moves in SAN", () => {
    const pgn = pgnForGame(
      makeGame({ initialFen: "8/P6k/8/8/8/8/7K/8 w - - 0 60", moves: "a7a8q" }),
    );
    expect(pgn).toContain("a8=Q");
  });

  it("does not throw on malformed FEN and omits the FEN headers", () => {
    const game = makeGame({ initialFen: "not a real fen", moves: "e2e4" });
    expect(() => pgnForGame(game)).not.toThrow();
    const pgn = pgnForGame(game);
    expect(pgn).not.toContain('[SetUp "1"]');
    expect(pgn).toContain("e4");
  });
});

describe("positions with black to move first", () => {
  it("derives the side to move from the position, not ply parity", () => {
    expect(sideToMove(makeGame({ initialFen: blackToMoveFen }))).toBe("b");
    expect(
      sideToMove(makeGame({ initialFen: blackToMoveFen, moves: "e7e5" })),
    ).toBe("w");
    expect(sideToMove(makeGame({ moves: "e2e4" }))).toBe("b");
  });

  it("seeds move-row numbering and columns from the initial FEN", () => {
    const fen = "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 3";
    const rows = moveRows(makeGame({ initialFen: fen, moves: "e7e5 g1f3" }));
    expect(rows).toEqual([
      { number: 3, white: undefined, black: "e5" },
      { number: 4, white: "Nf3", black: undefined },
    ]);
  });

  it("returns every full-move row from move one", () => {
    const rows = moveRows(
      makeGame({
        moves:
          "e2e4 e7e5 g1f3 b8c6 f1b5 a7a6 b5a4 g8f6 e1g1 f8e7 f1e1 b7b5 a4b3",
      }),
    );
    expect(rows).toHaveLength(7);
    expect(rows[0]).toEqual({ number: 1, white: "e4", black: "e5" });
    expect(rows[6]).toEqual({ number: 7, white: "Bb3", black: undefined });
  });
});

describe("positionFor", () => {
  it("caches one derived position per game object", () => {
    const game = makeGame({ moves: "e2e4" });
    expect(positionFor(game)).toBe(positionFor(game));
    expect(positionFor(game).fen()).toContain(" b ");
  });
});
