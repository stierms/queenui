import type { LogsSource } from "../pages/LogsPage";
import {
  diagnosticLevel,
  type AppSnapshot,
  type DiagnosticEntry,
  type DiagnosticFilter,
  type DiagnosticLevel,
  type LogFilter,
  type LogHeaderField,
  type LogLine,
  type LogMatch,
  type LogPage,
  type LogSearchBlock,
  type LogSessionMatches,
  type LogSessionSummary,
  type LogsOverview,
} from "../types";

/** True when running a dev build with the given `?name` URL parameter set. */
export function hasPreviewParam(name: string): boolean {
  return (
    import.meta.env.DEV && new URLSearchParams(window.location.search).has(name)
  );
}

export const presentationPreviewSnapshot: AppSnapshot = {
  engines: [
    {
      id: "preview-engine",
      name: "Queen 0.42 NNUE",
      path: "C:\\Engines\\queen.exe",
      author: "QueenUI",
      optionCount: 60,
      options: [
        {
          name: "Hash",
          optionType: "spin",
          defaultValue: "16",
          value: "512",
          min: 1,
          max: 65536,
          choices: [],
        },
        {
          name: "Threads",
          optionType: "spin",
          defaultValue: "1",
          value: "8",
          min: 1,
          max: 128,
          choices: [],
        },
        {
          name: "SyzygyPath",
          optionType: "string",
          defaultValue: "",
          value: "",
          min: null,
          max: null,
          choices: [],
        },
        ...Array.from({ length: 57 }, (_, index) => ({
          name: `Preview option ${index + 1}`,
          optionType: "spin",
          defaultValue: "1",
          value: "1",
          min: 1,
          max: 128,
          choices: [],
        })),
      ],
      openingBook: null,
    },
  ],
  accounts: [
    {
      id: "preview-account",
      username: "QueenBot",
      engineId: "preview-engine",
      rating: 2487,
      enabled: true,
    },
  ],
  runtimes: [{ accountId: "preview-account", status: "playing", error: null }],
  games: [
    {
      id: "P7vQ9kLm",
      accountId: "preview-account",
      botUsername: "QueenBot",
      opponent: "TacticalRaven",
      botRating: 2487,
      opponentRating: 2531,
      color: "black",
      initialFen: "startpos",
      moves:
        "e2e4 e7e5 g1f3 b8c6 f1b5 a7a6 b5a4 g8f6 e1g1 f8e7 f1e1 b7b5 a4b3 d7d6 c2c3 e8g8 h2h3",
      status: "started",
      whiteTime: 145820,
      blackTime: 151400,
      whiteIncrement: 2000,
      blackIncrement: 2000,
      clockUpdatedAt: Date.now(),
      result: null,
      error: null,
      engineThinking: true,
      engineLine:
        "info depth 22 seldepth 31 score cp 28 nodes 8421093 nps 5120000 time 1644 hashfull 417 pv c6a5 b3c2 c7c5 d2d4",
      engineInfo: {
        depth: 22,
        selectiveDepth: 31,
        scoreCp: 28,
        nodes: 8421093,
        nodesPerSecond: 5120000,
        timeMs: 1644,
        hashFull: 417,
        tablebaseHits: 0,
        multiPv: 1,
        principalVariation: ["c6a5", "b3c2", "c7c5", "d2d4"],
        raw: "info depth 22 seldepth 31 score cp 28 nodes 8421093 nps 5120000 time 1644 hashfull 417 pv c6a5 b3c2 c7c5 d2d4",
      },
    },
  ],
  campaigns: [],
  campaignRuntimes: [],
};

type PreviewGameSpec = {
  id: string;
  opponent: string;
  opponentRating: number;
  color: "white" | "black";
  moves: string;
  status?: string;
  result?: string | null;
  whiteTime?: number;
  blackTime?: number;
  /** Frozen clocks: `remainingClock` does not interpolate without this. */
  clockUpdatedAt?: number;
  thinking?: boolean;
  scoreCp?: number;
  mateIn?: number;
  depth?: number;
};

function previewGame(spec: PreviewGameSpec): AppSnapshot["games"][number] {
  const [template] = presentationPreviewSnapshot.games;
  return {
    ...template,
    id: spec.id,
    opponent: spec.opponent,
    opponentRating: spec.opponentRating,
    color: spec.color,
    moves: spec.moves,
    status: spec.status ?? "started",
    result: spec.result ?? null,
    whiteTime: spec.whiteTime ?? template.whiteTime,
    blackTime: spec.blackTime ?? template.blackTime,
    clockUpdatedAt: spec.clockUpdatedAt ?? Date.now(),
    engineThinking: spec.thinking ?? false,
    engineInfo: {
      ...template.engineInfo!,
      // The generated telemetry omits an absent score rather than nulling it
      // (serde `skip_serializing_if`), so a mate line has no `scoreCp`.
      scoreCp: spec.mateIn == null ? (spec.scoreCp ?? 0) : undefined,
      mateIn: spec.mateIn,
      depth: spec.depth ?? 24,
    },
  };
}

