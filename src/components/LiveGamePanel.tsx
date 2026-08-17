import { memo } from "react";
import { Download, Volume2, VolumeX } from "lucide-react";
import type { PieceSetId } from "../ChessPiece";
import { useRetainedEvaluation } from "../hooks/useRetainedEvaluation";
import { boardAppearanceStyle, type BoardThemeId } from "../lib/appearance";
import { displayResult, isLiveGame, sideToMove } from "../lib/chess";
import type { CollapsedWidgets, GameWidget } from "../lib/gameView";
import { TooltipButton } from "../ui/primitives";
import { playerColor, type LiveGame } from "../types";
import { BoardAppearancePicker } from "./appearance";
import { Chessboard, EvalRail, LiveClock } from "./board";
import { EngineTelemetryPanel } from "./EngineTelemetryPanel";

export const LiveGamePanel = memo(function LiveGamePanel({
  game,
  engineName,
  moveSoundsEnabled,
  onToggleMoveSounds,
  boardTheme,
  pieceSet,
  stale = false,
  collapsed,
  onBoardThemeChange,
  onPieceSetChange,
  onExportPgn,
  onToggleWidget,
  onFocus,
}: {
  game: LiveGame;
  engineName: string;
  moveSoundsEnabled: boolean;
  onToggleMoveSounds: () => void;
  boardTheme: BoardThemeId;
  pieceSet: PieceSetId;
  /**
   * The snapshot feeding this panel may be out of date. Everything that
   * asserts liveness — the eyebrow, the clocks, the thinking pulse, the
   * best-move arrow — must stop asserting it.
   */
  stale?: boolean;
  /**
   * Which of the two widgets are put away, and how to toggle one. Passed by
   * the games surface's focus view; omitted by the Overview board, which
   * renders both widgets open and shows no chevrons.
   */
  collapsed?: CollapsedWidgets;
  onBoardThemeChange: (theme: BoardThemeId) => void;
  onPieceSetChange: (set: PieceSetId) => void;
  onExportPgn: (game: LiveGame) => void;
  onToggleWidget?: (widget: GameWidget) => void;
  /**
   * Makes this board a way into the focus view, exactly as a grid tile is —
   * same overlay, same accessible name, same handler — so choosing a game means
   * the same thing in both overviews.
   *
   * Passed by the Detail overview alone. The focus view is already the game this
   * would drill into and the Overview page's board has nowhere to drill to, so
   * both omit it and no overlay is rendered.
   */
  onFocus?: (game: LiveGame) => void;
}) {
  const whiteToMove = sideToMove(game) === "w";
  const ourTurn = (game.color === "white") === whiteToMove;
  const ourColor = game.color === "white" ? "White" : "Black";
  const opponentColor = game.color === "white" ? "Black" : "White";
  const ourTime = game.color === "white" ? game.whiteTime : game.blackTime;
  const opponentTime = game.color === "white" ? game.blackTime : game.whiteTime;
  // Clock highlights and interpolation only ever apply to live games.
  const gameRunning = isLiveGame(game);
  // A finished game cannot go stale — its state is final whatever the link does.
  const frozen = stale && gameRunning;
  // The eval bar and the evaluation tile hold the last score the engine
  // reported instead of dropping to 0.00 while the next search spins up.
  const evaluation = useRetainedEvaluation(game.engineInfo);
  /*
   * Both widgets away. The layout keeps its two columns and the second one
   * simply stops reserving the board's height, so collapsing moves nothing:
   * same board, same size, same page height, two headings where the widgets
   * were.
   */
  const widgetsPutAway =
    collapsed?.analysis === true && collapsed.moves === true;
  return (
    <article
      className={`panel live-panel ${frozen ? "panel-frozen" : ""}`}
      style={boardAppearanceStyle(boardTheme)}
    >
      <div className="panel-heading live-game-heading">
        <div>
          <span
            className={`eyebrow ${
              frozen
                ? "stale-eyebrow"
                : gameRunning
                  ? "live-eyebrow"
                  : "finished-eyebrow"
            }`}
          >
            <i />
            {frozen
              ? "Not live · waiting for the runner"
              : gameRunning
                ? "Live on Lichess"
                : `Finished · ${displayResult(game.result)}`}
          </span>
          <h2>
            {game.botUsername} <span>vs.</span> {game.opponent}
          </h2>
        </div>
        <div className="game-heading-actions">
          <span className="game-id">#{game.id}</span>
          <TooltipButton
            variant="icon"
            onClick={() => onExportPgn(game)}
            aria-label="Export PGN"
            tooltip="Export PGN"
          >
            <Download size={16} />
          </TooltipButton>
          <BoardAppearancePicker
            boardTheme={boardTheme}
            pieceSet={pieceSet}
            onBoardThemeChange={onBoardThemeChange}
            onPieceSetChange={onPieceSetChange}
          />
          <TooltipButton
            variant="icon"
            onClick={onToggleMoveSounds}
            aria-label={
              moveSoundsEnabled ? "Mute move sounds" : "Enable move sounds"
            }
            tooltip={
              moveSoundsEnabled ? "Mute move sounds" : "Enable move sounds"
            }
          >
            {moveSoundsEnabled ? <Volume2 size={16} /> : <VolumeX size={16} />}
          </TooltipButton>
        </div>
      </div>
      <div className={`game-layout ${widgetsPutAway ? "widgets-away" : ""}`}>
        <div className={`board-wrap ${onFocus ? "board-wrap-choosable" : ""}`}>
          <div className="player-row top-player">
            <span className="player-avatar opponent-avatar">
              {game.opponent[0]}
            </span>
            <div>
              <strong>{game.opponent}</strong>
              <small>
                Opponent · {opponentColor}
                {game.opponentRating != null ? ` · ${game.opponentRating}` : ""}
              </small>
            </div>
            <LiveClock
              milliseconds={opponentTime}
              active={gameRunning && !ourTurn}
              clockUpdatedAt={game.clockUpdatedAt}
              frozen={frozen}
            />
          </div>
          <div className="board-stage">
            {/* `LiveGame.color` is `string` on the wire (Lichess relays it);
                narrowed once here rather than widening the rail's prop. */}
            <EvalRail info={evaluation} ourColor={playerColor(game.color)} />
            <Chessboard game={game} pieceSet={pieceSet} frozen={frozen} />
          </div>
          <div className="player-row our-player">
            <span className="player-avatar queen-avatar">
              {game.botUsername[0]}
            </span>
            <div>
              <strong>{game.botUsername}</strong>
              <small>
                Your engine · {ourColor}
                {game.botRating != null ? ` · ${game.botRating}` : ""}
              </small>
            </div>
            <LiveClock
              milliseconds={ourTime}
              active={gameRunning && ourTurn}
              clockUpdatedAt={game.clockUpdatedAt}
              frozen={frozen}
            />
          </div>
          {/*
           * The board and its two player rows are the target; the panel's own
           * controls — export, appearance, sound — are outside this wrapper and
           * stay clickable. The label is the tile's label, word for word, so the
           * one thing an operator can do to a board is called the same thing
           * wherever they meet it. Nothing inside the overlay is announced: an
           * `aria-label` replaces the content, and the chip is there to make a
           * transparent hit area visible on hover, not to be read twice.
           */}
          {onFocus && (
            <button
              className="panel-hit"
              aria-label={`Focus on ${game.botUsername} versus ${
                game.opponent.trim() || "an unknown opponent"
              }`}
              onClick={() => onFocus(game)}
            >
              <span aria-hidden="true">Focus</span>
            </button>
          )}
        </div>
        <EngineTelemetryPanel
          game={game}
          engineName={engineName}
          evaluation={evaluation}
          frozen={frozen}
          collapsed={collapsed}
          onToggleWidget={onToggleWidget}
        />
      </div>
    </article>
  );
});
