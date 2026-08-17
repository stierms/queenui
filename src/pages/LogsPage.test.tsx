import {
  act,
  cleanup,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { save } from "@tauri-apps/plugin-dialog";
import * as commands from "../api/commands";
import { onDiagnostic, onLogsUpdated } from "../api/events";
import type { RunAction } from "../hooks/useActionRunner";
import {
  diagnosticLevel,
  type AppSnapshot,
  type DiagnosticEntry,
  type DiagnosticFilter,
  type DiagnosticLevel,
  type LogDirection,
  type LogLine,
  type LogPage,
  type LogSearchBlock,
  type LogSessionSummary,
} from "../types";
import { LogsPage } from "./LogsPage";

vi.mock("../api/commands", () => ({
  listLogSessions: vi.fn(),
  searchLogSessions: vi.fn(),
  getLogPage: vi.fn(),
  getLogOutline: vi.fn(),
  searchLogSession: vi.fn(),
  exportLogSession: vi.fn(),
  deleteLogSession: vi.fn(),
  clearLogSessions: vi.fn(),
  getLogsOverview: vi.fn(),
  getDiagnostics: vi.fn(),
  clearDiagnostics: vi.fn(),
}));

vi.mock("../api/events", () => ({
  onLogsUpdated: vi.fn(() => () => undefined),
  onDiagnostic: vi.fn(() => () => undefined),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn() }));

const NOW = 1_750_000_000_000;
const TOTAL_LINES = 4_000;
const DIRECTIONS: LogDirection[] = [">", "<", "!", "#"];

const snapshot: AppSnapshot = {
  engines: [
    {
      id: "engine-1",
      name: "Queen 0.42",
      path: "C:\\queen.exe",
      author: null,
      optionCount: 12,
      options: [],
      openingBook: null,
    },
  ],
  accounts: [
    {
      id: "queenbot",
      username: "QueenBot",
      engineId: "engine-1",
      rating: 2487,
      enabled: true,
    },
  ],
  runtimes: [{ accountId: "queenbot", status: "playing", error: null }],
  games: [],
  campaigns: [],
  campaignRuntimes: [],
};

const finishedSession: LogSessionSummary = {
  id: "session-1",
  kind: "game",
  gameId: "P7vQ9kLm",
  accountId: "queenbot",
  botUsername: "QueenBot",
  opponent: "TacticalRaven",
  engineId: "engine-1",
  engineName: "Queen 0.42",
  color: "black",
  clock: "3+2",
  startedAtMs: NOW - 3_600_000,
  finishedAtMs: NOW - 3_000_000,
  status: "mate",
  result: "0-1",
  lineCount: TOTAL_LINES,
  searchCount: 3,
  compressedBytes: 148_000,
  rawBytes: 1_240_000,
  droppedLines: 0,
  live: false,
};

const liveSession: LogSessionSummary = {
  ...finishedSession,
  id: "session-2",
  gameId: "K3xR8dTa",
  opponent: "SilentBishop",
  status: "started",
  result: null,
  finishedAtMs: null,
  lineCount: 180,
  live: true,
};

const outline: LogSearchBlock[] = [
  {
    moveNumber: 8,
    ply: 16,
    // Blocks carry the note line's "w"/"b", unlike a session's
    // "white"/"black"; both spellings must read as a black move.
    color: "b",
    startLine: 120,
    endLine: 260,
    bestMove: "c6a5",
    elapsedMs: 940,
    depth: 21,
    scoreCp: 14,
    mateIn: null,
  },
  {
    moveNumber: 12,
    ply: 24,
    color: "black",
    startLine: 2_000,
    endLine: 2_140,
    bestMove: "g1f3",
    elapsedMs: 1_640,
    depth: 24,
    scoreCp: 28,
    mateIn: null,
  },
  {
    moveNumber: 23,
    ply: 46,
    color: "black",
    startLine: 3_400,
    endLine: 3_520,
    bestMove: "d7d5",
    elapsedMs: 2_110,
    depth: 26,
    scoreCp: null,
    mateIn: 4,
  },
];

const diagnostics: DiagnosticEntry[] = [
  {
    id: "diag-error",
    atMs: NOW - 30_000,
    level: "error",
    source: "storage",
    accountId: "queenbot",
    gameId: null,
    message: "Could not rotate the engine log",
    detail: "os error 32: the file is in use",
  },
  {
    id: "diag-warn",
    atMs: NOW - 60_000,
    level: "warn",
    source: "lichess-stream",
    accountId: "queenbot",
    gameId: null,
    message: "Event stream reconnected after 2 attempts",
    detail: null,
  },
  {
    id: "diag-info",
    atMs: NOW - 90_000,
    level: "info",
    source: "engine",
    accountId: "queenbot",
    gameId: null,
    message: "Queen 0.42 started for game P7vQ9kLm",
    detail: null,
  },
];

const LEVEL_RANK: Record<DiagnosticLevel, number> = {
  info: 0,
  warn: 1,
  error: 2,
};

function lineAt(index: number): LogLine {
  return {
    index,
    atMs: index * 41,
    direction: DIRECTIONS[index % 4],
    text: `line ${index} info depth ${(index % 32) + 1} score cp 24`,
  };
}

function logPage(sessionId: string, offset: number, limit: number): LogPage {
  const total = sessionId === "session-2" ? 180 : TOTAL_LINES;
  return {
    sessionId,
    totalLines: total,
    offset,
    lines: Array.from(
      { length: Math.max(0, Math.min(limit, total - offset)) },
      (_unused, position) => lineAt(offset + position),
    ),
    header: [
      { key: "Engine", value: "Queen 0.42" },
      { key: "Engine path", value: "C:\\Engines\\queen.exe" },
      { key: "Options applied", value: "Hash=512, Threads=8" },
    ],
    live: sessionId === "session-2",
  };
}

/** Captures the live-diagnostic callback the page registers. */
let emitDiagnostic: ((entry: DiagnosticEntry) => void) | undefined;
/** Captures the `logs-updated` callback, so a refresh can be forced. */
let logsUpdated: (() => void) | undefined;
/** Live diagnostic subscriptions taken out over one render. */
let diagnosticSubscriptions = 0;

function renderPage() {
  const showNotice = vi.fn();
  const runAction: RunAction = async (_key, action, success) => {
    try {
      await action();
      if (success) showNotice("success", success);
      return true;
    } catch {
      showNotice("error", "action failed");
      return false;
    }
  };
  render(
    <LogsPage
      snapshot={snapshot}
      busy={new Set<string>()}
      runAction={runAction}
      showNotice={showNotice}
    />,
  );
  return { showNotice };
}

beforeEach(() => {
  vi.clearAllMocks();
  emitDiagnostic = undefined;
  logsUpdated = undefined;
  diagnosticSubscriptions = 0;
  vi.mocked(onDiagnostic).mockImplementation((callback) => {
    emitDiagnostic = callback;
    diagnosticSubscriptions += 1;
    return () => undefined;
  });
  vi.mocked(onLogsUpdated).mockImplementation((callback) => {
    logsUpdated = callback;
    return () => undefined;
  });
  vi.mocked(commands.listLogSessions).mockResolvedValue([
    finishedSession,
    liveSession,
  ]);
  vi.mocked(commands.searchLogSessions).mockResolvedValue([
    {
      session: finishedSession,
      matchCount: 37,
      first: {
        lineIndex: 118,
        direction: "<",
        text: "bestmove c6a5 ponder b3c2",
      },
    },
  ]);
  vi.mocked(commands.getLogPage).mockImplementation(
    (sessionId, offset, limit) =>
      Promise.resolve(logPage(sessionId, offset, limit)),
  );
  vi.mocked(commands.getLogOutline).mockResolvedValue(outline);
  vi.mocked(commands.searchLogSession).mockResolvedValue([
    { lineIndex: 40, direction: "<", text: "bestmove e2e4" },
    { lineIndex: 1_600, direction: "<", text: "bestmove g1f3" },
    { lineIndex: 3_210, direction: "<", text: "bestmove d7d5" },
  ]);
  vi.mocked(commands.exportLogSession).mockResolvedValue(undefined);
  vi.mocked(commands.deleteLogSession).mockResolvedValue(undefined);
  vi.mocked(commands.clearLogSessions).mockResolvedValue(2);
  vi.mocked(commands.getLogsOverview).mockResolvedValue({
    sessionCount: 2,
    compressedBytes: 296_000,
    rawBytes: 2_480_000,
    oldestStartedAtMs: NOW - 86_400_000,
    liveCount: 1,
    retention: { captureEnabled: true, maxTotalMb: 512, maxAgeDays: 30 },
  });
  vi.mocked(commands.getDiagnostics).mockImplementation(
    (filter: DiagnosticFilter) => {
      const minimum = filter.level
        ? LEVEL_RANK[diagnosticLevel(filter.level)]
        : 0;
      return Promise.resolve(
        diagnostics.filter(
          (entry) => LEVEL_RANK[diagnosticLevel(entry.level)] >= minimum,
        ),
      );
    },
  );
  vi.mocked(commands.clearDiagnostics).mockResolvedValue(undefined);
});

afterEach(cleanup);

describe("LogsPage engine sessions", () => {
  it("lists sessions and opens the newest one in the viewer", async () => {
    renderPage();

    expect(
      await screen.findByRole("heading", {
        name: /Queen 0.42 vs. TacticalRaven/,
      }),
    ).toBeInTheDocument();
    expect(vi.mocked(commands.listLogSessions)).toHaveBeenCalledWith({
      accountId: null,
      engineId: null,
      fromMs: null,
      toMs: null,
      query: null,
      limit: 200,
    });

    const list = screen.getByLabelText("Recorded sessions");
    expect(within(list).getByText("TacticalRaven")).toBeInTheDocument();
    expect(within(list).getByText("SilentBishop")).toBeInTheDocument();
    expect(within(list).getByText("#P7vQ9kLm")).toBeInTheDocument();
    // The live row uses the moss live-dot eyebrow.
    expect(
      within(list).getByText("Live", { selector: ".live-eyebrow" }),
    ).toBeInTheDocument();

    const summary = screen.getByLabelText("Recording summary");
    expect(within(summary).getByText("289 KB")).toBeInTheDocument();
    // Same units the Settings panel edits these two limits in.
    expect(within(summary).getByText("0.5 GB · 30 days")).toBeInTheDocument();

    expect(await screen.findByText(/^line 0 /)).toBeInTheDocument();
    expect(screen.getByText("Session header")).toBeInTheDocument();
    expect(screen.getByText("C:\\Engines\\queen.exe")).toBeInTheDocument();
  });

  it("renders the outline rail and scrolls the viewer to a block", async () => {
    const user = userEvent.setup();
    renderPage();
    await screen.findByText(/^line 0 /);

    const rail = screen.getByLabelText("Search blocks");
    // Black's searches are numbered "12…"; white's would read "12.".
    const block = within(rail).getByRole("button", { name: /^12…/ });
    expect(block).toHaveTextContent("d24");
    expect(block).toHaveTextContent("+0.28");
    expect(block).toHaveTextContent("1.64 s");
    expect(block).toHaveTextContent("g1f3");
    // A forced mate wins over the centipawn column, spelled the same way
    // the board's evaluation readout spells it.
    expect(
      within(rail).getByRole("button", { name: /^23…/ }),
    ).toHaveTextContent("M4");
    // The backend spells a block's side to move "b", not "black".
    expect(within(rail).getByRole("button", { name: /^8…/ })).toBeVisible();

    await user.click(block);

    await waitFor(() =>
      expect(document.querySelector('[data-line="2000"]')).toBeTruthy(),
    );
    expect(document.querySelector('[data-line="2000"]')).toHaveClass(
      "log-line-active",
    );
    expect(screen.queryByText(/^line 0 /)).not.toBeInTheDocument();
  });

  it("searches inside the session and steps through the matches", async () => {
    const user = userEvent.setup();
    renderPage();
    await screen.findByText(/^line 0 /);

    await user.type(
      screen.getByLabelText("Search inside this session"),
      "bestmove{Enter}",
    );

    await waitFor(() =>
      expect(vi.mocked(commands.searchLogSession)).toHaveBeenCalledWith(
        "session-1",
        { text: "bestmove", regex: false, caseSensitive: false, limit: 500 },
      ),
    );
    expect(await screen.findByText("1 of 3")).toBeInTheDocument();
    await waitFor(() =>
      expect(document.querySelector('[data-line="40"]')).toHaveClass(
        "log-line-active",
      ),
    );

    await user.click(screen.getByRole("button", { name: "Next match" }));
    expect(screen.getByText("2 of 3")).toBeInTheDocument();
    await waitFor(() =>
      expect(document.querySelector('[data-line="1600"]')).toHaveClass(
        "log-line-active",
      ),
    );

    await user.click(screen.getByRole("button", { name: "Previous match" }));
    expect(screen.getByText("1 of 3")).toBeInTheDocument();
  });

  it("re-runs the search with the regex and case toggles", async () => {
    const user = userEvent.setup();
    renderPage();
    await screen.findByText(/^line 0 /);

    await user.type(
      screen.getByLabelText("Search inside this session"),
      "best.*ve{Enter}",
    );
    await screen.findByText("1 of 3");

    await user.click(
      screen.getByRole("button", { name: "Regular expression" }),
    );
    await user.click(screen.getByRole("button", { name: "Match case" }));

    await waitFor(() =>
      expect(vi.mocked(commands.searchLogSession)).toHaveBeenLastCalledWith(
        "session-1",
        { text: "best.*ve", regex: true, caseSensitive: true, limit: 500 },
      ),
    );
  });

  it("switches the list into cross-session search mode", async () => {
    const user = userEvent.setup();
    renderPage();
    await screen.findByText(/^line 0 /);

    await user.click(
      screen.getByRole("switch", { name: "Search inside logs" }),
    );
    await user.type(screen.getByLabelText("Filter sessions"), "bestmove");

    await waitFor(() =>
      expect(vi.mocked(commands.searchLogSessions)).toHaveBeenCalledWith(
        expect.objectContaining({ query: null }),
        { text: "bestmove", regex: false, caseSensitive: false, limit: 500 },
      ),
    );
    expect(await screen.findByText("37 matches")).toBeInTheDocument();
    expect(screen.getByText("bestmove c6a5 ponder b3c2")).toBeInTheDocument();
  });

  it("filters the list by account and engine", async () => {
    const user = userEvent.setup();
    renderPage();
    await screen.findByText(/^line 0 /);

    await user.selectOptions(
      screen.getByLabelText("Filter sessions by engine"),
      "engine-1",
    );

    await waitFor(() =>
      expect(vi.mocked(commands.listLogSessions)).toHaveBeenCalledWith(
        expect.objectContaining({ engineId: "engine-1", accountId: null }),
      ),
    );
  });

  it("exports the selected session in the chosen mode", async () => {
    const user = userEvent.setup();
    vi.mocked(save).mockResolvedValue("C:\\logs\\session.txt");
    renderPage();
    await screen.findByText(/^line 0 /);

    await user.click(screen.getByRole("button", { name: /Export/ }));
    await user.click(
      await screen.findByRole("button", { name: /Plain UCI transcript/ }),
    );

    await waitFor(() =>
      expect(vi.mocked(commands.exportLogSession)).toHaveBeenCalledWith(
        "session-1",
        "C:\\logs\\session.txt",
        "plain",
      ),
    );
  });

  it("deletes a session only after confirmation", async () => {
    const user = userEvent.setup();
    renderPage();
    await screen.findByText(/^line 0 /);

    await user.click(screen.getByRole("button", { name: "Delete session" }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("Delete this log session?");

    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(vi.mocked(commands.deleteLogSession)).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "Delete session" }));
    await user.click(
      within(await screen.findByRole("dialog")).getByRole("button", {
        name: "Delete session",
      }),
    );

    await waitFor(() =>
      expect(vi.mocked(commands.deleteLogSession)).toHaveBeenCalledWith(
        "session-1",
      ),
    );
  });

  it("shows an empty state when nothing has been recorded", async () => {
    vi.mocked(commands.listLogSessions).mockResolvedValue([]);
    renderPage();

    expect(await screen.findByText("Nothing recorded yet")).toBeInTheDocument();
    expect(screen.getByText("No session selected")).toBeInTheDocument();
  });

  it("offers a retry when the log service fails", async () => {
    vi.mocked(commands.listLogSessions).mockRejectedValue(
      new Error("backend down"),
    );
    renderPage();

    expect(
      await screen.findByText("The log service didn’t answer"),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Retry" })).toBeInTheDocument();
  });

  it("reports the line count a page can return, and what is not flushed", async () => {
    // The summary counts every line written; the recorder flushes once per
    // completed move, so the pages are behind by the search in progress.
    vi.mocked(commands.listLogSessions).mockResolvedValue([
      { ...liveSession, lineCount: 260 },
    ]);
    renderPage();

    // 180 readable, 260 written.
    expect(await screen.findByText(/180 lines/)).toHaveTextContent(
      /180 lines · \+80 unflushed/,
    );
  });
});

describe("LogsPage session selection", () => {
  it("keeps the open session while the filter is typed", async () => {
    const user = userEvent.setup();
    renderPage();
    await screen.findByText(/^line 0 /);

    // Open the second session explicitly.
    await user.click(screen.getByText("SilentBishop"));
    expect(
      await screen.findByRole("heading", {
        name: /Queen 0.42 vs. SilentBishop/,
      }),
    ).toBeInTheDocument();

    // A filter that excludes it must not hand the pane to another game.
    vi.mocked(commands.listLogSessions).mockResolvedValue([finishedSession]);
    await user.type(screen.getByLabelText("Filter sessions"), "Raven");

    await waitFor(() =>
      expect(vi.mocked(commands.listLogSessions)).toHaveBeenCalledWith(
        expect.objectContaining({ query: "Raven" }),
      ),
    );
    await waitFor(() =>
      expect(
        screen.queryByRole("button", { name: /SilentBishop/ }),
      ).not.toBeInTheDocument(),
    );
    expect(
      screen.getByRole("heading", { name: /Queen 0.42 vs. SilentBishop/ }),
    ).toBeInTheDocument();
    expect(screen.getByText(/stays open even though the list/)).toBeVisible();
  });

  it("does not let a new session take over the pane", async () => {
    renderPage();
    await screen.findByText(/^line 0 /);
    // The newest session is opened for the operator on the first load.
    expect(
      screen.getByRole("heading", { name: /Queen 0.42 vs. TacticalRaven/ }),
    ).toBeInTheDocument();

    // A game starting pushes a newer row to the top of the list.
    const newer: LogSessionSummary = {
      ...liveSession,
      id: "session-3",
      gameId: "N3wG4me1",
      opponent: "FreshChallenger",
    };
    vi.mocked(commands.listLogSessions).mockResolvedValue([
      newer,
      finishedSession,
      liveSession,
    ]);
    await act(async () => {
      logsUpdated?.();
    });

    await screen.findByText("FreshChallenger");
    expect(
      screen.getByRole("heading", { name: /Queen 0.42 vs. TacticalRaven/ }),
    ).toBeInTheDocument();
  });

  it("returns focus to the session list after a delete", async () => {
    const user = userEvent.setup();
    renderPage();
    await screen.findByText(/^line 0 /);

    await user.click(screen.getByRole("button", { name: "Delete session" }));
    await user.click(
      within(await screen.findByRole("dialog")).getByRole("button", {
        name: "Delete session",
      }),
    );

    await waitFor(() =>
      expect(vi.mocked(commands.deleteLogSession)).toHaveBeenCalled(),
    );
    // The button that opened the dialog unmounted with the viewer, so Radix
    // has nothing to restore to; focus must not fall to <body>.
    await waitFor(() =>
      expect(document.activeElement).toBe(
        screen.getByLabelText("Recorded sessions"),
      ),
    );
    // The next session opens rather than nothing at all.
    expect(
      screen.getByRole("heading", { name: /Queen 0.42 vs. SilentBishop/ }),
    ).toBeInTheDocument();
  });
});

describe("LogsPage failure states", () => {
  it("distinguishes a failed in-session search from an empty one", async () => {
    const user = userEvent.setup();
    vi.mocked(commands.searchLogSession).mockRejectedValue(
      new Error("regex parse error at offset 6"),
    );
    renderPage();
    await screen.findByText(/^line 0 /);

    await user.type(
      screen.getByLabelText("Search inside this session"),
      "score (?{Enter}",
    );

    // "No matches" would read as "your text isn't in this log" — and the
    // reason has to be on screen, not only in a hover title.
    expect(await screen.findByText(/^Search failed — /)).toBeInTheDocument();
    expect(screen.queryByText("No matches")).not.toBeInTheDocument();
  });

  it("says the outline could not be read instead of claiming there is none", async () => {
    vi.mocked(commands.getLogOutline).mockRejectedValue(
      new Error("gzip stream corrupt"),
    );
    renderPage();
    await screen.findByText(/^line 0 /);

    const rail = await screen.findByLabelText("Search blocks");
    expect(
      within(rail).getByText("The search outline could not be read."),
    ).toBeInTheDocument();
    expect(
      within(rail).queryByText("No completed searches yet."),
    ).not.toBeInTheDocument();

    // Retry asks again.
    vi.mocked(commands.getLogOutline).mockResolvedValue(outline);
    await userEvent.click(within(rail).getByRole("button", { name: "Retry" }));
    expect(
      await within(rail).findByRole("button", { name: /^12…/ }),
    ).toBeInTheDocument();
  });

  it("says the recording summary failed instead of hiding the strip", async () => {
    vi.mocked(commands.getLogsOverview).mockRejectedValue(
      new Error("storage unavailable"),
    );
    renderPage();

    expect(
      await screen.findByText("The recording summary couldn’t be read."),
    ).toBeInTheDocument();
    expect(
      screen.queryByLabelText("Recording summary"),
    ).not.toBeInTheDocument();
  });

  it("reports a save dialog that rejects", async () => {
    const user = userEvent.setup();
    vi.mocked(save).mockRejectedValue(new Error("dialog plugin unavailable"));
    const { showNotice } = renderPage();
    await screen.findByText(/^line 0 /);

    await user.click(screen.getByRole("button", { name: /Export/ }));
    await user.click(
      await screen.findByRole("button", { name: /Plain UCI transcript/ }),
    );

    await waitFor(() =>
      expect(showNotice).toHaveBeenCalledWith(
        "error",
        expect.stringContaining("dialog plugin unavailable"),
      ),
    );
    expect(vi.mocked(commands.exportLogSession)).not.toHaveBeenCalled();
  });
});

describe("LogsPage outline keyboard", () => {
  it("is a single tab stop the arrow keys move within", async () => {
    const user = userEvent.setup();
    renderPage();
    await screen.findByText(/^line 0 /);

    const rail = screen.getByRole("toolbar", { name: "Search blocks" });
    const rows = within(rail).getAllByRole("button");
    expect(rows).toHaveLength(3);
    // One tab stop for the whole rail, not one per search.
    expect(rows.filter((row) => row.tabIndex === 0)).toHaveLength(1);
    expect(rows[0].tabIndex).toBe(0);

    rows[0].focus();
    await user.keyboard("{ArrowDown}");
    expect(document.activeElement).toBe(rows[1]);
    expect(rows[1].tabIndex).toBe(0);
    expect(rows[0].tabIndex).toBe(-1);

    await user.keyboard("{End}");
    expect(document.activeElement).toBe(rows[2]);
    await user.keyboard("{Home}");
    expect(document.activeElement).toBe(rows[0]);
  });
});

describe("LogsPage diagnostics", () => {
  async function openDiagnostics(user: ReturnType<typeof userEvent.setup>) {
    renderPage();
    await screen.findByRole("tab", { name: /App diagnostics/ });
    await user.click(screen.getByRole("tab", { name: /App diagnostics/ }));
    return screen.findByLabelText("Diagnostic entries");
  }

  it("treats the level filter as a minimum", async () => {
    const user = userEvent.setup();
    const list = await openDiagnostics(user);

    await waitFor(() =>
      expect(vi.mocked(commands.getDiagnostics)).toHaveBeenCalledWith({
        level: "info",
        source: null,
        accountId: null,
        query: null,
        limit: 1000,
      }),
    );
    expect(
      await within(list).findByText("Queen 0.42 started for game P7vQ9kLm"),
    ).toBeInTheDocument();

    await user.selectOptions(screen.getByLabelText("Minimum level"), "warn");

    await waitFor(() =>
      expect(vi.mocked(commands.getDiagnostics)).toHaveBeenCalledWith(
        expect.objectContaining({ level: "warn" }),
      ),
    );
    await waitFor(() =>
      expect(
        within(list).queryByText("Queen 0.42 started for game P7vQ9kLm"),
      ).not.toBeInTheDocument(),
    );
    expect(
      within(list).getByText("Event stream reconnected after 2 attempts"),
    ).toBeInTheDocument();
    expect(
      within(list).getByText("Could not rotate the engine log"),
    ).toBeInTheDocument();
  });

  it("live-appends entries that pass the current filter", async () => {
    const user = userEvent.setup();
    const list = await openDiagnostics(user);
    await within(list).findByText("Could not rotate the engine log");

    await user.selectOptions(screen.getByLabelText("Minimum level"), "warn");
    await waitFor(() =>
      expect(vi.mocked(commands.getDiagnostics)).toHaveBeenCalledWith(
        expect.objectContaining({ level: "warn" }),
      ),
    );

    act(() =>
      emitDiagnostic?.({
        id: "diag-live-info",
        atMs: NOW,
        level: "info",
        source: "engine",
        accountId: "queenbot",
        gameId: null,
        message: "Quiet informational note",
        detail: null,
      }),
    );
    expect(
      within(list).queryByText("Quiet informational note"),
    ).not.toBeInTheDocument();

    act(() =>
      emitDiagnostic?.({
        id: "diag-live-error",
        atMs: NOW,
        level: "error",
        source: "engine",
        accountId: "queenbot",
        gameId: null,
        message: "Engine exited during search",
        detail: "exit code 0xC0000005",
      }),
    );
    const rows = within(list).getAllByRole("button");
    expect(rows[0]).toHaveTextContent("Engine exited during search");
    expect(rows[0].closest(".logs-diag-row")).toHaveClass("logs-diag-error");
  });

  it("keeps one subscription across filter changes and keeps what arrives", async () => {
    const user = userEvent.setup();
    // The refetch is held open, so the live entry lands while it is in
    // flight — exactly when a wholesale setEntries used to erase it.
    let release: ((rows: DiagnosticEntry[]) => void) | undefined;
    const list = await openDiagnostics(user);
    await within(list).findByText("Could not rotate the engine log");

    vi.mocked(commands.getDiagnostics).mockImplementation(
      () =>
        new Promise<DiagnosticEntry[]>((resolve) => {
          release = resolve;
        }),
    );
    await user.selectOptions(screen.getByLabelText("Minimum level"), "warn");
    await waitFor(() => expect(release).toBeDefined());

    // One listener for the life of the panel: re-filtering must not tear it
    // down and drop every event in the gap.
    expect(diagnosticSubscriptions).toBe(1);

    const arrived: DiagnosticEntry = {
      id: "diag-live-during-refetch",
      atMs: NOW,
      level: "error",
      source: "engine",
      accountId: "queenbot",
      gameId: null,
      message: "Engine exited while the list was reloading",
      detail: null,
    };
    act(() => emitDiagnostic?.(arrived));
    await act(async () => {
      release?.([diagnostics[0], diagnostics[1]]);
    });

    expect(
      within(list).getByText("Engine exited while the list was reloading"),
    ).toBeInTheDocument();
  });

  it("forgets the sources it learned when the record is cleared", async () => {
    const user = userEvent.setup();
    await openDiagnostics(user);
    const sources = screen.getByLabelText("Filter by source");
    await waitFor(() =>
      expect(within(sources).getByText("storage")).toBeInTheDocument(),
    );

    vi.mocked(commands.getDiagnostics).mockResolvedValue([]);
    await user.click(screen.getByRole("button", { name: /Clear/ }));
    await user.click(
      within(await screen.findByRole("dialog")).getByRole("button", {
        name: "Clear diagnostics",
      }),
    );

    // A source that no longer has a single entry behind it would only ever
    // filter down to a permanently empty pane.
    await waitFor(() =>
      expect(within(sources).queryByText("storage")).not.toBeInTheDocument(),
    );
    expect(within(sources).getByText("All sources")).toBeInTheDocument();
  });

  it("expands a detail and clears the record", async () => {
    const user = userEvent.setup();
    const list = await openDiagnostics(user);
    const row = await within(list).findByText(
      "Could not rotate the engine log",
    );

    await user.click(row);
    expect(
      within(list).getByText("os error 32: the file is in use"),
    ).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /Clear/ }));
    await user.click(
      within(await screen.findByRole("dialog")).getByRole("button", {
        name: "Clear diagnostics",
      }),
    );

    await waitFor(() =>
      expect(vi.mocked(commands.clearDiagnostics)).toHaveBeenCalled(),
    );
  });
});
