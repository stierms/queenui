import { memo } from "react";
import type { PieceSetId } from "../ChessPiece";
import { useRetainedEvaluation } from "../hooks/useRetainedEvaluation";
import { boardAppearanceStyle, type BoardThemeId } from "../lib/appearance";
import {
  boardOrientation,
  colorToMove,
  displayResult,
  isLiveGame,
  opposingColor,
} from "../lib/chess";
import { evaluationLabel, evaluationPercent } from "../lib/evaluation";
import type { LiveGame, PlayerColor } from "../types";
import { Chessboard, LiveClock } from "./board";

/** One side of the board as the tile states it: who, which colour, how long. */
type Nameplate = {
  /**
   * Which edge of the board the plate is drawn on. Taken from
   * `boardOrientation`, never assumed — a plate on the opposite edge from the
   * pieces it names is worse than no plate at all.
   */
  edge: "top" | "bottom";
  /** True for our engine's plate; the opponent's is the other one. */
  ours: boolean;
  /** How the focus view names this player's part in the game. */
  role: string;
  name: string;
  rating: number | null;
  color: PlayerColor;
  milliseconds: number;
  active: boolean;
};

const COLOR_NAMES: Record<PlayerColor, string> = {
  white: "White",
  black: "Black",
};

/**
 * A player's nameplate: colour dot, name, rating, clock.
 *
 * The dot is the whole colour statement on screen — a tile has no room for the
 * word the focus view prints — and nothing about an 11px circle is announced,
 * so the colour and the player's part in the game are restated in text beside
 * it rather than left to sighted operators only.
 */
function TileNameplate({
  plate,
  clockUpdatedAt,
  frozen,
}: {
  plate: Nameplate;
  clockUpdatedAt: number;
  frozen: boolean;
}) {
  return (
    <div
      className={`tile-plate tile-plate-${plate.edge} ${
        plate.ours ? "tile-plate-ours" : ""
      }`}
    >
      <i
        className={`tile-color tile-color-${plate.color}`}
        aria-hidden="true"
      />
      <strong>{plate.name}</strong>
      <span className="sr-only">
        {plate.role} · {COLOR_NAMES[plate.color]}
      </span>
      {plate.rating != null && <small>{plate.rating}</small>}
      <span className="tile-clock">
        <LiveClock
          milliseconds={plate.milliseconds}
          active={plate.active}
          clockUpdatedAt={clockUpdatedAt}
          frozen={frozen}
        />
      </span>
    </div>
  );
}

/**
 * One live board in the grid.
 *
 * Not a smaller `LiveGamePanel`. The panel's job is to let an operator read one
 * game in depth; a tile's job is to let them scan eight at once and pick the
 * one that needs them. So the calc and match widgets are not shrunk down here,
 * they are rendered as *form*: the evaluation becomes a bar the width of the
 * board, and the two players become nameplates on the board's own edges, each
 * carrying its own clock. Everything else — depth, nodes, the principal
 * variation, the move list — is what Focus is for.
 *
 * The nameplates are the standard chess-GUI convention and they follow the
 * board, not a guess: `boardOrientation` says which colour is along the bottom
 * edge, the near plate is that colour and the far plate is the other one. A
 * tile used to name the opponent alone, in one fixed spot, which left every
 * board in the grid silent about who was playing which colour — and silent
 * about our own engine's name and rating entirely.
 *
 * The board itself is the same `Chessboard`, at the same fidelity: last-move
 * highlight, check glow, best-move arrow, result overlay. A tile that redrew
 * the position its own way would be a second implementation to keep honest.
 *
 * Truthfulness carries over intact. A frozen snapshot desaturates the board,
 * stops the clocks and says "Not live" instead of claiming a live game, and a
 * finished game keeps its result overlay — a tile never states more than the
 * panel would about the same game.
 */
