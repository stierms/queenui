import { useMemo, useState } from "react";
import { isLiveGame } from "../lib/chess";
import type { LiveGame } from "../types";

/**
 * Live games keep their first-seen order (so boards do not jump around as
 * clocks update); finished games follow, most recently updated first.
 *
 * The internal order map evicts games that finished or disappeared from
 * the snapshot, and the returned array is memoized on `games`.
 */
export function useGamesInDisplayOrder(games: LiveGame[]) {
  const [order] = useState(() => new Map<string, number>());

  return useMemo(() => {
    const liveIds = new Set(
      games.filter((game) => isLiveGame(game)).map((game) => game.id),
    );
    for (const id of [...order.keys()]) {
      if (!liveIds.has(id)) order.delete(id);
    }
    let nextPosition = Math.max(-1, ...order.values()) + 1;
    for (const game of games) {
      if (isLiveGame(game) && !order.has(game.id)) {
        order.set(game.id, nextPosition);
        nextPosition += 1;
      }
    }
    return [...games].sort((left, right) => {
      const leftLive = isLiveGame(left);
      const rightLive = isLiveGame(right);
      if (leftLive !== rightLive) return leftLive ? -1 : 1;
      if (leftLive && rightLive)
        return order.get(left.id)! - order.get(right.id)!;
      return right.clockUpdatedAt - left.clockUpdatedAt;
    });
  }, [games, order]);
}
