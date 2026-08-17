import { Chess } from "chess.js";
import {
  playerColor,
  type GameStatus,
  type LiveGame,
  type PlayerColor,
} from "../types";

/**
 * The fields a position replay actually needs. Narrower than `LiveGame` so a
 * caller can memoize on exactly these and not on the whole snapshot object.
 */
export type PositionSource = Pick<LiveGame, "id" | "initialFen" | "moves">;

function movesOf(game: PositionSource) {
  return game.moves.split(/\s+/).filter(Boolean);
}

/** Loads the game's initial FEN into `chess`; returns whether it applied. */
function loadInitialFen(chess: Chess, game: PositionSource) {
  if (!game.initialFen || game.initialFen === "startpos") return false;
  try {
    chess.load(game.initialFen);
    return true;
  } catch {
    /* keep the standard position visible for malformed FEN */
    return false;
  }
}

function applyUciMoves(chess: Chess, moves: string[]) {
  for (const move of moves) {
    try {
      chess.move({
        from: move.slice(0, 2),
        to: move.slice(2, 4),
        promotion: move[4],
      });
    } catch {
      break;
    }
  }
}

function buildPosition(game: PositionSource) {
  const chess = new Chess();
  loadInitialFen(chess, game);
  applyUciMoves(chess, movesOf(game));
  return chess;
}

/**
 * Cache keyed on the game's identity *and* its move list, not on the snapshot
 * object. Every snapshot event produces fresh game objects, so an
 * object-identity cache could never hit across snapshots: eight concurrent
 * campaign boards replayed eight full games from the start several times a
 * second, and the cost grew with game length.
 *
 * Bounded, and evicted oldest-first — a Map preserves insertion order, and
 * re-inserting a hit moves it to the end.
 */
const POSITION_CACHE_LIMIT = 64;
const positionCache = new Map<string, Chess>();

function positionKey(game: PositionSource) {
  return `${game.id}:${game.initialFen}:${game.moves}`;
}

/**
 * The position after every move of `game`. Treat the instance as read-only:
 * it is shared by every caller holding the same position.
 */
export function positionFor(game: PositionSource): Chess {
  const key = positionKey(game);
  const cached = positionCache.get(key);
  if (cached) {
    positionCache.delete(key);
    positionCache.set(key, cached);
    return cached;
  }
  const chess = buildPosition(game);
  positionCache.set(key, chess);
  if (positionCache.size > POSITION_CACHE_LIMIT) {
    const oldest = positionCache.keys().next();
    if (!oldest.done) positionCache.delete(oldest.value);
  }
  return chess;
}

function initialPositionFor(game: PositionSource) {
  const chess = new Chess();
  loadInitialFen(chess, game);
  return chess;
}

/** The side to move in the current position ("w" or "b"). */
export function sideToMove(game: LiveGame): "w" | "b" {
  return positionFor(game).turn();
}

/** The side to move, spelled the way `LiveGame.color` spells a colour. */
export function colorToMove(game: LiveGame): PlayerColor {
  return sideToMove(game) === "w" ? "white" : "black";
}

/**
 * Which colour a board is drawn from — the side along its bottom edge.
 *
 * Boards are own-perspective: `Chessboard` reverses the squares when we play
 * black, so our engine is always the near player and the opponent always the
 * far one. Anything that has to agree with the squares about which side is
 * where — the file and rank labels, the best-move arrow, a tile's two
 * nameplates — reads the orientation here instead of repeating the
 * `color === "black"` test. A nameplate on the opposite edge from the pieces it
 * names is worse than no nameplate, so there is one definition of the rule and
 * the labels move with the board if it ever changes.
 */
export function boardOrientation(game: Pick<LiveGame, "color">): PlayerColor {
  return playerColor(game.color);
}

/** The colour that is not `color`. */
export function opposingColor(color: PlayerColor): PlayerColor {
  return color === "white" ? "black" : "white";
}

/**
 * The two statuses that mean "still being played". This predicate gates the
 * close guard, so it must agree exactly with `runtime.rs`'s `is_live`.
 */
const LIVE_GAME_STATUSES: ReadonlySet<string> = new Set<GameStatus>([
  "started",
  "created",
]);

export function isLiveGame(game: LiveGame) {
  return LIVE_GAME_STATUSES.has(game.status);
}

