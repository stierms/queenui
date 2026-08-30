import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { RunAction } from "../hooks/useActionRunner";
import type { ScorebookLab, ScorebookStats } from "../types";
import { ScorebookPage } from "./ScorebookPage";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => undefined)),
}));
vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: vi.fn(() => Promise.resolve()),
}));

const DAY = 86_400_000;
const NOW = 1_750_000_000_000;

const stats: ScorebookStats = {
  totalGames: 34,
  wins: 20,
  draws: 6,
  losses: 8,
  scorePercent: 67.6,
  streak: { kind: "win", length: 5 },
  avgOpponentRating: 2215,
  performanceRating: 2348,
  byEngine: [
    {
      engineId: "engine-1",
      engineName: "Queen",
      games: 30,
      wins: 19,
      draws: 5,
      losses: 6,
      scorePercent: 71.7,
      avgOpponentRating: 2222,
      performanceRating: 2360,
    },
    {
      engineId: null,
      engineName: "Imported / unknown",
      games: 4,
      wins: 1,
      draws: 1,
      losses: 2,
      scorePercent: 37.5,
      avgOpponentRating: 2100,
      performanceRating: null,
    },
  ],
  byColor: [
    {
      color: "white",
      games: 17,
      wins: 11,
      draws: 3,
      losses: 3,
      scorePercent: 73.5,
    },
    {
      color: "black",
      games: 17,
      wins: 9,
      draws: 3,
      losses: 5,
      scorePercent: 61.8,
    },
  ],
  byPerf: [
    {
      perf: "blitz",
      games: 34,
      wins: 20,
      draws: 6,
      losses: 8,
      scorePercent: 67.6,
    },
  ],
  byOpponentBand: [
    {
      label: "2000–2199",
      minRating: 2000,
      games: 14,
      wins: 10,
      draws: 2,
      losses: 2,
      scorePercent: 78.6,
    },
    {
      label: "2200–2399",
      minRating: 2200,
      games: 20,
      wins: 10,
      draws: 4,
      losses: 6,
      scorePercent: 60,
    },
  ],
  byTermination: [
    { status: "mate", games: 18, wins: 14, draws: 0, losses: 4 },
    { status: "resign", games: 13, wins: 6, draws: 3, losses: 4 },
  ],
  timeLosses: 2,
  topOpponents: [
    {
      name: "RivalBot",
      games: 6,
      wins: 4,
      draws: 1,
      losses: 1,
      scorePercent: 75,
      lastPlayedAtMs: NOW - DAY,
    },
  ],
  activity: [
    { dayStartMs: NOW - 2 * DAY, games: 3, scorePoints: 2 },
    { dayStartMs: NOW - DAY, games: 4, scorePoints: 3.5 },
  ],
  activityBucket: "day",
  ratingSeries: [
    { atMs: NOW - DAY, rating: 2300 },
    { atMs: NOW, rating: 2348 },
  ],
  openings: [{ name: "Sicilian Defence", games: 9, scorePercent: 61 }],
  accounts: [{ id: "queenbot", username: "QueenBot" }],
  engines: [{ id: "engine-1", name: "Queen" }],
  imported: 4,
  recorded: 30,
  lab: null,
};

const lab: ScorebookLab = {
  telemetryGames: 28,
  thrownWins: [
    {
      id: "abcd1234",
      opponent: "GrinderBot",
      opponentRating: 2280,
      finishedAtMs: NOW - DAY,
      peakEvalCp: 340,
      result: "loss",
      engineName: "Queen",
    },
  ],
  steals: [
    {
      id: "wxyz9876",
      opponent: "SharpBot",
      opponentRating: 2310,
      finishedAtMs: NOW - 3 * DAY,
      peakEvalCp: -410,
      result: "draw",
      engineName: "Queen",
    },
  ],
  conversionRate: 87.5,
  defenseRate: 33.3,
  avgBlundersPerGame: 0.42,
  byEngineLab: [
    {
      engineId: "engine-1",
      engineName: "Queen",
      games: 28,
      avgDepth: 21.4,
      avgBlunders: 0.42,
      conversionRate: 87.5,
      avgMoveTimeMs: 1840,
    },
  ],
  depthByPerf: [{ perf: "blitz", games: 28, avgDepth: 21.4, minDepth: 9 }],
  flaggedWinning: 1,
  avgEndClockMs: 42_000,
  book: {
    gamesWithBook: 20,
    gamesWithout: 8,
    scoreWith: 55,
    scoreWithout: 70,
    avgBookPlies: 8.2,
    avgExitEvalCp: -35,
  },
  reliability: {
    engineRestarts: 2,
    submissionRetries: 5,
    streamReconnects: 1,
    flagSafetyStops: 3,
    failureResigns: 1,
  },
  byConfig: [
    {
      fingerprint: "a1b2c3d4e5f6a7b8",
      engineName: "Queen",
      games: 16,
      scorePercent: 71,
      firstSeenMs: NOW - 20 * DAY,
      lastSeenMs: NOW - 2 * DAY,
    },
    {
      fingerprint: "ffee00112233aabb",
      engineName: "Queen",
      games: 12,
      scorePercent: 62,
      firstSeenMs: NOW - 40 * DAY,
      lastSeenMs: NOW - 21 * DAY,
    },
  ],
};

