import { useRef } from "react";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { LogDirection, LogLine, LogPage } from "../types";
import { LogViewer, type LogViewerHandle } from "./LogViewer";

const TOTAL = 20_000;
const ROW_HEIGHT = 22;
/** 30 rows visible; the component adds 12 rows of overscan on each side. */
const VIEWPORT = 660;
const OVERSCAN = 12;

const DIRECTIONS: LogDirection[] = [">", "<", "!", "#"];

function lineAt(index: number): LogLine {
  return {
    index,
    atMs: index * 37,
    direction: DIRECTIONS[index % 4],
    text: `line ${index} info depth ${(index % 40) + 1} score cp 24`,
  };
}

function makeFetchPage(total = TOTAL) {
  return vi.fn(
    (sessionId: string, offset: number, limit: number): Promise<LogPage> =>
      Promise.resolve({
        sessionId,
        totalLines: total,
        offset,
        lines: Array.from(
          { length: Math.max(0, Math.min(limit, total - offset)) },
          (_unused, position) => lineAt(offset + position),
        ),
        header: [{ key: "Engine", value: "Queen 0.42" }],
        live: false,
      }),
  );
}

function renderViewer(
  overrides: Partial<Parameters<typeof LogViewer>[0]> = {},
  total = TOTAL,
) {
  const fetchPage = overrides.fetchPage ?? makeFetchPage(total);
  render(
    <LogViewer
      sessionId="session-1"
      totalLines={total}
      fetchPage={fetchPage}
      rowHeight={ROW_HEIGHT}
      overscan={OVERSCAN}
      initialViewportHeight={VIEWPORT}
      {...overrides}
    />,
  );
  return { fetchPage };
}

function renderedLineIndexes() {
  return Array.from(document.querySelectorAll<HTMLElement>("[data-line]")).map(
    (row) => Number(row.dataset.line),
  );
}

/**
 * Retries chain through a timer, a state update, and a rejected promise, so
 * one long advance does not run them all: step the clock instead.
 */
async function settleTimers(steps = 6, stepMs = 50) {
  for (let step = 0; step < steps; step += 1) {
    await act(() => vi.advanceTimersByTimeAsync(stepMs));
  }
}

/** A page over a total the test can move between calls. */
function pageOver(
  sessionId: string,
  offset: number,
  limit: number,
  total: number,
): LogPage {
  return {
    sessionId,
    totalLines: total,
    offset,
    lines: Array.from(
      { length: Math.max(0, Math.min(limit, total - offset)) },
      (_unused, position) => lineAt(offset + position),
    ),
    header: [],
    live: true,
  };
}

/** `fetchPage` whose promises the test resolves by hand, in any order. */
function deferredFetch() {
  const calls: Array<{
    offset: number;
    resolve: (page: LogPage) => void;
    reject: (error: Error) => void;
  }> = [];
  const fetchPage = vi.fn(
    (_sessionId: string, offset: number) =>
      new Promise<LogPage>((resolve, reject) => {
        calls.push({ offset, resolve, reject });
      }),
  );
  return { fetchPage, calls };
}

function callsFor(fetchPage: { mock: { calls: unknown[][] } }, offset: number) {
  return fetchPage.mock.calls.filter((call) => call[1] === offset).length;
}

function scrollTo(line: number) {
  fireEvent.scroll(screen.getByRole("region", { name: "Engine log" }), {
    target: { scrollTop: line * ROW_HEIGHT },
  });
}

let consoleError: ReturnType<typeof vi.spyOn>;

beforeEach(() => {
  // The viewer reports every failed page; the expected noise is silenced so
  // a real unexpected error still stands out in the run.
  consoleError = vi.spyOn(console, "error").mockImplementation(() => undefined);
});

afterEach(() => {
  consoleError.mockRestore();
  cleanup();
});