/**
 * The live boards behind `?games-preview`.
 *
 * Four, because four is what a 2560px desktop fits in one row of the grid, and
 * a fixture that only ever showed two would never once draw the layout the
 * grid is sized for. Their evaluations deliberately disagree — a small plus, a
 * minus, a clear advantage, a forced mate — so the eval bars under the boards
 * are four different lengths instead of four identical ones.
 *
 * `BulletHawk` carries the low clock, with `clockUpdatedAt: 0` so the clock is
 * shown exactly as the (imaginary) server sent it: `remainingClock` only
 * extrapolates from a real update time, so the urgency chip stays at fourteen
 * seconds instead of running out while the screen is being looked at.
 */
const PREVIEW_LIVE_GAMES: PreviewGameSpec[] = [
  {
    id: "P7vQ9kLm",
    opponent: "TacticalRaven",
    opponentRating: 2531,
    color: "black",
    moves:
      "e2e4 e7e5 g1f3 b8c6 f1b5 a7a6 b5a4 g8f6 e1g1 f8e7 f1e1 b7b5 a4b3 d7d6 c2c3 e8g8 h2h3",
    whiteTime: 145820,
    blackTime: 151400,
    thinking: true,
    scoreCp: 28,
    depth: 22,
  },
  {
    id: "K3xR8dTa",
    opponent: "SilentBishop",
    opponentRating: 2402,
    color: "white",
    moves: "d2d4 g8f6 c2c4 e7e6 g1f3 d7d5 b1c3 f8e7 c1g5 h7h6",
    whiteTime: 92400,
    blackTime: 88150,
    scoreCp: -64,
  },
  {
    id: "R6nH2sVx",
    opponent: "BulletHawk",
    opponentRating: 2194,
    color: "white",
    moves:
      "e2e4 c7c5 g1f3 d7d6 d2d4 c5d4 f3d4 g8f6 b1c3 a7a6 f1e2 e7e5 d4b3 f8e7 e1g1 e8g8",
    whiteTime: 14200,
    blackTime: 46700,
    clockUpdatedAt: 0,
    thinking: true,
    scoreCp: 231,
    depth: 26,
  },
  {
    id: "Z9pL2cVb",
    opponent: "MirrorMatchBot",
    opponentRating: 2610,
    color: "black",
    moves: "d2d4 d7d5 c2c4 c7c6 g1f3 g8f6 b1c3 d5c4 a2a4 c8f5 e2e3 e7e6 f1c4",
    whiteTime: 61300,
    blackTime: 58800,
    mateIn: 4,
    depth: 33,
  },
];

/** Finished boards, so "All" has an archive and the page has a second row. */
const PREVIEW_ARCHIVE_GAMES: PreviewGameSpec[] = [
  {
    id: "W2mB4nQe",
    opponent: "EndgameOwl",
    opponentRating: 2288,
    color: "white",
    moves: "e2e4 c7c5 g1f3 d7d6 d2d4 c5d4 f3d4 g8f6 b1c3 a7a6",
    status: "mate",
    result: "1-0",
    mateIn: 2,
    depth: 31,
  },
  {
    id: "T4kY7bNm",
    opponent: "RookRaider",
    opponentRating: 2455,
    color: "black",
    moves: "e2e4 c7c6 d2d4 d7d5 b1c3 d5e4 c3e4 c8f5 e4g3 f5g6 h2h4 h7h6",
    status: "resign",
    result: "0-1",
    scoreCp: -412,
  },
  {
    id: "J8sW3rQp",
    opponent: "PawnStorm",
    opponentRating: 2107,
    color: "white",
    moves: "d2d4 f7f5 g2g3 g8f6 f1g2 e7e6 g1f3 f8e7 e1g1 e8g8 c2c4 d7d6",
    status: "outoftime",
    result: "1-0",
    scoreCp: 86,
  },
  {
    id: "C2vF9hLd",
    opponent: "QuietKnight",
    opponentRating: 2340,
    color: "black",
    moves: "e2e4 e7e5 g1f3 b8c6 f1c4 g8f6 f3g5 d7d5 e4d5 c6a5 c4b5 c7c6",
    status: "draw",
    result: "1/2-1/2",
    scoreCp: 4,
  },
  {
    id: "N5dQ8xRt",
    opponent: "ZugzwangZebra",
    opponentRating: 2523,
    color: "white",
    moves: "c2c4 e7e5 b1c3 g8f6 g1f3 b8c6 g2g3 d7d5 c4d5 f6d5 f1g2 d5b6",
    status: "mate",
    result: "1-0",
    mateIn: 1,
    depth: 29,
  },
  {
    id: "B7gM4zKw",
    opponent: "OpeningOtter",
    opponentRating: 1988,
    color: "black",
    moves: "e2e4 e7e6 d2d4 d7d5 e4e5 c7c5 c2c3 b8c6 g1f3 d8b6 f1e2 c5d4",
    status: "resign",
    result: "1-0",
    scoreCp: -298,
  },
  {
    id: "H3rT6yPc",
    opponent: "SwindleSwan",
    opponentRating: 2276,
    color: "white",
    moves: "e2e4 d7d5 e4d5 d8d5 b1c3 d5a5 d2d4 g8f6 g1f3 c8f5 f1d3 f5d3",
    status: "timeout",
    result: "0-1",
    scoreCp: -37,
  },
  {
    id: "V1cX5jFb",
    opponent: "LateralLynx",
    opponentRating: 2418,
    color: "black",
    moves: "d2d4 g8f6 c2c4 g7g6 b1c3 d7d5 c4d5 f6d5 e2e4 d5c3 b2c3 f8g7",
    status: "aborted",
    result: null,
    scoreCp: 15,
  },
];