export const GameTile = memo(function GameTile({
  game,
  boardTheme,
  pieceSet,
  stale = false,
  onFocus,
}: {
  game: LiveGame;
  boardTheme: BoardThemeId;
  pieceSet: PieceSetId;
  /** The snapshot behind this tile may be out of date. */
  stale?: boolean;
  onFocus: (game: LiveGame) => void;
}) {
  const gameRunning = isLiveGame(game);
  // A finished game cannot go stale — its state is final whatever the link does.
  const frozen = stale && gameRunning;
  // Same retained score the panel's eval rail and evaluation tile read, so a
  // tile and the focus view never disagree about what the engine last said.
  const evaluation = useRetainedEvaluation(game.engineInfo);
  const ourPercent = evaluationPercent(evaluation);
  /*
   * The near edge of the board is our engine's, because that is what
   * own-perspective means; the far edge is therefore the opponent's.
   */
  const ourColor = boardOrientation(game);
  const theirColor = opposingColor(ourColor);
  /*
   * The bar fills from the left with WHITE's share of the game, the way the
   * rail fills from the bottom with it. Scores arrive from our engine's
   * perspective, so playing black inverts them.
   */
  const whitePercent = ourColor === "black" ? 100 - ourPercent : ourPercent;
  const opponent = game.opponent.trim();
  const score = evaluationLabel(evaluation);
  /*
   * Each clock is looked up from its player's colour rather than from
   * "ours"/"theirs", so a clock and the nameplate it sits in cannot come apart:
   * one field decides which side of the board a player is on, which clock is
   * theirs, and whether it is running.
   */
  const toMove = colorToMove(game);
  const clockOf = (color: PlayerColor) =>
    color === "white" ? game.whiteTime : game.blackTime;
  const plateFor = (
    color: PlayerColor,
    rest: Pick<Nameplate, "edge" | "ours" | "role" | "name" | "rating">,
  ): Nameplate => ({
    ...rest,
    color,
    milliseconds: clockOf(color),
    active: gameRunning && toMove === color,
  });
  const farPlate = plateFor(theirColor, {
    edge: "top",
    ours: false,
    role: "Opponent",
    // The game context fills the opponent in from Lichess's `gameFull`; a
    // board that arrived before it has none to name.
    name: opponent || "Opponent unknown",
    rating: game.opponentRating ?? null,
  });
  const nearPlate = plateFor(ourColor, {
    edge: "bottom",
    ours: true,
    role: "Your engine",
    // The Lichess account the game is being played on, spelled the way the
    // focus view's match widget spells it.
    name: game.botUsername,
    rating: game.botRating ?? null,
  });
  return (
    <article
      className={`panel game-tile board-identity-${boardTheme} ${frozen ? "panel-frozen" : ""}`}
      style={boardAppearanceStyle(boardTheme)}
    >
      <div className="tile-top">
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
            ? "Not live"
            : gameRunning
              ? "Live"
              : `Finished · ${displayResult(game.result)}`}
        </span>
        {/*
         * A game can be running and still have reported an engine problem; the
         * panel prints that text in full. A tile has no room for it, but it has
         * room to say there is one — leaving the board looking untroubled is
         * how a fleet of twelve hides the one that needs attention.
         */}
        {game.error && (
          <span className="tile-problem" title={game.error}>
            Engine problem
          </span>
        )}
        {/*
         * The score in figures, where the far clock used to sit. The bar under
         * the board carries the same number as its accessible name, so this is
         * the sighted reading of it and is not announced twice.
         */}
        <span className="tile-score" aria-hidden="true">
          {score}
        </span>
      </div>
      <TileNameplate
        plate={farPlate}
        clockUpdatedAt={game.clockUpdatedAt}
        frozen={frozen}
      />
      <Chessboard game={game} pieceSet={pieceSet} frozen={frozen} />
      {/*
       * `role="img"` is load-bearing here as it is on the rail: an aria-label
       * on a role-less element is dropped, and a bar nobody can hear is a bar
       * that says nothing to half the operators who need it.
       */}
      <div
        className="tile-eval"
        role="img"
        aria-label={`Evaluation for our engine ${score}`}
      >
        <i style={{ width: `${whitePercent}%` }} />
      </div>
      <TileNameplate
        plate={nearPlate}
        clockUpdatedAt={game.clockUpdatedAt}
        frozen={frozen}
      />
      {/*
       * The whole tile is the target, but only this button is in the tab order
       * and only it carries a name. Wrapping the tile in a `<button>` instead
       * would fold the board, the clocks and the eyebrow into one enormous
       * accessible name and hide the squares' own labels.
       */}
      <button
        className="tile-hit"
        aria-label={`Focus on ${game.botUsername} versus ${opponent || "an unknown opponent"}`}
        onClick={() => onFocus(game)}
      />
    </article>
  );
});