describe("LogViewer windowing", () => {
  it("mounts only the visible window of a 20 000-line session", async () => {
    renderViewer();
    await screen.findByText(/^line 0 /);

    const rows = document.querySelectorAll(".log-line");
    // 30 visible rows plus one overscan block below; nothing above line 0.
    expect(rows.length).toBe(30 + OVERSCAN);
    expect(rows.length).toBeLessThan(TOTAL / 100);
    expect(screen.queryByText(/^line 500 /)).not.toBeInTheDocument();
    // The spacer still reserves the full scroll height.
    expect(document.querySelector<HTMLElement>(".log-canvas")).toHaveStyle({
      height: `${TOTAL * ROW_HEIGHT}px`,
    });
  });

  it("renders the slice that matches the scroll offset", async () => {
    renderViewer();
    await screen.findByText(/^line 0 /);

    const region = screen.getByRole("region", { name: "Engine log" });
    fireEvent.scroll(region, { target: { scrollTop: 220 * ROW_HEIGHT } });

    const indexes = renderedLineIndexes();
    expect(indexes[0]).toBe(220 - OVERSCAN);
    expect(indexes[indexes.length - 1]).toBe(220 + 30 + OVERSCAN - 1);
    expect(screen.getByText(/^line 220 /)).toBeInTheDocument();
    expect(screen.queryByText(/^line 0 /)).not.toBeInTheDocument();
  });

  it("fetches the pages a distant scroll lands on", async () => {
    const { fetchPage } = renderViewer();
    await screen.findByText(/^line 0 /);
    expect(fetchPage).toHaveBeenCalledWith("session-1", 0, 500);

    const region = screen.getByRole("region", { name: "Engine log" });
    fireEvent.scroll(region, { target: { scrollTop: 12_000 * ROW_HEIGHT } });

    // The rows are placeholders until the page arrives.
    expect(
      screen.getByLabelText("Line 12001 loading", { exact: true }),
    ).toBeInTheDocument();
    expect(fetchPage).toHaveBeenCalledWith("session-1", 12_000, 500);
    expect(await screen.findByText(/^line 12000 /)).toBeInTheDocument();
  });

  it("scrolls an arbitrary line into the rendered window on demand", async () => {
    function Harness() {
      const ref = useRef<LogViewerHandle>(null);
      return (
        <>
          <LogViewer
            ref={ref}
            sessionId="session-1"
            totalLines={TOTAL}
            fetchPage={makeFetchPage()}
            rowHeight={ROW_HEIGHT}
            overscan={OVERSCAN}
            initialViewportHeight={VIEWPORT}
          />
          <button
            type="button"
            onClick={() => ref.current?.scrollToLine(15_000)}
          >
            jump
          </button>
        </>
      );
    }
    render(<Harness />);
    await screen.findByText(/^line 0 /);

    fireEvent.click(screen.getByRole("button", { name: "jump" }));

    expect(await screen.findByText(/^line 15000 /)).toBeInTheDocument();
    expect(renderedLineIndexes()).toContain(15_000);
    expect(screen.queryByText(/^line 0 /)).not.toBeInTheDocument();
  });

  it("colours each row by direction and marks the active line", async () => {
    renderViewer({ activeLine: 5 });
    await screen.findByText(/^line 0 /);

    const rowFor = (index: number) =>
      document.querySelector(`[data-line="${index}"]`);
    expect(rowFor(0)).toHaveClass("log-line-sent");
    expect(rowFor(1)).toHaveClass("log-line-received");
    expect(rowFor(2)).toHaveClass("log-line-stderr");
    expect(rowFor(3)).toHaveClass("log-line-note");
    expect(rowFor(5)).toHaveClass("log-line-active");
    expect(rowFor(4)).not.toHaveClass("log-line-active");
  });

  it("highlights the matched substring inside each row", async () => {
    renderViewer({
      highlight: { text: "score cp", regex: false, caseSensitive: false },
    });
    await screen.findByText(/^line 0 /);

    const hits = document.querySelectorAll("mark.log-hit");
    expect(hits.length).toBe(30 + OVERSCAN);
    expect(hits[0]).toHaveTextContent("score cp");
  });

  it("ignores an invalid regex instead of throwing", async () => {
    renderViewer({
      highlight: { text: "score (", regex: true, caseSensitive: false },
    });
    await screen.findByText(/^line 0 /);
    expect(document.querySelectorAll("mark.log-hit")).toHaveLength(0);
  });

  it("polls the tail only while tailing is on", async () => {
    vi.useFakeTimers();
    try {
      const fetchPage = makeFetchPage(600);
      render(
        <LogViewer
          sessionId="session-1"
          totalLines={600}
          fetchPage={fetchPage}
          rowHeight={ROW_HEIGHT}
          overscan={OVERSCAN}
          initialViewportHeight={VIEWPORT}
          tailing
          pollMs={750}
        />,
      );
      await act(() => vi.advanceTimersByTimeAsync(0));
      const initialCalls = fetchPage.mock.calls.length;

      await act(() => vi.advanceTimersByTimeAsync(1_600));

      // The tail page (offset 500 covers lines 500–599) is re-read.
      expect(fetchPage.mock.calls.length).toBeGreaterThan(initialCalls);
      expect(fetchPage).toHaveBeenCalledWith("session-1", 500, 500);
    } finally {
      vi.useRealTimers();
    }
  });

  it("keeps a page that lands after the window or the total moved", async () => {
    // A live session grows on every poll, which re-runs the fetch effect.
    // Pages in flight at that moment must still be applied, or the viewer
    // is stuck on placeholders forever.
    let resolvePage: ((page: LogPage) => void) | undefined;
    const fetchPage = vi.fn(
      () =>
        new Promise<LogPage>((resolve) => {
          resolvePage = resolve;
        }),
    );
    const { rerender } = render(
      <LogViewer
        sessionId="session-1"
        totalLines={1_000}
        fetchPage={fetchPage}
        rowHeight={ROW_HEIGHT}
        overscan={OVERSCAN}
        initialViewportHeight={VIEWPORT}
      />,
    );
    expect(screen.getByLabelText("Line 1 loading")).toBeInTheDocument();

    rerender(
      <LogViewer
        sessionId="session-1"
        totalLines={1_040}
        fetchPage={fetchPage}
        rowHeight={ROW_HEIGHT}
        overscan={OVERSCAN}
        initialViewportHeight={VIEWPORT}
      />,
    );
    await act(async () => {
      resolvePage?.({
        sessionId: "session-1",
        totalLines: 1_040,
        offset: 0,
        lines: Array.from({ length: 500 }, (_unused, index) => lineAt(index)),
        header: [],
        live: true,
      });
    });

    expect(screen.getByText(/^line 0 /)).toBeInTheDocument();
    expect(fetchPage).toHaveBeenCalledTimes(1);
  });

  it("shows an empty state when the session has no lines", () => {
    renderViewer({ emptyMessage: "Nothing recorded yet." }, 0);
    expect(screen.getByText("Nothing recorded yet.")).toBeInTheDocument();
    expect(document.querySelectorAll(".log-line")).toHaveLength(0);
  });
});