/**
 * A multi-game list for `?games-preview`: four live boards for the grid, an
 * archive behind the "All" filter, and a failed game.
 *
 * The failure is not decoration. A retained game error is the state this page
 * used to render as nothing at all, and a fixture where every game is healthy
 * is exactly how that path rots back — the same reason `?logs-faults` exists.
 *
 * `?games-preview&games-duo` narrows it to two live boards, which is the other
 * shape the grid has to look deliberate in: fewer games, larger tiles.
 */
function previewGamesList(): AppSnapshot["games"] {
  const [template] = presentationPreviewSnapshot.games;
  const live = PREVIEW_LIVE_GAMES.map(previewGame);
  if (hasPreviewParam("games-duo")) return live.slice(0, 2);
  return [
    ...live,
    ...PREVIEW_ARCHIVE_GAMES.map(previewGame),
    {
      ...template,
      id: "F5tD1jRk",
      opponent: "GambitFalcon",
      opponentRating: 2361,
      color: "white",
      moves: "e2e4 e7e5 g1f3 b8c6 f1c4 f8c5 c2c3 g8f6",
      status: "error",
      result: null,
      error:
        "Engine process exited during search (exit code 0xC0000005). The game was abandoned on move 8.",
      engineThinking: false,
      engineInfo: null,
      engineLine: null,
    },
  ];
}

/* ===== Engines preview (`?engines-preview`) ===== */

/**
 * One card per probe state, so all three can be seen at once.
 *
 * A card's badge is a claim about a probe that happened at a point in time, and
 * the states differ only in data the backend fills in — which is exactly the
 * kind of difference a fixture with one happy engine hides.
 */
const ENGINES_PREVIEW_ENGINES: AppSnapshot["engines"] = [
  {
    ...presentationPreviewSnapshot.engines[0],
    probeOk: true,
    lastProbedAtMs: Date.now() - 6 * 60_000,
  },
  {
    ...presentationPreviewSnapshot.engines[0],
    id: "preview-engine-stale",
    name: "Rookery 3.1",
    path: "C:\\Engines\\rookery.exe",
    author: "Rookery Labs",
    // A retained profile whose executable stopped answering: the options and
    // book are still worth keeping, the readiness claim is not.
    probeOk: false,
    lastProbedAtMs: Date.now() - 3 * 86_400_000,
  },
  {
    ...presentationPreviewSnapshot.engines[0],
    id: "preview-engine-unprobed",
    name: "Dragon 3",
    path: "C:\\Engines\\dragon.exe",
    author: null,
    // Saved before QueenUI recorded probe results, and config load does not
    // probe — so nothing is known either way.
    probeOk: undefined,
    lastProbedAtMs: undefined,
  },
];

/* ===== Logs preview (`?logs-preview`) ===== */

/** A second engine and bot so the Logs filters have something to choose. */
const LOGS_PREVIEW_ENGINES: AppSnapshot["engines"] = [
  ...presentationPreviewSnapshot.engines,
  {
    id: "preview-engine-2",
    name: "Rookery 3.1",
    path: "C:\\Engines\\rookery.exe",
    author: "Rookery Labs",
    optionCount: 18,
    options: [],
    openingBook: null,
  },
];

const LOGS_PREVIEW_ACCOUNTS: AppSnapshot["accounts"] = [
  ...presentationPreviewSnapshot.accounts,
  {
    id: "preview-account-2",
    username: "QueenBotBlitz",
    engineId: "preview-engine-2",
    rating: 2312,
    enabled: true,
  },
];