/** The live games, in the order given. */
export function liveGamesOnly(games: readonly LiveGame[]) {
  return games.filter(isLiveGame);
}

/**
 * A game whose task died — `"error"` is set by `spawn_game_wrapper` when the
 * game future returns an error or panics, together with the text in
 * `game.error`. It is not a Lichess status and never arrives from Lichess.
 *
 * These used to disappear: `is_live` excluded them, so no board showed them,
 * and `prune_finished_games` swept them up with the finished games, so within
 * a snapshot or two there was nothing to find. A game the app had been playing
 * simply stopped existing, with the reason recorded nowhere the operator would
 * look. The backend now retains them until they are dismissed; this predicate
 * is what stops the frontend from repeating the disappearance.
 */
export function isFailedGame(game: LiveGame) {
  return game.status === "error";
}

/** The failed games, in the order given. */
export function failedGamesOnly(games: readonly LiveGame[]) {
  return games.filter(isFailedGame);
}

/**
 * The games that have a board worth rendering: live and genuinely finished.
 *
 * A failed game is neither. Passing one to a board panel would print
 * "Finished · *" over a half-played position and hide the only thing about it
 * that matters, so the two lists are kept apart at the source.
 */
export function boardGamesOnly(games: readonly LiveGame[]) {
  return games.filter((game) => !isFailedGame(game));
}

/**
 * How many games are live, optionally narrowed to one account.
 *
 * "How many games are live" is asked on four screens and had four answers.
 * Three of them filtered `snapshot.games` with `isLiveGame` separately and so
 * agreed by luck; the fourth — the Challenges capacity panel — read the
 * matchmaking scheduler's own `activeGames` counter, which
 * `stop_campaign` does not reset (it resets `pendingChallenges` only, see
 * `crates/queen-core/src/campaign.rs`). A stopped campaign therefore kept
 * reporting the games it *had been* playing: "Active games 2" on Challenges
 * while the sidebar badge, the Overview status strip and the Games page all
 * said 0. One function, so the number has one definition; the scheduler's own
 * counter now only describes a scheduler that is still running.
 */
export function countLiveGames(games: readonly LiveGame[], accountId?: string) {
  return games.filter(
    (game) =>
      isLiveGame(game) &&
      (accountId === undefined || game.accountId === accountId),
  ).length;
}

/**
 * Keyed on `GameStatus`, so adding a status to the union without giving it
 * copy is a compile error. Lichess may still send something not in the union
 * — that falls through to the de-camel-cased spelling below.
 */
const GAME_STATUS_LABELS: Record<GameStatus, string> = {
  created: "Waiting to start",
  started: "In progress",
  mate: "Checkmate",
  resign: "Resignation",
  stalemate: "Stalemate",
  timeout: "Time forfeit",
  outoftime: "Time forfeit",
  draw: "Draw",
  aborted: "Game aborted",
  cheat: "Game ended",
  noStart: "Never started",
  unknownFinish: "Game ended",
  variantEnd: "Game ended",
};

export function gameStatusLabel(status: string) {
  return (
    GAME_STATUS_LABELS[status as GameStatus] ??
    status
      .replace(/([A-Z])/g, " $1")
      .replace(/^./, (letter) => letter.toUpperCase())
  );
}

export function displayResult(result?: string | null) {
  return result === "1/2-1/2" ? "½ – ½" : (result?.replace("-", " – ") ?? "*");
}

export function formatClock(milliseconds: number) {
  const seconds = Math.max(0, Math.ceil(milliseconds / 1000));
  return `${Math.floor(seconds / 60)
    .toString()
    .padStart(2, "0")}:${(seconds % 60).toString().padStart(2, "0")}`;
}

/**
 * The clock as it stands now, extrapolated from the last server update.
 *
 * `active` is false — and the last server value is shown verbatim — whenever
 * the side is not to move, *or* the connection that would deliver the next
 * update is degraded. Interpolating on a dead link is what let a bot's clock
 * run visibly to 00:00 on a game that had been decided minutes earlier.
 */
export function remainingClock(
  milliseconds: number,
  active: boolean,
  clockUpdatedAt: number,
  now = Date.now(),
) {
  if (!active || !clockUpdatedAt) return milliseconds;
  return Math.max(0, milliseconds - Math.max(0, now - clockUpdatedAt));
}

