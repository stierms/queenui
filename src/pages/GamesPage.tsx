import {
  useCallback,
  useEffect,
  useState,
  type CSSProperties,
  type ReactNode,
} from "react";
import { ArrowLeft, Gamepad2, LayoutGrid, LayoutList } from "lucide-react";
import type { PieceSetId } from "../ChessPiece";
import { EmptyPage } from "../components/EmptyPage";
import { FailedGameCard } from "../components/FailedGameCard";
import { GameTile } from "../components/GameTile";
import { LiveGamePanel } from "../components/LiveGamePanel";
import { hasPreviewParam } from "../dev/preview";
import type { BusyKeys } from "../hooks/useActionRunner";
import { useCollapsedWidgets } from "../hooks/useCollapsedWidgets";
import { useGamesOverview } from "../hooks/useGamesOverview";
import type { BoardThemeId } from "../lib/appearance";
import {
  boardGamesOnly,
  countLiveGames,
  failedGamesOnly,
  isLiveGame,
  liveGamesOnly,
} from "../lib/chess";
import { countText } from "../lib/format";
import { entersFocused, gamesView, type GamesOverview } from "../lib/gameView";
import { engineNameForGame, type AppSnapshot, type LiveGame } from "../types";

/**
 * The widest the tile row ever gets. Four boards side by side is what a
 * 2560px desktop fits at a size where the pieces are still readable; a fifth
 * would buy a column and cost the position on every board in it.
 */
const MAX_TILE_COLUMNS = 4;

/**
 * Escape belongs to whatever is layered over the page — a dialog, a dropdown,
 * the board-appearance popover — for as long as one is open. Radix closes
 * those on its own document listener without marking the event handled, so a
 * page-level Escape would otherwise dismiss the popover *and* leave the focus
 * view in the same press.
 */
function overlayIsOpen() {
  return (
    document.querySelector(
      '[role="dialog"], [role="menu"], [data-radix-popper-content-wrapper]',
    ) != null
  );
}

/**
 * One segment of the overview control.
 *
 * A component for two buttons because the two facts it states — the pressed
 * styling and the announced `aria-pressed` — are the same fact, and a segmented
 * control where they drift apart is one that lies to a screen reader about
 * which view is on screen.
 */
function OverviewButton({
  overview,
  current,
  onChoose,
  icon,
  label,
}: {
  overview: GamesOverview;
  current: GamesOverview;
  onChoose: (overview: GamesOverview) => void;
  icon: ReactNode;
  label: string;
}) {
  const pressed = current === overview;
  return (
    <button
      className={pressed ? "selected" : ""}
      aria-pressed={pressed}
      onClick={() => onChoose(overview)}
    >
      {icon}
      {label}
    </button>
  );
}