type PreviewLogSession = {
  summary: LogSessionSummary;
  header: LogHeaderField[];
  /** The lines a page can return: what the gzip decodes to. */
  lines: LogLine[];
  outline: LogSearchBlock[];
  /** Lines already appended to the live tail, so growth stays bounded. */
  grown: number;
  /**
   * Lines the recorder has written that a page cannot return yet. The
   * summary counts every line written; the stream is flushed once per
   * completed move, so a live session is ahead of its own file by the whole
   * search in progress, and an interrupted one stays ahead forever. Keeping
   * the two counts apart here is what stops the viewer from quietly
   * assuming they agree.
   */
  unflushed: number;
};

const PV_MOVES = [
  "g1f3",
  "d2d4",
  "c2c4",
  "b1c3",
  "e2e3",
  "f1d3",
  "e1g1",
  "h2h3",
  "d1c2",
  "f1e1",
  "c1d2",
  "a1c1",
  "d4d5",
  "c3e4",
  "f3g5",
  "e4c5",
  "b2b4",
  "d3f5",
  "c2b3",
  "e3e4",
  "f5e6",
  "d5d6",
  "b4b5",
  "a2a4",
  "e6c7",
];

function pvLine(seed: number, depth: number) {
  const length = 3 + (depth % 5);
  const moves: string[] = [];
  for (let index = 0; index < length; index += 1) {
    moves.push(PV_MOVES[(seed * 7 + index * 5 + depth) % PV_MOVES.length]);
  }
  return moves.join(" ");
}

type PreviewSessionSpec = {
  id: string;
  gameId: string;
  accountId: string;
  botUsername: string;
  opponent: string;
  engineId: string;
  engineName: string;
  enginePath: string;
  color: "white" | "black";
  clock: string;
  startedAtMs: number;
  finishedAtMs: number | null;
  status: string | null;
  result: string | null;
  live: boolean;
  blocks: number;
  linesPerBlock: number;
  firstMoveNumber: number;
  /** Written but not decodable: a search in flight, or a truncated file. */
  unflushed: number;
};

/**
 * Generates a plausible UCI session: handshake, then one `go`…`bestmove`
 * cycle per block with a full iterative-deepening ladder, a few `currmove`
 * updates, and the occasional line on stderr.
 */