describe("LogViewer line counts", () => {
  it("renders the count a page reports, not the summary's", async () => {
    // The summary counts every line written; a page counts what the gzip
    // decodes to, and the recorder only flushes once per completed move. On
    // a live session the summary is ahead by the whole in-flight search, so
    // rendering it fills the visible pane with unfillable placeholders.
    renderViewer({ fetchPage: makeFetchPage(600) }, 5_000);
    await screen.findByText(/^line 0 /);

    expect(document.querySelector<HTMLElement>(".log-canvas")).toHaveStyle({
      height: `${600 * ROW_HEIGHT}px`,
    });
    expect(document.querySelectorAll(".log-line-pending")).toHaveLength(0);
  });

  it("follows a page count downwards for an interrupted session", async () => {
    // An interrupted session decodes to fewer lines than were written, so
    // the count must not be latched with a Math.max against the summary.
    const { fetchPage, calls } = deferredFetch();
    renderViewer({ fetchPage, tailing: true, pollMs: 750 }, 4_000);
    await waitFor(() => expect(calls.length).toBeGreaterThan(0));

    await act(async () => {
      calls[0].resolve(pageOver("session-1", 0, 500, 900));
    });
    expect(document.querySelector<HTMLElement>(".log-canvas")).toHaveStyle({
      height: `${900 * ROW_HEIGHT}px`,
    });
  });

  it("ignores a page that lands after a newer one", async () => {
    vi.useFakeTimers();
    try {
      const { fetchPage, calls } = deferredFetch();
      render(
        <LogViewer
          sessionId="session-1"
          totalLines={600}
          fetchPage={fetchPage}
          rowHeight={ROW_HEIGHT}
          overscan={OVERSCAN}
          initialViewportHeight={VIEWPORT}
          tailing
          pollMs={750}
        />,
      );
      // The window fetch, then a tail poll while it is still outstanding.
      await act(() => vi.advanceTimersByTimeAsync(0));
      expect(calls.length).toBe(1);
      await act(() => vi.advanceTimersByTimeAsync(800));
      expect(calls.length).toBe(2);

      // The newer answer lands first; the older one must not rewind the
      // tail to its own, staler snapshot.
      await act(async () => {
        calls[1].resolve(pageOver("session-1", calls[1].offset, 500, 900));
      });
      await act(async () => {
        calls[0].resolve(pageOver("session-1", calls[0].offset, 500, 600));
      });

      expect(document.querySelector<HTMLElement>(".log-canvas")).toHaveStyle({
        height: `${900 * ROW_HEIGHT}px`,
      });
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("LogViewer tailing", () => {
  it("never runs two tail polls at once", async () => {
    vi.useFakeTimers();
    try {
      const { fetchPage, calls } = deferredFetch();
      render(
        <LogViewer
          sessionId="session-1"
          totalLines={600}
          fetchPage={fetchPage}
          rowHeight={ROW_HEIGHT}
          overscan={OVERSCAN}
          initialViewportHeight={VIEWPORT}
          tailing
          pollMs={750}
        />,
      );
      await act(() => vi.advanceTimersByTimeAsync(0));
      const windowCalls = calls.length;

      // Three intervals pass with the first poll still outstanding.
      await act(() => vi.advanceTimersByTimeAsync(2_400));

      expect(callsFor(fetchPage, 500)).toBe(1);
      expect(calls.length).toBe(windowCalls + 1);
    } finally {
      vi.useRealTimers();
    }
  });

  it("asks again for a page the session outgrew", async () => {
    // Page 1 is answered while the file holds 60 lines, so it comes back
    // with ten of the fifty lines its range covers — and the same answer
    // already puts the file at 1 000 lines. The tail poll only ever revisits
    // the page holding the newest line, so without an explicit refetch lines
    // 60–99 stay grey for the rest of the viewer's life.
    const { fetchPage, calls } = deferredFetch();
    render(
      <LogViewer
        sessionId="session-1"
        totalLines={60}
        fetchPage={fetchPage}
        pageSize={50}
        rowHeight={ROW_HEIGHT}
        overscan={0}
        initialViewportHeight={220}
      />,
    );
    await waitFor(() => expect(callsFor(fetchPage, 0)).toBe(1));
    await act(async () => {
      calls[0].resolve(pageOver("session-1", 0, 50, 60));
    });
    await screen.findByText(/^line 0 /);

    scrollTo(55);
    await waitFor(() => expect(callsFor(fetchPage, 50)).toBe(1));
    await act(async () => {
      calls[1].resolve({
        sessionId: "session-1",
        totalLines: 1_000,
        offset: 50,
        lines: Array.from({ length: 10 }, (_unused, index) =>
          lineAt(50 + index),
        ),
        header: [],
        live: true,
      });
    });

    // Page 1 holds 10 of the 50 lines it now covers, so it is asked for
    // again rather than being left marked as fetched.
    expect(screen.getByLabelText("Line 61 loading")).toBeInTheDocument();
    await waitFor(() => expect(callsFor(fetchPage, 50)).toBe(2));
    await act(async () => {
      calls[2].resolve(pageOver("session-1", 50, 50, 1_000));
    });
    expect(await screen.findByText(/^line 60 /)).toBeInTheDocument();
  });
});

describe("LogViewer failures", () => {
  it("reports a failing page and stops re-issuing the call", async () => {
    vi.useFakeTimers();
    try {
      const fetchPage = vi.fn(() =>
        Promise.reject(new Error("gzip stream corrupt")),
      );
      render(
        <LogViewer
          sessionId="session-1"
          totalLines={600}
          fetchPage={fetchPage}
          rowHeight={ROW_HEIGHT}
          overscan={OVERSCAN}
          initialViewportHeight={VIEWPORT}
          maxAttempts={3}
          retryMs={10}
        />,
      );

      // Three bounded attempts, then a visible message rather than a page
      // of grey skeletons.
      await settleTimers();
      expect(fetchPage).toHaveBeenCalledTimes(3);
      expect(screen.getByRole("alert")).toHaveTextContent(
        /could not be read from this session/,
      );
      expect(
        screen.getByLabelText("Line 1 unavailable", { exact: true }),
      ).toBeInTheDocument();

      // Scrolling must not restart the storm.
      scrollTo(6);
      await settleTimers();
      expect(fetchPage).toHaveBeenCalledTimes(3);

      // Retry is the only way back, and it works.
      fireEvent.click(screen.getByRole("button", { name: "Retry" }));
      expect(fetchPage).toHaveBeenCalledTimes(4);
    } finally {
      vi.useRealTimers();
    }
  });

  it("clears the failure once a retry succeeds", async () => {
    vi.useFakeTimers();
    try {
      let failing = true;
      const fetchPage = vi.fn(
        (sessionId: string, offset: number, limit: number) =>
          failing
            ? Promise.reject(new Error("gzip stream corrupt"))
            : Promise.resolve(pageOver(sessionId, offset, limit, 600)),
      );
      render(
        <LogViewer
          sessionId="session-1"
          totalLines={600}
          fetchPage={fetchPage}
          rowHeight={ROW_HEIGHT}
          overscan={OVERSCAN}
          initialViewportHeight={VIEWPORT}
          maxAttempts={2}
          retryMs={10}
        />,
      );
      await settleTimers();
      expect(screen.getByRole("alert")).toBeInTheDocument();

      failing = false;
      fireEvent.click(screen.getByRole("button", { name: "Retry" }));
      await settleTimers(2);

      expect(screen.queryByRole("alert")).not.toBeInTheDocument();
      expect(screen.getByText(/^line 0 /)).toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("LogViewer session contract", () => {
  function labelledFetch() {
    return vi.fn(
      (sessionId: string, offset: number, limit: number): Promise<LogPage> =>
        Promise.resolve({
          sessionId,
          totalLines: 20_000,
          offset,
          lines: Array.from({ length: limit }, (_unused, position) => ({
            ...lineAt(offset + position),
            text: `${sessionId} line ${offset + position}`,
          })),
          header: [],
          live: false,
        }),
    );
  }

  it("resets itself when the session changes, without a remount", async () => {
    const fetchPage = labelledFetch();
    const props = {
      fetchPage,
      totalLines: 20_000,
      rowHeight: ROW_HEIGHT,
      overscan: OVERSCAN,
      initialViewportHeight: VIEWPORT,
    };
    const { rerender } = render(<LogViewer sessionId="alpha" {...props} />);
    await screen.findByText("alpha line 0");
    scrollTo(9_000);
    await screen.findByText("alpha line 9000");

    // Same element, different session: no page, count, or scroll position
    // from the old one may survive.
    rerender(<LogViewer sessionId="beta" {...props} />);
    expect(screen.queryByText(/^alpha /)).not.toBeInTheDocument();
    expect(renderedLineIndexes()[0] ?? 0).toBe(0);

    expect(await screen.findByText("beta line 0")).toBeInTheDocument();
    expect(fetchPage).toHaveBeenCalledWith("beta", 0, 500);
  });

  it("bounds how many pages it keeps", async () => {
    const fetchPage = makeFetchPage();
    renderViewer({ fetchPage, maxCachedPages: 2 });
    await screen.findByText(/^line 0 /);
    for (const line of [4_000, 8_000, 12_000]) {
      scrollTo(line);
      await screen.findByText(new RegExp(`^line ${line} `));
    }

    // The oldest, farthest page has been dropped rather than retained for
    // the life of a 20 000-line read.
    scrollTo(0);
    expect(screen.getByLabelText("Line 1 loading")).toBeInTheDocument();
    await waitFor(() => expect(callsFor(fetchPage, 0)).toBe(2));
  });
});

describe("LogViewer long lines", () => {
  it("makes the whole of a long line reachable", async () => {
    const long = `info depth 34 seldepth 48 multipv 1 score cp 31 nodes 91827364 nps 5102340 hashfull 981 tbhits 0 time 18001 pv ${Array.from(
      { length: 24 },
      (_unused, index) => `e${(index % 8) + 1}f${(index % 8) + 1}`,
    ).join(" ")}`;
    const fetchPage = vi.fn(
      (sessionId: string, offset: number, limit: number): Promise<LogPage> =>
        Promise.resolve({
          sessionId,
          totalLines: 40,
          offset,
          lines: Array.from(
            { length: Math.max(0, Math.min(limit, 40 - offset)) },
            (_unused, position) => ({
              ...lineAt(offset + position),
              text: long,
            }),
          ),
          header: [],
          live: false,
        }),
    );
    renderViewer({ fetchPage }, 40);
    await screen.findAllByText(long);

    // The canvas is widened past the pane so the scroller can pan to the end
    // of the line, and the full text is available without panning at all.
    const canvas = document.querySelector<HTMLElement>(".log-canvas");
    expect(canvas?.style.minWidth).toBe(
      `calc(var(--log-gutter) + ${long.length + 2}ch)`,
    );
    expect(document.querySelector(".log-line-text")).toHaveAttribute(
      "title",
      long,
    );
  });
});
