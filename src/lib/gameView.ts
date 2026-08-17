import { countLiveGames } from "./chess";
import type { LiveGame } from "../types";

/**
 * The two ways the games surface shows *all* of the games.
 *
 * `grid` is every board as a tile, `detail` is the stacked panels — the same
 * set of games either way, and the same live/all filter over it. One is for
 * scanning a fleet at a glance, the other for reading the engine's telemetry
 * beside each board without picking a game first.
 *
 * The operator's choice between them is remembered (`storedGamesOverview`),
 * which is what makes "the overview" a single thing the rest of the surface
 * can return to.
 */
export type GamesOverview = "grid" | "detail";

/**
 * What the games surface is drawing: one of the two overviews, or a single
 * game drilled into.
 *
 * `focus` is deliberately not a third thing a control can offer. Focus means
 * *this* game, so it can only be entered by naming one — a mode switch into it
 * has to invent which board it means, and the board it invented (the first
 * live one) was a game the operator had not chosen. Every way in therefore
 * carries a game with it, and every way out is `gamesView` falling back to the
 * overview.
 */
export type GamesView = GamesOverview | "focus";

export const gamesOverviewStorageKey = "queenui-games-overview";

/**
 * Which overview the operator last chose, and so which one the surface opens
 * in and returns to.
 *
 * A preference, not state, and stored like the board theme and the collapsed
 * widgets are: someone who reads this page in Detail is saying how they want
 * to read it, not how they wanted to read it once. Grid is the default because
 * it is the presentation that survives a fleet — a dozen games in Detail is a
 * dozen screens of scrolling.
 *
 * Written as the bare word rather than as JSON: it is one of two values, and
 * an envelope around a single enum would be one more shape to validate for no
 * gain. Anything else on disk — an older build's value, a hand-edited record,
 * a browser that refuses storage — reads as the default rather than throwing
 * on the way to a page render.
 */
export function storedGamesOverview(): GamesOverview {
  try {
    return localStorage.getItem(gamesOverviewStorageKey) === "detail"
      ? "detail"
      : "grid";
  } catch {
    return "grid";
  }
}

/**
 * Whether the surface opens with a game already drilled into.
 *
 * Exactly one live game is a game to watch, so the page opens on its board and
 * saves the operator a click they had no alternative to. Any other count is a
 * *set* of games — a fleet to survey, or an archive to read — and a set is
 * what the overview is for; there is no non-arbitrary single game to choose.
 *
 * Called from a `useState` initializer and from nowhere else: never in an
 * effect, never on a snapshot change. A game starting or ending while the
 * operator is on this page must not move the view out from under them, and the
 * only way to guarantee that is for the live count to be read exactly once.
 */
export function entersFocused(games: readonly LiveGame[]): boolean {
  return countLiveGames(games) === 1;
}

/**
 * The view being drawn, from the remembered overview and whether a game is
 * drilled into.
 *
 * This is also the return rule, and the reason it is one line: leaving focus
 * does not *decide* anywhere to go, it only stops focusing, and what is
 * underneath is the overview the operator was already on. Escape, the back
 * button and the segmented control therefore cannot disagree with each other
 * about where "back" is, in this session or after a restart.
 */
export function gamesView(
  overview: GamesOverview,
  focused: boolean,
): GamesView {
  return focused ? "focus" : overview;
}

/** The two widgets beside the board in the focus view. */
export type GameWidget = "analysis" | "moves";

export type CollapsedWidgets = Record<GameWidget, boolean>;

export const collapsedWidgetsStorageKey = "queenui-game-widgets";

const EXPANDED: CollapsedWidgets = { analysis: false, moves: false };

/**
 * Which focus-view widgets are put away, for every game.
 *
 * Deliberately one record and not a map keyed by game id. An operator who
 * collapses the moves list is saying "I do not want the moves list", not "I do
 * not want the moves list *of game P7vQ9kLm*" — a per-game memory would spring
 * the widget back open on the next board and again on every new game, which
 * makes the control feel broken rather than remembered. It survives a restart
 * for the same reason the board theme does: it is a preference, not state.
 */
export function storedCollapsedWidgets(): CollapsedWidgets {
  try {
    const stored: unknown = JSON.parse(
      localStorage.getItem(collapsedWidgetsStorageKey) ?? "null",
    );
    if (!stored || typeof stored !== "object") return EXPANDED;
    const record = stored as Partial<Record<GameWidget, unknown>>;
    return {
      analysis: record.analysis === true,
      moves: record.moves === true,
    };
  } catch {
    // A malformed record says nothing about what the operator wanted; the
    // widgets are worth more open than closed, so start from open.
    return EXPANDED;
  }
}