/**
 * True when local extrapolation has burned the whole clock while the game is
 * still shown as running. Lichess would have ended such a game and sent a
 * final update, so reaching this state means the display is ahead of the
 * truth — the clock is rendered as unconfirmed rather than as a confident
 * flag fall.
 */
export function clockExhausted(remaining: number, active: boolean) {
  return active && remaining <= 0;
}

export function squareCenter(square: string, orientation: LiveGame["color"]) {
  const file = square.charCodeAt(0) - 97;
  const rank = 8 - Number(square[1]);
  const column = orientation === "black" ? 7 - file : file;
  const row = orientation === "black" ? 7 - rank : rank;
  return { x: (column + 0.5) * 12.5, y: (row + 0.5) * 12.5 };
}

const pieceNames: Record<string, string> = {
  p: "pawn",
  n: "knight",
  b: "bishop",
  r: "rook",
  q: "queen",
  k: "king",
};

export function pieceLabel(color: "w" | "b", type: string) {
  return `${color === "w" ? "white" : "black"} ${pieceNames[type] ?? type}`;
}

export function pgnForGame(game: LiveGame) {
  const chess = new Chess();
  if (loadInitialFen(chess, game)) {
    chess.setHeader("SetUp", "1");
    chess.setHeader("FEN", game.initialFen);
  }
  const whiteName = game.color === "white" ? game.botUsername : game.opponent;
  const blackName = game.color === "black" ? game.botUsername : game.opponent;
  const whiteRating =
    game.color === "white" ? game.botRating : game.opponentRating;
  const blackRating =
    game.color === "black" ? game.botRating : game.opponentRating;
  chess.setHeader("Event", "QueenUI Lichess Bot Game");
  chess.setHeader("Site", `https://lichess.org/${game.id}`);
  chess.setHeader(
    "Date",
    new Date(game.clockUpdatedAt || Date.now())
      .toISOString()
      .slice(0, 10)
      .replace(/-/g, "."),
  );
  chess.setHeader("Round", "-");
  chess.setHeader("White", whiteName);
  chess.setHeader("Black", blackName);
  if (whiteRating != null) chess.setHeader("WhiteElo", String(whiteRating));
  if (blackRating != null) chess.setHeader("BlackElo", String(blackRating));
  chess.setHeader("Result", game.result || "*");
  chess.setHeader("Termination", gameStatusLabel(game.status));
  applyUciMoves(chess, movesOf(game));
  return `${chess.pgn({ newline: "\n", maxWidth: 88 })}\n`;
}

export function latestMoveWasCapture(game: LiveGame) {
  const moves = movesOf(game);
  const history = positionFor(game).history({ verbose: true });
  if (history.length === 0 || history.length !== moves.length) return false;
  const last = history[history.length - 1];
  return last.flags.includes("c") || last.flags.includes("e");
}

export function principalVariationSan(game: LiveGame) {
  const pvMoves = game.engineInfo?.principalVariation?.slice(0, 10) ?? [];
  if (pvMoves.length === 0) return "";
  const chess = new Chess(positionFor(game).fen());
  const san: string[] = [];
  for (const uci of pvMoves) {
    try {
      san.push(
        chess.move({
          from: uci.slice(0, 2),
          to: uci.slice(2, 4),
          promotion: uci[4],
        }).san,
      );
    } catch {
      san.push(uci);
      break;
    }
  }
  return san.join(" ");
}

export type MoveRow = {
  number: number;
  white?: string;
  black?: string;
};

/**
 * Every full-move row of the game. Move numbers and the white/black columns
 * are seeded from the game's initial FEN, so positions where black moves
 * first (or that start beyond move 1) are numbered correctly.
 */
export function moveRows(game: LiveGame): MoveRow[] {
  const history = positionFor(game).history();
  if (history.length === 0) return [];
  const initial = initialPositionFor(game);
  const blackStarts = initial.turn() === "b";
  const firstNumber = initial.moveNumber();
  const padded: (string | undefined)[] = blackStarts
    ? [undefined, ...history]
    : history;
  return Array.from({ length: Math.ceil(padded.length / 2) }, (_, index) => ({
    number: firstNumber + index,
    white: padded[index * 2],
    black: padded[index * 2 + 1],
  }));
}