function buildPreviewSession(spec: PreviewSessionSpec): PreviewLogSession {
  const lines: LogLine[] = [];
  const outline: LogSearchBlock[] = [];
  let atMs = 0;
  const push = (
    direction: LogLine["direction"],
    text: string,
    stepMs: number,
  ) => {
    atMs += stepMs;
    lines.push({ index: lines.length, atMs, direction, text });
  };

  push("#", `session opened · QueenUI 0.4.1 · ${spec.engineName}`, 0);
  push(">", "uci", 3);
  push("<", `id name ${spec.engineName}`, 9);
  push("<", "id author QueenUI Labs", 1);
  push("<", "option name Hash type spin default 16 min 1 max 4096", 1);
  push("<", "option name Threads type spin default 1 min 1 max 64", 1);
  push("<", "option name UCI_ShowWDL type check default false", 1);
  push("<", "uciok", 1);
  push(">", "setoption name Hash value 512", 4);
  push(">", "setoption name Threads value 8", 1);
  push(">", "ucinewgame", 1);
  push(">", "isready", 1);
  push("!", "NNUE evaluation using queen-b1a8f2.nnue enabled", 14);
  push("<", "readyok", 3);
  push("#", `opening book: performance.bin · 8 plies played from book`, 2);

  const playedMoves: string[] = [];
  for (let block = 0; block < spec.blocks; block += 1) {
    const moveNumber = spec.firstMoveNumber + block;
    const ply = moveNumber * 2 - (spec.color === "white" ? 1 : 0);
    const startLine = lines.length;
    const scoreCp = Math.round(28 + Math.sin(block * 0.7) * 46 + block * 1.4);
    const maxDepth = 22 + (block % 9);
    const best = PV_MOVES[(block * 3 + 4) % PV_MOVES.length];

    push(
      "#",
      `move ${moveNumber} · ${spec.color} to play · ${lines.length} lines so far`,
      120,
    );
    playedMoves.push(PV_MOVES[block % PV_MOVES.length]);
    push(">", `position startpos moves ${playedMoves.slice(-16).join(" ")}`, 2);
    push(
      ">",
      `go wtime ${180_000 - block * 3_100} btime ${180_000 - block * 2_800} winc 2000 binc 2000`,
      1,
    );

    let nodes = 1_200;
    let elapsed = 0;
    for (let depth = 1; depth <= maxDepth; depth += 1) {
      nodes = Math.round(nodes * 1.35 + depth * 900);
      // UCI reports `time` in milliseconds; 5.1 Mnps puts a deep blitz
      // search in the 1–6 second range.
      elapsed = Math.max(1, Math.round(nodes / 5_100));
      const depthScore = scoreCp + Math.round(Math.cos(depth * 0.9) * 12);
      push(
        "<",
        `info depth ${depth} seldepth ${depth + 6} multipv 1 score cp ${depthScore} nodes ${nodes} nps 5102340 hashfull ${Math.min(999, depth * 31)} time ${elapsed} pv ${pvLine(block, depth)}`,
        depth < 8 ? 1 : 12,
      );
    }
    if (block % 7 === 3) {
      push("!", `tablebase probe timed out after 40 ms — continuing`, 3);
    }

    // Pad with `currmove` chatter so a session reaches a realistic size.
    let filler = 0;
    while (lines.length - startLine < spec.linesPerBlock - 2) {
      const depth = maxDepth - (filler % 4);
      push(
        "<",
        `info depth ${depth} currmove ${PV_MOVES[(filler * 3) % PV_MOVES.length]} currmovenumber ${(filler % 34) + 1}`,
        2,
      );
      filler += 1;
    }

    push(
      "<",
      `bestmove ${best} ponder ${PV_MOVES[(block * 5 + 2) % PV_MOVES.length]}`,
      4,
    );
    const endLine = lines.length - 1;
    outline.push({
      moveNumber,
      ply,
      // Search blocks carry the note line's UCI-style side to move, not the
      // session's Lichess-style colour. Mirroring the backend here keeps the
      // preview honest about what the page actually receives.
      color: spec.color === "white" ? "w" : "b",
      startLine,
      endLine,
      bestMove: best,
      elapsedMs: elapsed + (block % 5) * 130,
      depth: maxDepth,
      scoreCp,
      mateIn: null,
    });
  }

  if (!spec.live) {
    push("#", `game finished · ${spec.result ?? "unknown"}`, 200);
    push(">", "quit", 2);
  }

  const rawBytes = lines.reduce((sum, line) => sum + line.text.length + 24, 0);
  const header: LogHeaderField[] = [
    { key: "QueenUI", value: "0.4.1 (2026-07-18)" },
    { key: "Engine", value: spec.engineName },
    { key: "Engine path", value: spec.enginePath },
    { key: "Engine author", value: "QueenUI Labs" },
    { key: "Account", value: spec.botUsername },
    { key: "Opponent", value: spec.opponent },
    { key: "Game", value: `https://lichess.org/${spec.gameId}` },
    { key: "Time control", value: spec.clock },
    { key: "Colour", value: spec.color },
    { key: "Initial FEN", value: "startpos" },
    { key: "Options applied", value: "Hash=512, Threads=8, UCI_ShowWDL=false" },
    { key: "Opening book", value: "performance.bin · max 8 plies · top 25%" },
    { key: "Started", value: new Date(spec.startedAtMs).toISOString() },
  ];

  return {
    summary: {
      id: spec.id,
      kind: "game",
      gameId: spec.gameId,
      accountId: spec.accountId,
      botUsername: spec.botUsername,
      opponent: spec.opponent,
      engineId: spec.engineId,
      engineName: spec.engineName,
      color: spec.color,
      clock: spec.clock,
      startedAtMs: spec.startedAtMs,
      finishedAtMs: spec.finishedAtMs,
      status: spec.status,
      result: spec.result,
      // Deliberately ahead of `lines.length`, which is what `getPage`
      // reports: the two counts mean different things.
      lineCount: lines.length + spec.unflushed,
      searchCount: outline.length,
      compressedBytes: Math.round(rawBytes / 8.4),
      rawBytes,
      droppedLines: 0,
      live: spec.live,
    },
    header,
    lines,
    outline,
    grown: 0,
    unflushed: spec.unflushed,
  };
}

const MINUTE = 60_000;
const HOUR = 3_600_000;