export function GamesPage({
  snapshot,
  busy,
  moveSoundsEnabled,
  onToggleMoveSounds,
  boardTheme,
  pieceSet,
  stale = false,
  onBoardThemeChange,
  onPieceSetChange,
  onExportPgn,
  onDismissGameError,
}: {
  snapshot: AppSnapshot;
  busy: BusyKeys;
  moveSoundsEnabled: boolean;
  onToggleMoveSounds: () => void;
  boardTheme: BoardThemeId;
  pieceSet: PieceSetId;
  /** These boards are rendered from a snapshot that may be out of date. */
  stale?: boolean;
  onBoardThemeChange: (theme: BoardThemeId) => void;
  onPieceSetChange: (set: PieceSetId) => void;
  onExportPgn: (game: LiveGame) => void;
  onDismissGameError: (game: LiveGame) => void;
}) {
  const [filter, setFilter] = useState<"live" | "all">(
    hasPreviewParam("games-preview") ? "all" : "live",
  );
  /*
   * Which overview this surface means, remembered across visits and restarts.
   * The segmented control writes it, and it is where leaving a focused game
   * lands — so "back" is one place, not a guess about where the operator was.
   */
  const { overview, chooseOverview } = useGamesOverview();
  /*
   * Whether a game is drilled into, decided from the snapshot as it stood when
   * this page mounted and never again. `useState` with an initializer is the
   * whole mechanism: no effect watches the game count, because a board arriving
   * or a game ending must not rearrange the screen an operator is reading.
   * Everything after mount is their choice — a game, the control, Escape or the
   * back button.
   */
  const [focused, setFocused] = useState(() => entersFocused(snapshot.games));
  const [focusedId, setFocusedId] = useState<string | null>(
    () => liveGamesOnly(snapshot.games)[0]?.id ?? null,
  );
  const view = gamesView(overview, focused);
  const { collapsed, toggleWidget } = useCollapsedWidgets();
  /*
   * Two lists, because a failed game is not a board. It has no result, its
   * position stopped where the task died, and the only thing worth reading
   * about it is the error text — so it is kept out of the tiles, out of the
   * panel list and out of both filter counts, which describe what the list
   * below them contains.
   */
  const failedGames = failedGamesOnly(snapshot.games);
  const boardGames = boardGamesOnly(snapshot.games);
  /*
   * The focused game outlives the filter on purpose. Its board is what the
   * operator asked to look at, and a game that finishes under them — or a
   * switch to "Live" while a finished board is up — should not blank the
   * screen; the panel already says "Finished · 1-0" over the final position.
   * Only a game leaving the snapshot entirely moves the focus, to the first
   * board in display order, which is the newest live one.
   */
  const focusGame =
    boardGames.find((game) => game.id === focusedId) ?? boardGames[0];
  /*
   * The only way into the focus view, from either overview: a game, named. The
   * grid's affordance is its tile and Detail's is its board, and both land
   * here, so there is no path that focuses a board the operator did not pick.
   */
  const focusOnGame = useCallback((game: LiveGame) => {
    setFocusedId(game.id);
    setFocused(true);
  }, []);
  const leaveFocus = useCallback(() => setFocused(false), []);

  /*
   * Escape leaves the focus view, always. It used to do nothing when the
   * snapshot held a single board, on the grounds that a grid of one tile is the
   * same board smaller — but a drilled-in state whose exit is sometimes absent
   * is a trap, and the back button beside it would have to vanish too. The
   * overview also carries what focus cannot: the filter counts, the failed
   * cards, and every other board in the archive.
   */
  useEffect(() => {
    if (!focused) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      if (overlayIsOpen()) return;
      setFocused(false);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [focused]);

  /*
   * "No games yet" only when there is genuinely nothing — including nothing
   * that failed. A snapshot holding only failures used to be indistinguishable
   * from a fresh install here, which is the disappearance this round is about,
   * one screen further out.
   */
  if (!boardGames.length && !failedGames.length)
    return (
      <EmptyPage
        icon={<Gamepad2 />}
        title="No games yet"
        copy="Games appear automatically when a connected Lichess bot starts playing."
      />
    );
  const liveCount = countLiveGames(snapshot.games);
  const visibleGames = boardGames.filter(
    (game) => filter === "all" || isLiveGame(game),
  );
  const tileColumns = Math.min(visibleGames.length, MAX_TILE_COLUMNS);
  const focusedOpponent = focusGame?.opponent.trim();
  const inFocus = view === "focus" && focusGame != null;
  const emptyList = boardGames.length ? (
    <section className="panel filtered-games-empty">
      <span>
        <Gamepad2 size={21} />
      </span>
      <div>
        <strong>No live games</strong>
        <small>Completed games are still available in the archive.</small>
      </div>
      <button onClick={() => setFilter("all")}>Show all games</button>
    </section>
  ) : /*
   * Nothing but failures in the snapshot. The panel above offers "Show all
   * games" over an archive that would also be empty, and its second line
   * would be false — so the failed cards are left to carry the screen.
   */
  null;
  return (
    <div className="module-content games-page">
      <section className="games-toolbar">
        <div>
          <h2>{filter === "live" ? "Live games" : "All games"}</h2>
          <p>
            {stale
              ? "Frozen at the runner's last update — not currently connected."
              : inFocus
                ? `Focused on the game against ${focusedOpponent || "an unknown opponent"} — press Esc for all games.`
                : filter === "live"
                  ? "Boards currently connected to Lichess."
                  : "Live and completed games from this QueenUI session."}
          </p>
        </div>
        <div className="games-toolbar-controls">
          {/*
           * Focus is a drilled-in state, so the way out of it stands where the
           * control stands rather than beside it as a third segment. A
           * segmented control with nothing pressed is what the archive view
           * used to show, and "which of these am I in?" is exactly the question
           * this round exists to stop asking; one button that says where it
           * goes cannot be misread. Escape does the same thing for anyone who
           * knows it, which nobody does from looking at the screen.
           */}
          {inFocus ? (
            <button
              className="games-back"
              // Named for the move, not the destination: "All games" alone
              // reads as a filter next to the Live/All pair, and this is the
              // only control on the page that leaves a state.
              aria-label="Back to all games"
              onClick={leaveFocus}
            >
              <ArrowLeft size={14} aria-hidden="true" />
              All games
            </button>
          ) : (
            /*
             * The same segmented control the filter is drawn as, because it is
             * the same kind of choice about the same list: two presentations of
             * every game there is. One of them is always pressed — whichever
             * one is on screen — and neither can put a game the operator did
             * not choose in front of them.
             */
            boardGames.length > 0 && (
              <div
                className="game-filter view-switch"
                role="group"
                aria-label="Board view"
              >
                <OverviewButton
                  overview="grid"
                  current={overview}
                  onChoose={chooseOverview}
                  icon={<LayoutGrid size={14} aria-hidden="true" />}
                  label="Grid"
                />
                <OverviewButton
                  overview="detail"
                  current={overview}
                  onChoose={chooseOverview}
                  icon={<LayoutList size={14} aria-hidden="true" />}
                  label="Detail"
                />
              </div>
            )
          )}
          <div className="game-filter" role="group" aria-label="Game filter">
            <button
              className={filter === "live" ? "selected" : ""}
              aria-pressed={filter === "live"}
              onClick={() => setFilter("live")}
            >
              <i />
              Live <span>{liveCount}</span>
            </button>
            <button
              className={filter === "all" ? "selected" : ""}
              aria-pressed={filter === "all"}
              onClick={() => setFilter("all")}
            >
              All <span>{boardGames.length}</span>
            </button>
          </div>
        </div>
      </section>
      {/*
       * Above the boards and outside the filter, on purpose. The filter starts
       * on "live" and a failed game is by definition not live, so anything
       * routed through it would be invisible on the screen an operator opens —
       * which is exactly how these games were lost the first time. It is also
       * why a failure is never a tile: the grid is boards, and this is not one.
       */}
      {failedGames.length > 0 && (
        <section className="failed-games" aria-label="Failed games">
          <p className="failed-games-lead">
            {countText(failedGames.length, "game")} stopped with an error.
            Dismissing one removes it from QueenUI; nothing on Lichess changes.
          </p>
          {failedGames.map((game) => (
            <FailedGameCard
              game={game}
              pending={busy.has(`dismiss-game-${game.id}`)}
              onDismiss={onDismissGameError}
              key={game.id}
            />
          ))}
        </section>
      )}
      {view === "focus" && focusGame ? (
        <LiveGamePanel
          game={focusGame}
          engineName={engineNameForGame(snapshot, focusGame)}
          moveSoundsEnabled={moveSoundsEnabled}
          onToggleMoveSounds={onToggleMoveSounds}
          boardTheme={boardTheme}
          pieceSet={pieceSet}
          stale={stale}
          collapsed={collapsed}
          onBoardThemeChange={onBoardThemeChange}
          onPieceSetChange={onPieceSetChange}
          onExportPgn={onExportPgn}
          onToggleWidget={toggleWidget}
        />
      ) : !visibleGames.length ? (
        emptyList
      ) : view === "grid" ? (
        <section
          className="games-grid"
          aria-label="Game boards"
          // How many tiles the row is allowed to be worth; the stylesheet
          // decides how many actually fit.
          style={{ "--tile-columns": tileColumns } as CSSProperties}
        >
          {visibleGames.map((game) => (
            <GameTile
              game={game}
              boardTheme={boardTheme}
              pieceSet={pieceSet}
              stale={stale}
              onFocus={focusOnGame}
              key={game.id}
            />
          ))}
        </section>
      ) : (
        /*
         * Detail: every game the filter admits, at the depth the focus view
         * shows one at — the presentation this page had before the grid, kept
         * because an operator reading two boards' telemetry side by side should
         * not have to choose between them. Each board is a way into its own
         * focus view, which is the same affordance a tile carries and the only
         * other way in.
         */
        <div className="games-detail">
          {visibleGames.map((game) => (
            <LiveGamePanel
              game={game}
              engineName={engineNameForGame(snapshot, game)}
              moveSoundsEnabled={moveSoundsEnabled}
              onToggleMoveSounds={onToggleMoveSounds}
              boardTheme={boardTheme}
              pieceSet={pieceSet}
              stale={stale}
              onBoardThemeChange={onBoardThemeChange}
              onPieceSetChange={onPieceSetChange}
              onExportPgn={onExportPgn}
              onFocus={focusOnGame}
              key={game.id}
            />
          ))}
        </div>
      )}
    </div>
  );
}
