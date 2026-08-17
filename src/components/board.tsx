import { useEffect, useMemo, useState } from "react";
import {
  ChessPiece,
  type PieceColor,
  type PieceKind,
  type PieceSetId,
} from "../ChessPiece";
import {
  boardOrientation,
  clockExhausted,
  displayResult,
  formatClock,
  gameStatusLabel,
  pieceLabel,
  positionFor,
  remainingClock,
  squareCenter,
} from "../lib/chess";
import { evaluationLabel, evaluationPercent } from "../lib/evaluation";
import type { EngineTelemetry, LiveGame } from "../types";

/**
 * A player's clock.
 *
 * `frozen` is the honesty switch: when the connection that delivers clock
 * updates is degraded, the clock stops interpolating and shows the last value
 * the server actually sent, marked as such. A ticking clock on data that
 * stopped arriving is a *more* convincing live game than a real one, which is
 * exactly the failure this app must not have.
 */
export function LiveClock({
  milliseconds,
  active,
  clockUpdatedAt,
  frozen = false,
}: {
  milliseconds: number;
  active: boolean;
  clockUpdatedAt: number;
  frozen?: boolean;
}) {
  const running = active && !frozen;
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!running) return;
    const timer = window.setInterval(() => setNow(Date.now()), 200);
    return () => window.clearInterval(timer);
  }, [running]);
  const remaining = remainingClock(milliseconds, running, clockUpdatedAt, now);
  // Extrapolating past zero on a game still shown as running means the
  // display has outrun the server; say so instead of flagging the game.
  const unconfirmed = clockExhausted(remaining, running);
  const classes = [
    running ? "active-clock" : "",
    running && remaining < 20_000 ? "clock-low" : "",
    running && remaining < 10_000 ? "clock-critical" : "",
    frozen && active ? "clock-frozen" : "",
    unconfirmed ? "clock-unconfirmed" : "",
  ]
    .filter(Boolean)
    .join(" ");
  const title = frozen
    ? "Frozen at the last update from the runner"
    : unconfirmed
      ? "Ran out locally — waiting for Lichess to confirm"
      : undefined;
  return (
    <time className={classes} title={title}>
      {formatClock(remaining)}
      {frozen && active && <span className="sr-only"> (frozen)</span>}
    </time>
  );
}

/**
 * The move our engine currently prefers. Hidden while the connection is
 * degraded: the arrow says "the engine is considering this *now*", and on a
 * frozen snapshot that claim is no longer true.
 */
export function BestMoveArrow({
  game,
  frozen = false,
}: {
  game: LiveGame;
  frozen?: boolean;
}) {
  const move =
    game.engineThinking && !frozen
      ? game.engineInfo?.principalVariation?.[0]
      : undefined;
  if (!move || move.length < 4) return null;
  const from = squareCenter(move.slice(0, 2), game.color);
  const to = squareCenter(move.slice(2, 4), game.color);
  const length = Math.hypot(to.x - from.x, to.y - from.y) || 1;
  const unitX = (to.x - from.x) / length;
  const unitY = (to.y - from.y) / length;
  const markerId = `arrow-${game.id.replace(/[^a-zA-Z0-9_-]/g, "")}`;
  return (
    <svg
      className="best-move-arrow"
      viewBox="0 0 100 100"
      aria-label={`Engine prefers ${move.slice(0, 2)} to ${move.slice(2, 4)}`}
    >
      <defs>
        <marker
          id={markerId}
          viewBox="0 0 10 10"
          refX="7.5"
          refY="5"
          markerWidth="3.2"
          markerHeight="3.2"
          orient="auto-start-reverse"
        >
          <path d="M 0 0 L 10 5 L 0 10 z" />
        </marker>
      </defs>
      <line
        x1={from.x + unitX * 2.5}
        y1={from.y + unitY * 2.5}
        x2={to.x - unitX * 4.5}
        y2={to.y - unitY * 4.5}
        markerEnd={`url(#${markerId})`}
      />
    </svg>
  );
}