function buildPreviewSessions(): PreviewLogSession[] {
  const now = Date.now();
  return [
    buildPreviewSession({
      id: "session-live",
      gameId: "P7vQ9kLm",
      accountId: "preview-account",
      botUsername: "QueenBot",
      opponent: "TacticalRaven",
      engineId: "preview-engine",
      engineName: "Queen 0.42 NNUE",
      enginePath: "C:\\Engines\\queen.exe",
      color: "black",
      clock: "3+2",
      startedAtMs: now - 7 * MINUTE,
      finishedAtMs: null,
      status: "started",
      result: null,
      live: true,
      blocks: 32,
      linesPerBlock: 124,
      firstMoveNumber: 5,
      // A search is always in flight on a live board.
      unflushed: 86,
    }),
    buildPreviewSession({
      id: "session-owl",
      gameId: "W2mB4nQe",
      accountId: "preview-account",
      botUsername: "QueenBot",
      opponent: "EndgameOwl",
      engineId: "preview-engine",
      engineName: "Queen 0.42 NNUE",
      enginePath: "C:\\Engines\\queen.exe",
      color: "white",
      clock: "1+0",
      startedAtMs: now - 3 * HOUR,
      finishedAtMs: now - 3 * HOUR + 4 * MINUTE,
      status: "mate",
      result: "1-0",
      live: false,
      blocks: 14,
      linesPerBlock: 62,
      firstMoveNumber: 6,
      unflushed: 0,
    }),
    buildPreviewSession({
      id: "session-bishop",
      gameId: "K3xR8dTa",
      accountId: "preview-account-2",
      botUsername: "QueenBotBlitz",
      opponent: "SilentBishop",
      engineId: "preview-engine-2",
      engineName: "Rookery 3.1",
      enginePath: "C:\\Engines\\rookery.exe",
      color: "black",
      clock: "5+3",
      startedAtMs: now - 26 * HOUR,
      finishedAtMs: now - 26 * HOUR + 11 * MINUTE,
      status: "resign",
      result: "0-1",
      live: false,
      blocks: 21,
      linesPerBlock: 88,
      firstMoveNumber: 4,
      // Interrupted: the tail of this file never made it to disk.
      unflushed: 57,
    }),
    buildPreviewSession({
      id: "session-mirror",
      gameId: "Z9pL2cVb",
      accountId: "preview-account",
      botUsername: "QueenBot",
      opponent: "MirrorMatchBot",
      engineId: "preview-engine",
      engineName: "Queen 0.42 NNUE",
      enginePath: "C:\\Engines\\queen.exe",
      color: "white",
      clock: "3+2",
      startedAtMs: now - 4 * 24 * HOUR,
      finishedAtMs: now - 4 * 24 * HOUR + 7 * MINUTE,
      status: "draw",
      result: "1/2-1/2",
      live: false,
      blocks: 9,
      linesPerBlock: 54,
      firstMoveNumber: 7,
      unflushed: 0,
    }),
  ];
}

function previewDiagnostics(): DiagnosticEntry[] {
  const now = Date.now();
  const rows: Array<[number, DiagnosticLevel, string, string, string | null]> =
    [
      [
        40_000,
        "warn",
        "lichess",
        "Event stream reconnected after 2 attempts",
        "GET /api/stream/event closed by peer (code 1006); backoff 1s, 4s.",
      ],
      [
        95_000,
        "info",
        "engine",
        "Queen 0.42 NNUE started for game P7vQ9kLm",
        "pid 18244 · Hash=512 Threads=8 · handshake 118 ms",
      ],
      [
        150_000,
        "error",
        "storage",
        "Could not rotate engine log for game K3xR8dTa",
        "os error 32: The process cannot access the file because it is being used by another process.",
      ],
      [
        260_000,
        "warn",
        "lichess",
        "Declining challenge from BlitzGremlin failed",
        "HTTP 429 Too Many Requests · retry-after 60",
      ],
      [
        420_000,
        "info",
        "campaign",
        "Scan complete — 312 bots online, 18 eligible",
        null,
      ],
      [
        690_000,
        "error",
        "engine",
        "Rookery 3.1 exited during search",
        "exit code 0xC0000005 · restarted, game continued from move 27",
      ],
      [910_000, "info", "lichess", "Game stream opened for W2mB4nQe", null],
      [
        1_240_000,
        "warn",
        "engine",
        "Move submitted 480 ms after the clock deadline",
        "engine reported bestmove at 2 480 ms for a 2 000 ms budget",
      ],
      [
        1_600_000,
        "info",
        "storage",
        "Retention pruned 3 sessions (412 MB cap)",
        null,
      ],
      [
        2_050_000,
        "error",
        "lichess",
        "Token rejected for QueenBotBlitz",
        "HTTP 401 · the account token may have been revoked",
      ],
      [
        2_600_000,
        "warn",
        "storage",
        "Disk space below 2 GB — capture may be paused",
        "free 1.7 GB on C:",
      ],
      [3_300_000, "info", "campaign", "Matchmaking stopped by operator", null],
    ];
  return rows.map(([ago, level, source, message, detail], index) => ({
    id: `preview-diagnostic-${index}`,
    atMs: now - ago,
    level,
    source,
    accountId: index % 3 === 0 ? "preview-account-2" : "preview-account",
    gameId: null,
    message,
    detail,
  }));
}

function escapeForRegExp(value: string) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function previewMatcher(query: {
  text: string;
  regex: boolean;
  caseSensitive: boolean;
}) {
  const source = query.regex ? query.text : escapeForRegExp(query.text);
  try {
    return new RegExp(source, query.caseSensitive ? "" : "i");
  } catch {
    return null;
  }
}