const labStats: ScorebookStats = { ...stats, lab };

// Local midnight keeps date-input string assertions timezone-safe.
const BASE = new Date(2026, 5, 1).getTime();

const brushStats: ScorebookStats = {
  ...stats,
  activity: Array.from({ length: 10 }, (_, index) => ({
    dayStartMs: BASE + index * DAY,
    games: 2,
    scorePoints: 1,
  })),
  activityBucket: "day",
};

/** Mirrors the page's yyyy-mm-dd date-input formatting (local calendar). */
function dateValue(ms: number) {
  const date = new Date(ms);
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${date.getFullYear()}-${month}-${day}`;
}

// jsdom lacks PointerEvent; a MouseEvent with the pointer type name still
// carries clientX and triggers React's onPointer* handlers.
function brushPointer(target: Element, type: string, clientX: number) {
  fireEvent(
    target,
    new MouseEvent(type, { bubbles: true, cancelable: true, clientX }),
  );
}

const emptyStats: ScorebookStats = {
  ...stats,
  totalGames: 0,
  wins: 0,
  draws: 0,
  losses: 0,
  scorePercent: 0,
  streak: { kind: "none", length: 0 },
  avgOpponentRating: null,
  performanceRating: null,
  byEngine: [],
  byColor: [],
  byPerf: [],
  byOpponentBand: [],
  byTermination: [],
  timeLosses: 0,
  topOpponents: [],
  activity: [],
  ratingSeries: [],
  openings: [],
  imported: 0,
  recorded: 0,
};

function mockStats(payload: ScorebookStats) {
  vi.mocked(invoke).mockImplementation((command: string) =>
    command === "get_scorebook_stats"
      ? Promise.resolve(payload)
      : Promise.resolve({ imported: 12, skipped: 3, scanned: 15 }),
  );
}

function renderPage() {
  const showNotice = vi.fn();
  const runAction: RunAction = async (_key, action) => {
    try {
      await action();
      return true;
    } catch {
      showNotice("error", "action failed");
      return false;
    }
  };
  render(
    <ScorebookPage
      busy={new Set<string>()}
      runAction={runAction}
      showNotice={showNotice}
    />,
  );
  return { showNotice };
}

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  mockStats(stats);
});

afterEach(cleanup);

describe("ScorebookPage", () => {
  it("renders KPIs, charts, and the engine table from the stats command", async () => {
    renderPage();

    expect(
      await screen.findByRole("heading", { name: "Scorebook" }),
    ).toBeInTheDocument();
    expect(vi.mocked(invoke)).toHaveBeenCalledWith("get_scorebook_stats", {
      filter: {
        accountId: null,
        engineId: null,
        perf: null,
        fromMs: null,
        toMs: null,
      },
    });

    const kpis = screen.getByLabelText("Scorebook summary");
    expect(within(kpis).getByText("34")).toBeInTheDocument();
    expect(within(kpis).getByText("67.6%")).toBeInTheDocument();
    expect(within(kpis).getByText("W5")).toBeInTheDocument();
    expect(within(kpis).getByText("2215")).toBeInTheDocument();
    expect(within(kpis).getByText("2348")).toBeInTheDocument();
    const record = within(kpis).getByText("Record").nextElementSibling;
    expect(record).toHaveTextContent("20–6–8");

    expect(
      screen.getByRole("heading", { name: "Activity" }),
    ).toBeInTheDocument();
    expect(screen.getByText("2000–2199")).toBeInTheDocument();
    expect(screen.getByText("Mate")).toBeInTheDocument();
    expect(
      screen.getByText(/2 losses on time — check engine time management/),
    ).toBeInTheDocument();
    expect(screen.getByText("White")).toBeInTheDocument();
    expect(screen.getByText("Black")).toBeInTheDocument();

    const table = screen.getByRole("table", { name: "Results by engine" });
    expect(within(table).getByText("Queen")).toBeInTheDocument();
    const unknown = within(table).getByText("Imported / unknown");
    expect(unknown.closest(".scorebook-table-row")).toHaveClass(
      "scorebook-muted",
    );
    expect(within(table).getByText("71.7%")).toBeInTheDocument();

    expect(screen.getByText("RivalBot")).toBeInTheDocument();
    expect(screen.getByText("Sicilian Defence")).toBeInTheDocument();
  });

  it("refetches with the chosen filter", async () => {
    const user = userEvent.setup();
    renderPage();
    await screen.findByRole("heading", { name: "Scorebook" });

    await user.selectOptions(screen.getByLabelText("Filter by speed"), "blitz");
    await waitFor(() =>
      expect(vi.mocked(invoke)).toHaveBeenCalledWith("get_scorebook_stats", {
        filter: {
          accountId: null,
          engineId: null,
          perf: "blitz",
          fromMs: null,
          toMs: null,
        },
      }),
    );

    await user.selectOptions(
      screen.getByLabelText("Filter by account"),
      "queenbot",
    );
    await waitFor(() =>
      expect(vi.mocked(invoke)).toHaveBeenCalledWith("get_scorebook_stats", {
        filter: {
          accountId: "queenbot",
          engineId: null,
          perf: "blitz",
          fromMs: null,
          toMs: null,
        },
      }),
    );
  });

  it("shows the empty state with an import call to action", async () => {
    mockStats(emptyStats);
    renderPage();

    expect(
      await screen.findByRole("heading", { name: "Your scorebook is empty" }),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/Finished games are recorded automatically/),
    ).toBeInTheDocument();
    expect(
      screen.getAllByRole("button", { name: "Import from Lichess" }).length,
    ).toBeGreaterThanOrEqual(1);
  });

  it("imports Lichess history for the selected account and reports the count", async () => {
    const user = userEvent.setup();
    const { showNotice } = renderPage();
    await screen.findByRole("heading", { name: "Scorebook" });

    await user.click(
      screen.getByRole("button", { name: "Import from Lichess" }),
    );

    await waitFor(() =>
      expect(vi.mocked(invoke)).toHaveBeenCalledWith(
        "import_lichess_history",
        expect.objectContaining({ accountId: "queenbot" }),
      ),
    );
    await waitFor(() =>
      expect(showNotice).toHaveBeenCalledWith(
        "success",
        expect.stringContaining("Imported 12 games"),
      ),
    );
    // A successful import refetches the stats.
    const statsCalls = vi
      .mocked(invoke)
      .mock.calls.filter(([command]) => command === "get_scorebook_stats");
    expect(statsCalls.length).toBeGreaterThanOrEqual(2);
  });

  it("renders the engine lab when telemetry is present", async () => {
    mockStats(labStats);
    renderPage();

    expect(
      await screen.findByRole("heading", { name: "What Lichess can’t see" }),
    ).toBeInTheDocument();

    const kpis = screen.getByLabelText("Engine lab summary");
    expect(within(kpis).getByText("28")).toBeInTheDocument();
    expect(within(kpis).getByText("87.5%")).toBeInTheDocument();
    expect(within(kpis).getByText("33.3%")).toBeInTheDocument();
    expect(within(kpis).getByText("0.42")).toBeInTheDocument();
    expect(within(kpis).getByText("00:42")).toBeInTheDocument();
    const flagged =
      within(kpis).getByText("Flagged winning").nextElementSibling;
    expect(flagged).toHaveTextContent("1");
    expect(flagged).toHaveClass("lab-stat-claret");

    const thrown = screen
      .getByRole("heading", { name: "Thrown wins" })
      .closest("section") as HTMLElement;
    expect(within(thrown).getByText("GrinderBot")).toBeInTheDocument();
    expect(within(thrown).getByText("+3.4")).toBeInTheDocument();
    expect(within(thrown).getByText("Loss")).toBeInTheDocument();
    const escapes = screen
      .getByRole("heading", { name: "Great escapes" })
      .closest("section") as HTMLElement;
    expect(within(escapes).getByText("SharpBot")).toBeInTheDocument();
    expect(within(escapes).getByText("−4.1")).toBeInTheDocument();

    const depth = screen.getByRole("table", { name: "Search depth by speed" });
    expect(within(depth).getByText("Blitz")).toBeInTheDocument();
    expect(within(depth).getByText("21.4")).toBeInTheDocument();

    // Book scores 55% with vs 70% without: the underperformance hint shows.
    expect(
      screen.getByText(/Book lines may be underperforming/),
    ).toBeInTheDocument();

    const cohorts = screen.getByRole("table", { name: "Config cohorts" });
    expect(within(cohorts).getByText("a1b2c3d4e5f6")).toBeInTheDocument();

    const reliability = screen.getByLabelText("Reliability");
    const restarts =
      within(reliability).getByText("Engine restarts").nextElementSibling;
    expect(restarts).toHaveTextContent("2");
    expect(restarts).toHaveClass("lab-stat-brass");
    expect(
      within(reliability).getByText("Submission retries").nextElementSibling,
    ).toHaveTextContent("5");
    const safetyStops =
      within(reliability).getByText("Flag-safety stops").nextElementSibling;
    expect(safetyStops).toHaveTextContent("3");
    expect(safetyStops).toHaveClass("lab-stat-brass");
    const resigns =
      within(reliability).getByText("Failure resigns").nextElementSibling;
    expect(resigns).toHaveTextContent("1");
    expect(resigns).toHaveClass("lab-stat-claret");

    const internals = screen.getByRole("table", { name: "Engine internals" });
    expect(within(internals).getByText("Queen")).toBeInTheDocument();
    expect(within(internals).getByText("1.8s")).toBeInTheDocument();
  });

  it("shows the quiet lab hint when telemetry is absent", async () => {
    mockStats({ ...stats, lab: null });
    renderPage();

    expect(
      await screen.findByRole("heading", { name: "What Lichess can’t see" }),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        /The Engine lab fills in as your bots play games through QueenUI/,
      ),
    ).toBeInTheDocument();
    expect(screen.queryByLabelText("Engine lab summary")).toBeNull();
  });

  it("opens a lab game on Lichess when its id chip is clicked", async () => {
    mockStats(labStats);
    const user = userEvent.setup();
    renderPage();
    await screen.findByRole("heading", { name: "What Lichess can’t see" });

    await user.click(screen.getByRole("button", { name: "abcd1234" }));

    expect(vi.mocked(openUrl)).toHaveBeenCalledWith(
      "https://lichess.org/abcd1234",
    );
  });

  it("reports an opener that refuses the game link", async () => {
    mockStats(labStats);
    vi.mocked(openUrl).mockRejectedValueOnce(new Error("no browser"));
    const user = userEvent.setup();
    const { showNotice } = renderPage();
    await screen.findByRole("heading", { name: "What Lichess can’t see" });

    await user.click(screen.getByRole("button", { name: "abcd1234" }));

    // Swallowing this left a chip that quietly did nothing.
    await waitFor(() =>
      expect(showNotice).toHaveBeenCalledWith(
        "error",
        expect.stringContaining("no browser"),
      ),
    );
  });

  it("merges the brushed time range into the stats filter and shows the chip", async () => {
    mockStats(brushStats);
    const user = userEvent.setup();
    renderPage();
    await screen.findByRole("heading", { name: "Activity" });

    // Fallback width 640 / 10 buckets = 64px per slot: drag buckets 1–4.
    const svg = screen.getByRole("img", {
      name: /drag to select a time range/,
    });
    brushPointer(svg, "pointerdown", 100);
    brushPointer(svg, "pointermove", 300);
    brushPointer(svg, "pointerup", 300);

    await waitFor(() =>
      expect(vi.mocked(invoke)).toHaveBeenCalledWith("get_scorebook_stats", {
        filter: {
          accountId: null,
          engineId: null,
          perf: null,
          fromMs: BASE + DAY,
          toMs: BASE + 5 * DAY - 1,
        },
      }),
    );
    // The range chip counts the games inside the four selected buckets.
    expect(screen.getByText(/· 8 games/)).toBeInTheDocument();

    await user.click(
      screen.getByRole("button", { name: "Clear time selection" }),
    );
    await waitFor(() => expect(screen.queryByText(/· 8 games/)).toBeNull());
    const statsCalls = vi
      .mocked(invoke)
      .mock.calls.filter(([command]) => command === "get_scorebook_stats");
    const lastFilter = statsCalls[statsCalls.length - 1][1] as {
      filter: { fromMs: number | null };
    };
    expect(lastFilter.filter.fromMs).toBeNull();
  });

  it("keeps the date inputs in sync with the brush selection", async () => {
    mockStats(brushStats);
    const user = userEvent.setup();
    renderPage();
    await screen.findByRole("heading", { name: "Activity" });

    await user.click(screen.getByRole("button", { name: "Filter by date" }));
    const fromInput = screen.getByLabelText("From");
    const toInput = screen.getByLabelText("To");
    expect(fromInput).toHaveValue("");

    // Brushing a single bucket fills both inputs with its calendar days.
    const svg = screen.getByRole("img", {
      name: /drag to select a time range/,
    });
    brushPointer(svg, "pointerdown", 100);
    brushPointer(svg, "pointerup", 100);
    await waitFor(() => expect(fromInput).toHaveValue(dateValue(BASE + DAY)));
    expect(toInput).toHaveValue(dateValue(BASE + 2 * DAY - 1));

    // Editing the inputs drives the filter (inclusive end-of-day for To).
    fireEvent.change(fromInput, { target: { value: dateValue(BASE) } });
    fireEvent.change(toInput, { target: { value: dateValue(BASE + 2 * DAY) } });
    await waitFor(() =>
      expect(vi.mocked(invoke)).toHaveBeenCalledWith("get_scorebook_stats", {
        filter: expect.objectContaining({
          fromMs: BASE,
          toMs: BASE + 3 * DAY - 1,
        }) as unknown,
      }),
    );
  });
});
