import { useEffect, useState } from "react";
import {
  gamesOverviewStorageKey,
  storedGamesOverview,
  type GamesOverview,
} from "../lib/gameView";

/**
 * Which of the two all-games overviews the games surface shows, remembered.
 *
 * Seeded from disk and written back on every change, exactly like the focus
 * view's collapsed widgets — and for the same reason: this is a statement about
 * how the operator reads the page, so re-deciding it for them on every visit is
 * a control that does not work.
 *
 * It is also where leaving a focused game lands. Keeping the memory here rather
 * than beside the focused-game state is what makes that true without an extra
 * "where did I come from" field: there is only ever one overview, so returning
 * to it cannot pick the wrong one.
 */
export function useGamesOverview() {
  const [overview, setOverview] = useState<GamesOverview>(storedGamesOverview);

  useEffect(() => {
    try {
      localStorage.setItem(gamesOverviewStorageKey, overview);
    } catch {
      // A browser refusing storage costs the memory, not the page.
    }
  }, [overview]);

  return { overview, chooseOverview: setOverview };
}
