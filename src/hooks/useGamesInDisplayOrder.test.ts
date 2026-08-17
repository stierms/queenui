import { describe, expect, it } from "vitest";
import { renderHook } from "@testing-library/react";
import { useGamesInDisplayOrder } from "./useGamesInDisplayOrder";
import type { LiveGame } from "../types";

function game(id: string, status: string, clockUpdatedAt: number): LiveGame {
  return {
    id,
    accountId: "acct",
    botUsername: "QueenBot",
    opponent: `Opponent-${id}`,
    color: "white",
    initialFen: "startpos",
    moves: "e2e4",
    status,
    whiteTime: 60_000,
    blackTime: 60_000,
    whiteIncrement: 0,
    blackIncrement: 0,
    clockUpdatedAt,
    botRating: null,
    opponentRating: null,
    result: null,
    engineLine: null,
    engineInfo: null,
    error: null,
    engineThinking: false,
  };
}

function ids(games: LiveGame[]) {
  return games.map((item) => item.id);
}

describe("useGamesInDisplayOrder", () => {
  it("keeps live games in first-seen order and finished games afterwards", () => {
    const { result, rerender } = renderHook(
      ({ games }: { games: LiveGame[] }) => useGamesInDisplayOrder(games),
      {
        initialProps: {
          games: [game("a", "started", 100), game("b", "started", 200)],
        },
      },
    );
    expect(ids(result.current)).toEqual(["a", "b"]);

    rerender({ games: [game("b", "started", 400), game("a", "started", 300)] });
    expect(ids(result.current)).toEqual(["a", "b"]);

    rerender({ games: [game("b", "started", 500), game("a", "draw", 600)] });
    expect(ids(result.current)).toEqual(["b", "a"]);
  });

  it("evicts finished and absent games so a returning id joins the end", () => {
    const { result, rerender } = renderHook(
      ({ games }: { games: LiveGame[] }) => useGamesInDisplayOrder(games),
      {
        initialProps: {
          games: [game("a", "started", 100), game("b", "started", 200)],
        },
      },
    );
    expect(ids(result.current)).toEqual(["a", "b"]);

    // "a" finishes and later disappears from the snapshot entirely.
    rerender({ games: [game("a", "draw", 300), game("b", "started", 400)] });
    rerender({ games: [game("b", "started", 500)] });

    // When "a" reappears live it must take a fresh slot after "b".
    rerender({ games: [game("a", "started", 600), game("b", "started", 700)] });
    expect(ids(result.current)).toEqual(["b", "a"]);
  });

  it("returns a stable array while the games reference is unchanged", () => {
    const games = [game("a", "started", 100)];
    const { result, rerender } = renderHook(
      (props: { games: LiveGame[] }) => useGamesInDisplayOrder(props.games),
      { initialProps: { games } },
    );
    const first = result.current;
    rerender({ games });
    expect(result.current).toBe(first);
  });
});