export function EvalRail({
  info,
  ourColor,
}: {
  info?: EngineTelemetry | null;
  ourColor?: "white" | "black";
}) {
  const ourPercent = evaluationPercent(info);
  /*
   * The bone fill always shows WHITE's share of the game. Scores arrive from
   * our engine's perspective, so when we play black the fill is inverted and
   * anchored to the top of the rail — our side stays at the bottom, matching
   * the board orientation.
   */
  const whitePercent = ourColor === "black" ? 100 - ourPercent : ourPercent;
  const fillFromTop = ourColor === "black";
  return (
    /*
     * `role="img"` is load-bearing: an aria-label on a role-less element is
     * dropped, so this rail used to be announced as nothing at all.
     */
    <div
      className="eval-rail"
      role="img"
      aria-label={`Evaluation for our engine ${evaluationLabel(info)}`}
    >
      <span className="eval-score" aria-hidden="true">
        {evaluationLabel(info)}
      </span>
      <div className="eval-track">
        <i
          className="eval-fill"
          style={{
            height: `${whitePercent}%`,
            ...(fillFromTop ? { top: 0, bottom: "auto" } : null),
          }}
        />
        <b />
      </div>
    </div>
  );
}

/*
 * Not `memo`-wrapped: `game` is a fresh object on every snapshot event, so a
 * shallow prop comparison could never short-circuit and the wrapper only read
 * as an optimization. The expensive part — replaying the game and mapping 64
 * squares — is memoized below on the fields it actually depends on, so
 * telemetry-only snapshots (a new depth, a new node count, several per second)
 * no longer rebuild the board.
 */
export function Chessboard({
  game,
  pieceSet,
  frozen = false,
}: {
  game: LiveGame;
  pieceSet: PieceSetId;
  /** The snapshot behind this position may be out of date. */
  frozen?: boolean;
}) {
  const { id, color, initialFen, moves } = game;
  const squares = useMemo(() => {
    // `positionFor` caches the replay across snapshots, so an unchanged move
    // list costs a Map lookup rather than a full game replay.
    const chess = positionFor({ id, initialFen, moves });
    const raw = chess.board().flat();
    /*
     * Own-perspective, stated once. `boardOrientation` is the single definition
     * of which colour sits along the bottom edge, so the squares, their file
     * and rank labels and a tile's nameplates cannot disagree about it.
     */
    const flipped = boardOrientation({ color }) === "black";
    const board = flipped ? [...raw].reverse() : raw;
    const moveList = moves.trim().split(/\s+/);
    const lastMove = moveList[moveList.length - 1] ?? "";
    const highlighted = new Set([lastMove.slice(0, 2), lastMove.slice(2, 4)]);
    const checkedColor = chess.inCheck() ? chess.turn() : null;
    return board.map((piece, index) => {
      const displayRow = Math.floor(index / 8);
      const displayCol = index % 8;
      const rank = flipped ? displayRow + 1 : 8 - displayRow;
      const file = flipped
        ? String.fromCharCode(104 - displayCol)
        : String.fromCharCode(97 + displayCol);
      const square = `${file}${rank}`;
      const inCheck =
        piece?.type === "k" && piece.color === checkedColor ? "in-check" : "";
      return (
        /*
         * `role="img"` for occupied squares: the label below is dropped
         * entirely on a role-less element, so the pieces used to be
         * unannounced. Empty squares stay silent rather than adding 30-odd
         * "empty square" stops to every traversal.
         */
        <div
          className={`square ${(displayRow + displayCol) % 2 ? "dark" : "light"} ${highlighted.has(square) ? "last-move" : ""} ${inCheck}`}
          role={piece ? "img" : undefined}
          aria-label={
            piece
              ? `${square}: ${pieceLabel(piece.color, piece.type)}`
              : undefined
          }
          key={square}
        >
          {piece && (
            <ChessPiece
              type={piece.type as PieceKind}
              color={piece.color as PieceColor}
              pieceSet={pieceSet}
            />
          )}
          {displayCol === 0 && <small className="rank">{rank}</small>}
          {displayRow === 7 && <small className="file">{file}</small>}
        </div>
      );
    });
  }, [id, color, initialFen, moves, pieceSet]);

  return (
    <div className={`board-surface ${frozen ? "board-frozen" : ""}`}>
      <div
        className="board"
        role="group"
        aria-label={
          frozen
            ? "Chess position, frozen at the last update from the runner"
            : "Current chess position"
        }
      >
        {squares}
      </div>
      <BestMoveArrow game={game} frozen={frozen} />
      {game.result && (
        // A game ending is worth announcing; the overlay was silent to AT.
        <div className="game-result-overlay" role="status">
          <span>Game over</span>
          <strong>{displayResult(game.result)}</strong>
          <small>{gameStatusLabel(game.status)}</small>
        </div>
      )}
    </div>
  );
}