function matchesLogFilter(session: LogSessionSummary, filter: LogFilter) {
  if (filter.accountId && session.accountId !== filter.accountId) return false;
  if (filter.engineId && session.engineId !== filter.engineId) return false;
  if (filter.query) {
    const haystack =
      `${session.opponent ?? ""} ${session.gameId ?? ""} ${session.engineName} ${session.botUsername}`.toLowerCase();
    if (!haystack.includes(filter.query.toLowerCase())) return false;
  }
  return true;
}

/** Two more lines of engine output each poll, so Follow visibly works. */
const LIVE_GROWTH_PER_POLL = 2;
const LIVE_GROWTH_CAP = 900;

function growLiveSession(session: PreviewLogSession) {
  if (!session.summary.live || session.grown >= LIVE_GROWTH_CAP) return;
  const last = session.lines[session.lines.length - 1];
  let atMs = last ? last.atMs : 0;
  for (let step = 0; step < LIVE_GROWTH_PER_POLL; step += 1) {
    atMs += 90;
    const depth = 18 + ((session.grown + step) % 11);
    session.lines.push({
      index: session.lines.length,
      atMs,
      direction: "<",
      text: `info depth ${depth} seldepth ${depth + 5} multipv 1 score cp ${24 + ((session.grown + step) % 17)} nodes ${4_200_000 + session.grown * 9_137} nps 5102340 time ${820 + session.grown * 7} pv ${pvLine(session.grown, depth)}`,
    });
  }
  session.grown += LIVE_GROWTH_PER_POLL;
  // The encoder flushes once per completed move, so the gap between what the
  // summary counts and what a page can return opens up over a search and
  // closes when the move lands. The viewer has to track the second number.
  const sinceFlush = session.grown % 40;
  session.unflushed = sinceFlush === 0 ? 0 : sinceFlush * 5;
  session.summary = {
    ...session.summary,
    lineCount: session.lines.length + session.unflushed,
    rawBytes: session.summary.rawBytes + LIVE_GROWTH_PER_POLL * 130,
    compressedBytes:
      session.summary.compressedBytes + LIVE_GROWTH_PER_POLL * 16,
  };
}

function buildLogsPreviewSource(): LogsSource {
  let sessions = buildPreviewSessions();
  const diagnostics = previewDiagnostics();
  const find = (sessionId: string) =>
    sessions.find((session) => session.summary.id === sessionId);
  /**
   * `?logs-preview&logs-faults` makes every read that has a failure path
   * reject, so the error states can be seen in a browser instead of only in
   * a test. A clean fixture is exactly how a failure path rots.
   */
  const faults = hasPreviewParam("logs-faults");
  const fault = (command: string) =>
    Promise.reject(new Error(`${command} failed: preview fault injection`));

  return {
    listSessions: (filter) =>
      Promise.resolve(
        sessions
          .map((session) => session.summary)
          .filter((summary) => matchesLogFilter(summary, filter)),
      ),
    searchSessions: (filter, query) => {
      const matcher = previewMatcher(query);
      if (!matcher) return Promise.resolve([]);
      const rows: LogSessionMatches[] = [];
      for (const session of sessions) {
        if (!matchesLogFilter(session.summary, filter)) continue;
        let matchCount = 0;
        let first: LogMatch | null = null;
        for (const line of session.lines) {
          if (!matcher.test(line.text)) continue;
          matchCount += 1;
          if (!first) {
            first = {
              lineIndex: line.index,
              direction: line.direction,
              text: line.text,
            };
          }
        }
        if (matchCount > 0) {
          rows.push({ session: session.summary, matchCount, first });
        }
      }
      return Promise.resolve(rows);
    },
    getPage: (sessionId, offset, limit) => {
      const session = find(sessionId);
      if (!session) return Promise.reject(new Error("unknown session"));
      // Every page but the first, so the fault mode still shows real lines
      // above the failure rather than an empty pane.
      if (faults && offset > 0) return fault("get_log_page");
      growLiveSession(session);
      const page: LogPage = {
        sessionId,
        // Only what the file decodes to — never the summary's count.
        totalLines: session.lines.length,
        offset,
        lines: session.lines.slice(offset, offset + limit),
        header: session.header,
        live: session.summary.live,
      };
      return Promise.resolve(page);
    },
    getOutline: (sessionId) =>
      faults
        ? fault("get_log_outline")
        : Promise.resolve(find(sessionId)?.outline ?? ([] as LogSearchBlock[])),
    searchSession: (sessionId, query) => {
      if (faults) return fault("search_log_session");
      const session = find(sessionId);
      const matcher = previewMatcher(query);
      if (!session || !matcher) return Promise.resolve([]);
      const found: LogMatch[] = [];
      for (const line of session.lines) {
        if (found.length >= query.limit) break;
        if (!matcher.test(line.text)) continue;
        found.push({
          lineIndex: line.index,
          direction: line.direction,
          text: line.text,
        });
      }
      return Promise.resolve(found);
    },
    exportSession: () => Promise.resolve(),
    deleteSession: (sessionId) => {
      sessions = sessions.filter((session) => session.summary.id !== sessionId);
      return Promise.resolve();
    },
    clearSessions: () => {
      const removed = sessions.length;
      sessions = [];
      return Promise.resolve(removed);
    },
    getOverview: () => {
      if (faults) return fault("get_logs_overview");
      const overview: LogsOverview = {
        sessionCount: sessions.length,
        compressedBytes: sessions.reduce(
          (sum, session) => sum + session.summary.compressedBytes,
          0,
        ),
        rawBytes: sessions.reduce(
          (sum, session) => sum + session.summary.rawBytes,
          0,
        ),
        oldestStartedAtMs: sessions.length
          ? Math.min(...sessions.map((session) => session.summary.startedAtMs))
          : null,
        liveCount: sessions.filter((session) => session.summary.live).length,
        retention: { captureEnabled: true, maxTotalMb: 512, maxAgeDays: 30 },
      };
      return Promise.resolve(overview);
    },
    getDiagnostics: (filter: DiagnosticFilter) => {
      const rank: Record<DiagnosticLevel, number> = {
        info: 0,
        warn: 1,
        error: 2,
      };
      const minimum = filter.level ? rank[diagnosticLevel(filter.level)] : 0;
      return Promise.resolve(
        diagnostics.filter((entry) => {
          if (rank[diagnosticLevel(entry.level)] < minimum) return false;
          if (filter.source && entry.source !== filter.source) return false;
          if (filter.accountId && entry.accountId !== filter.accountId) {
            return false;
          }
          if (filter.query) {
            const haystack =
              `${entry.source} ${entry.message} ${entry.detail ?? ""}`.toLowerCase();
            if (!haystack.includes(filter.query.toLowerCase())) return false;
          }
          return true;
        }),
      );
    },
    clearDiagnostics: () => {
      diagnostics.length = 0;
      return Promise.resolve();
    },
    subscribeLogs: () => () => undefined,
    subscribeDiagnostics: (callback) => {
      let emitted = 0;
      const timer = window.setInterval(() => {
        emitted += 1;
        if (emitted > 4) return;
        callback({
          id: `preview-diagnostic-live-${emitted}`,
          atMs: Date.now(),
          level:
            emitted % 3 === 0 ? "error" : emitted % 2 === 0 ? "warn" : "info",
          source: "lichess",
          accountId: "preview-account",
          gameId: "P7vQ9kLm",
          message: `Live diagnostic ${emitted} — stream heartbeat received`,
          detail: emitted % 2 === 0 ? "last heartbeat 6.0 s ago" : null,
        });
      }, 6000);
      return () => window.clearInterval(timer);
    },
  };
}

let cachedLogsSource: LogsSource | null = null;

/**
 * The `?logs-preview` data set. Memoized: the Logs page keys effects on the
 * source identity, so a fresh object per render would loop.
 */
export function logsPreviewSource(): LogsSource {
  if (!cachedLogsSource) cachedLogsSource = buildLogsPreviewSource();
  return cachedLogsSource;
}

export type PreviewState = {
  gamesPreview: boolean;
  logsPreview: boolean;
  enginesPreview: boolean;
  presentationPreview: boolean;
  previewSnapshot: AppSnapshot;
};

/**
 * DEV-only presentation previews driven by URL parameters
 * (`?game-preview`, `?games-preview`, `?logs-preview`, `?engines-preview`).
 * Behavior matches production builds where every flag is always false.
 */
export function previewState(): PreviewState {
  const gamesPreview = hasPreviewParam("games-preview");
  const logsPreview = hasPreviewParam("logs-preview");
  const enginesPreview = hasPreviewParam("engines-preview");
  const presentationPreview =
    hasPreviewParam("game-preview") ||
    gamesPreview ||
    logsPreview ||
    enginesPreview;
  const previewSnapshot = gamesPreview
    ? { ...presentationPreviewSnapshot, games: previewGamesList() }
    : logsPreview
      ? {
          ...presentationPreviewSnapshot,
          engines: LOGS_PREVIEW_ENGINES,
          accounts: LOGS_PREVIEW_ACCOUNTS,
        }
      : enginesPreview
        ? { ...presentationPreviewSnapshot, engines: ENGINES_PREVIEW_ENGINES }
        : presentationPreviewSnapshot;
  return {
    gamesPreview,
    logsPreview,
    enginesPreview,
    presentationPreview,
    previewSnapshot,
  };
}
