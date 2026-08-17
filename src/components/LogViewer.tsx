import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
  type UIEvent,
} from "react";
import {
  logDirection,
  type LogDirection,
  type LogLine,
  type LogPage,
} from "../types";

/** Highlight request echoed into the rows so matches read in place. */
export type LogHighlight = {
  text: string;
  regex: boolean;
  caseSensitive: boolean;
};

export type LogViewerHandle = {
  /** Put `line` inside the rendered window and scroll it into view. */
  scrollToLine: (line: number, align?: "start" | "center") => void;
  /** Jump to the newest line — used when Follow is switched back on. */
  scrollToEnd: () => void;
};

export type LogViewerProps = {
  sessionId: string;
  /**
   * The session summary's line count. It counts every line *written*, while
   * a page counts the lines currently *decodable* from the gzip — the
   * encoder only flushes once per completed move, so during a search the
   * summary runs ahead by the whole in-flight block. It is therefore used
   * only as an estimate until the first page lands.
   */
  totalLines: number;
  fetchPage: (
    sessionId: string,
    offset: number,
    limit: number,
  ) => Promise<LogPage>;
  /**
   * Every successful fetch, so the page above can lift header, live flag,
   * and the authoritative line count. Must be referentially stable.
   */
  onPage?: (page: LogPage) => void;
  /** Poll the tail while true: a live session on a visible tab. */
  tailing?: boolean;
  /** Keep the newest line pinned to the bottom of the viewport. */
  follow?: boolean;
  /** A manual scroll left the tail; the page turns Follow off. */
  onFollowOff?: () => void;
  /** Line the outline or the current search match points at. */
  activeLine?: number | null;
  highlight?: LogHighlight | null;
  label?: string;
  emptyMessage?: string;
  rowHeight?: number;
  overscan?: number;
  pageSize?: number;
  pollMs?: number;
  /** Attempts per page before the viewer stops and reports the failure. */
  maxAttempts?: number;
  /** First backoff step; attempt *n* waits `n * retryMs`. */
  retryMs?: number;
  /** Pages held in memory; the farthest from the window are dropped first. */
  maxCachedPages?: number;
  /**
   * Height assumed before the scroller has been laid out. jsdom and the
   * first paint both report `clientHeight === 0`; without a fallback the
   * window would stay empty until something triggered a resize.
   */
  initialViewportHeight?: number;
};

/** `>` sent, `<` received, `!` engine stderr, `#` QueenUI note. */
const DIRECTION_CLASS: Record<LogDirection, string> = {
  ">": "log-line-sent",
  "<": "log-line-received",
  "!": "log-line-stderr",
  "#": "log-line-note",
};

const DIRECTION_LABEL: Record<LogDirection, string> = {
  ">": "Sent to engine",
  "<": "Received",
  "!": "Engine stderr",
  "#": "QueenUI note",
};

/**
 * A page whose cached slice is shorter than the range it now covers is
 * refetched, but only so many times: a backend that keeps under-filling a
 * page must not put the viewer in a refetch loop.
 */
const MAX_REFILLS_PER_PAGE = 3;

/** `mm:ss.mmm` since the session opened. */
function elapsedText(atMs: number) {
  const safe = Math.max(0, atMs);
  const totalSeconds = Math.floor(safe / 1000);
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  const millis = Math.floor(safe % 1000);
  return `${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}.${String(millis).padStart(3, "0")}`;
}

function escapeRegExp(value: string) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/**
 * A global matcher for the current query, or null when the pattern is empty
 * or still half-typed — an invalid regex must never break the viewer.
 */
function highlightPattern(highlight: LogHighlight | null | undefined) {
  if (!highlight?.text) return null;
  const source = highlight.regex
    ? highlight.text
    : escapeRegExp(highlight.text);
  try {
    return new RegExp(source, highlight.caseSensitive ? "g" : "gi");
  } catch {
    return null;
  }
}

/** Cap per line: a pathological pattern must not explode one row. */
const MAX_HITS_PER_LINE = 24;

function highlightedText(text: string, pattern: RegExp | null): ReactNode {
  if (!pattern) return text;
  pattern.lastIndex = 0;
  const nodes: ReactNode[] = [];
  let cursor = 0;
  let hits = 0;
  let match = pattern.exec(text);
  while (match && hits < MAX_HITS_PER_LINE) {
    if (match[0].length === 0) {
      pattern.lastIndex += 1;
      match = pattern.exec(text);
      continue;
    }
    if (match.index > cursor) nodes.push(text.slice(cursor, match.index));
    nodes.push(
      <mark className="log-hit" key={match.index}>
        {match[0]}
      </mark>,
    );
    cursor = match.index + match[0].length;
    hits += 1;
    match = pattern.exec(text);
  }
  if (hits === 0) return text;
  if (cursor < text.length) nodes.push(text.slice(cursor));
  return nodes;
}

/** Stable, non-uniform placeholder widths so loading rows read as text. */
function placeholderWidth(index: number) {
  return 34 + ((index * 37) % 52);
}

/**
 * Fixed-row-height windowed log reader.
 *
 * Sessions run to tens of thousands of lines, so only the visible slice plus
 * a small overscan is mounted: rows are absolutely positioned inside a spacer
 * of `total * rowHeight`. Line data arrives page by page from `fetchPage` as
 * the window moves; rows whose page has not landed yet render a placeholder
 * rather than collapsing the layout.
 *
 * `total` is the newest `page.totalLines` — the count the viewer can actually
 * fetch — because the session summary counts lines the gzip has not been
 * flushed for yet. Rendering the summary's count would fill a live pane with
 * permanently unfillable placeholders.
 *
 * `scrollTop` is held in React state as well as on the element so that an
 * imperative jump re-windows immediately, without waiting for a scroll event.
 */
export const LogViewer = forwardRef<LogViewerHandle, LogViewerProps>(
  function LogViewer(
    {
      sessionId,
      totalLines,
      fetchPage,
      onPage,
      tailing = false,
      follow = false,
      onFollowOff,
      activeLine = null,
      highlight = null,
      label = "Engine log",
      emptyMessage = "Nothing has been recorded for this session yet.",
      rowHeight = 22,
      overscan = 12,
      pageSize = 500,
      pollMs = 750,
      maxAttempts = 3,
      retryMs = 600,
      maxCachedPages = 24,
      initialViewportHeight = 720,
    },
    ref,
  ) {
    const scrollRef = useRef<HTMLDivElement | null>(null);
    /** Pages fetched or in flight: the set that must not be asked for again. */
    const requestedRef = useRef<Set<number>>(new Set());
    /** Mirror of the page cache, readable from async callbacks. */
    const cacheRef = useRef<Map<number, LogLine[]>>(new Map());
    const attemptsRef = useRef<Map<number, number>>(new Map());
    const refillsRef = useRef<Map<number, number>>(new Map());
    /** Last applied request sequence per page — drops out-of-order answers. */
    const pageSeqRef = useRef<Map<number, number>>(new Map());
    const failedRef = useRef<Set<number>>(new Set());
    const retryTimersRef = useRef<Set<number>>(new Set());
    const sessionRef = useRef(sessionId);
    const seqRef = useRef(0);
    const totalSeqRef = useRef(0);
    const tailBusyRef = useRef(false);
    const windowPageRef = useRef(0);
    const tailPageRef = useRef(0);

    const [pages, setPages] = useState<ReadonlyMap<number, LogLine[]>>(
      () => new Map(),
    );
    const [scrollTop, setScrollTop] = useState(0);
    const [viewportHeight, setViewportHeight] = useState(initialViewportHeight);
    const [fetchedTotal, setFetchedTotal] = useState<number | null>(null);
    const [failedPages, setFailedPages] = useState<ReadonlySet<number>>(
      () => new Set(),
    );
    /** Longest cached line, in characters: the canvas is at least that wide. */
    const [widestLine, setWidestLine] = useState(0);
    /** Bumped when pages are invalidated, to re-run the fetch pass. */
    const [revision, setRevision] = useState(0);
    const [openSession, setOpenSession] = useState(sessionId);

    // The component owns its per-session contract rather than relying on the
    // parent to remount it: a changed `sessionId` drops every cached page,
    // request marker, and scroll position before the new session renders.
    if (openSession !== sessionId) {
      setOpenSession(sessionId);
      setPages(new Map());
      setFetchedTotal(null);
      setFailedPages(new Set());
      setWidestLine(0);
      setScrollTop(0);
      setRevision(0);
      sessionRef.current = sessionId;
      // Cleared in place: the unmount cleanup holds a reference to the timer
      // set, so replacing it would strand whatever the new session schedules.
      requestedRef.current.clear();
      cacheRef.current.clear();
      attemptsRef.current.clear();
      refillsRef.current.clear();
      pageSeqRef.current.clear();
      failedRef.current.clear();
      for (const timer of retryTimersRef.current) window.clearTimeout(timer);
      retryTimersRef.current.clear();
      totalSeqRef.current = 0;
      tailBusyRef.current = false;
    }

    // The summary count is only an estimate; once a page has landed the
    // viewer renders exactly what it can fetch, in either direction — an
    // interrupted session legitimately decodes to fewer lines than were
    // written.
    const total = fetchedTotal ?? Math.max(0, totalLines);
    const totalRef = useRef(total);
    useEffect(() => {
      totalRef.current = total;
    }, [total]);

    // A page that lands after the window moved on is still correct data, so
    // results are only discarded once the viewer itself is gone, or the
    // session it was fetched for is no longer the open one.
    const aliveRef = useRef(true);
    useEffect(() => {
      aliveRef.current = true;
      const timers = retryTimersRef.current;
      return () => {
        aliveRef.current = false;
        for (const timer of timers) window.clearTimeout(timer);
        timers.clear();
      };
    }, []);

    useLayoutEffect(() => {
      if (scrollRef.current) scrollRef.current.scrollTop = 0;
    }, [sessionId]);

    // Measure the scroller when the browser can; jsdom has no ResizeObserver
    // and reports a zero height, so the initial estimate simply stands.
    useLayoutEffect(() => {
      const element = scrollRef.current;
      if (!element) return;
      const measure = () => {
        if (element.clientHeight > 0) setViewportHeight(element.clientHeight);
      };
      measure();
      if (typeof ResizeObserver === "undefined") return;
      const observer = new ResizeObserver(measure);
      observer.observe(element);
      return () => observer.disconnect();
    }, []);

    const visibleCount = Math.max(1, Math.ceil(viewportHeight / rowHeight));
    const firstVisible = Math.min(
      Math.max(0, Math.floor(scrollTop / rowHeight)),
      Math.max(0, total - 1),
    );
    const start = Math.max(0, firstVisible - overscan);
    const end = Math.min(total, firstVisible + visibleCount + overscan);

    useEffect(() => {
      windowPageRef.current = Math.floor(start / pageSize);
      tailPageRef.current = Math.floor(Math.max(0, total - 1) / pageSize);
    }, [start, total, pageSize]);

    /** Drop the pages farthest from the window once the cache is over budget. */
    const evictDistantPages = useCallback(() => {
      const cache = cacheRef.current;
      if (cache.size <= maxCachedPages) return;
      const anchor = windowPageRef.current;
      const ordered = Array.from(cache.keys()).sort(
        (left, right) =>
          Math.abs(right - anchor) - Math.abs(left - anchor) || right - left,
      );
      for (const pageIndex of ordered) {
        if (cache.size <= maxCachedPages) break;
        if (Math.abs(pageIndex - anchor) <= 1) continue;
        if (pageIndex === tailPageRef.current) continue;
        cache.delete(pageIndex);
        requestedRef.current.delete(pageIndex);
        pageSeqRef.current.delete(pageIndex);
        attemptsRef.current.delete(pageIndex);
      }
    }, [maxCachedPages]);

    /**
     * A page is fetched for the range it covered *then*. A tail poll only
     * revisits the page holding the newest line, so once the session grows
     * past a boundary the page below it stays partially filled and its tail
     * renders as placeholders for the life of the viewer. Every cached page
     * that no longer covers its whole range is marked unfetched, and the
     * windowing pass picks it up again.
     */
    const invalidatePartialPages = useCallback(
      (currentTotal: number) => {
        let changed = false;
        for (const [pageIndex, lines] of cacheRef.current) {
          const covered = Math.min(currentTotal, (pageIndex + 1) * pageSize);
          if (pageIndex * pageSize + lines.length >= covered) continue;
          const refills = refillsRef.current.get(pageIndex) ?? 0;
          if (refills >= MAX_REFILLS_PER_PAGE) continue;
          refillsRef.current.set(pageIndex, refills + 1);
          if (requestedRef.current.delete(pageIndex)) changed = true;
        }
        if (changed) setRevision((value) => value + 1);
      },
      [pageSize],
    );

    const applyPage = useCallback(
      (pageIndex: number, page: LogPage, seq: number) => {
        if (failedRef.current.delete(pageIndex)) {
          setFailedPages(new Set(failedRef.current));
        }
        attemptsRef.current.delete(pageIndex);

        // A slow answer that arrives after a newer one must not rewind the
        // page — or, below, the tail.
        if (seq > (pageSeqRef.current.get(pageIndex) ?? 0)) {
          pageSeqRef.current.set(pageIndex, seq);
          cacheRef.current.set(pageIndex, page.lines);
          evictDistantPages();
          setPages(new Map(cacheRef.current));
          let widest = 0;
          for (const line of page.lines) {
            if (line.text.length > widest) widest = line.text.length;
          }
          setWidestLine((current) => (widest > current ? widest : current));
        }
        if (seq > totalSeqRef.current) {
          totalSeqRef.current = seq;
          setFetchedTotal(page.totalLines);
          invalidatePartialPages(page.totalLines);
          onPage?.(page);
        }
      },
      [evictDistantPages, invalidatePartialPages, onPage],
    );

    /**
     * A failed page is retried a bounded number of times and then left in
     * `requestedRef`, so scrolling cannot re-issue a call that keeps failing.
     * The banner above the canvas offers the only way back.
     */
    const failPage = useCallback(
      (pageIndex: number) => {
        const attempts = (attemptsRef.current.get(pageIndex) ?? 0) + 1;
        attemptsRef.current.set(pageIndex, attempts);
        if (attempts >= maxAttempts) {
          failedRef.current.add(pageIndex);
          setFailedPages(new Set(failedRef.current));
          return;
        }
        const timer = window.setTimeout(() => {
          retryTimersRef.current.delete(timer);
          if (!aliveRef.current) return;
          requestedRef.current.delete(pageIndex);
          setRevision((value) => value + 1);
        }, retryMs * attempts);
        retryTimersRef.current.add(timer);
      },
      [maxAttempts, retryMs],
    );

    const requestPage = useCallback(
      (pageIndex: number) => {
        const forSession = sessionId;
        const seq = (seqRef.current += 1);
        return fetchPage(forSession, pageIndex * pageSize, pageSize)
          .then((page) => {
            if (!aliveRef.current || sessionRef.current !== forSession) return;
            applyPage(pageIndex, page, seq);
          })
          .catch((error) => {
            console.error("get_log_page failed:", error);
            if (!aliveRef.current || sessionRef.current !== forSession) return;
            failPage(pageIndex);
          });
      },
      [sessionId, fetchPage, pageSize, applyPage, failPage],
    );

    const retryFailedPages = useCallback(() => {
      for (const pageIndex of failedRef.current) {
        requestedRef.current.delete(pageIndex);
        attemptsRef.current.delete(pageIndex);
      }
      failedRef.current.clear();
      setFailedPages(new Set());
      setRevision((value) => value + 1);
    }, []);

    // Pull every page the window touches; `requestedRef` keeps a page from
    // being fetched twice while its promise is in flight.
    useEffect(() => {
      if (end <= start) return;
      const firstPage = Math.floor(start / pageSize);
      const lastPage = Math.floor((end - 1) / pageSize);
      for (let pageIndex = firstPage; pageIndex <= lastPage; pageIndex += 1) {
        if (requestedRef.current.has(pageIndex)) continue;
        requestedRef.current.add(pageIndex);
        void requestPage(pageIndex);
      }
    }, [start, end, pageSize, requestPage, revision]);

    // Live tail: re-read the last page on an interval so growth shows up.
    // One poll at a time — an overlapping slow answer would rewind the tail.
    useEffect(() => {
      if (!tailing) return;
      const poll = () => {
        if (tailBusyRef.current) return;
        tailBusyRef.current = true;
        const pageIndex = Math.floor(
          Math.max(0, totalRef.current - 1) / pageSize,
        );
        const forSession = sessionId;
        const seq = (seqRef.current += 1);
        void fetchPage(forSession, pageIndex * pageSize, pageSize)
          .then((page) => {
            if (!aliveRef.current || sessionRef.current !== forSession) return;
            requestedRef.current.add(pageIndex);
            applyPage(pageIndex, page, seq);
          })
          .catch((error) => {
            console.error("get_log_page failed:", error);
            if (!aliveRef.current || sessionRef.current !== forSession) return;
            failPage(pageIndex);
          })
          .finally(() => {
            tailBusyRef.current = false;
          });
      };
      const timer = window.setInterval(poll, pollMs);
      return () => window.clearInterval(timer);
    }, [tailing, pollMs, sessionId, pageSize, fetchPage, applyPage, failPage]);

    const maxScrollTop = Math.max(0, total * rowHeight - viewportHeight);

    // Follow pins the tail; it re-runs whenever the session grows.
    useEffect(() => {
      if (!follow) return;
      const next = Math.max(0, total * rowHeight - viewportHeight);
      if (scrollRef.current) scrollRef.current.scrollTop = next;
      setScrollTop(next);
    }, [follow, total, rowHeight, viewportHeight]);

    useImperativeHandle(
      ref,
      () => ({
        scrollToLine(line, align = "start") {
          const target =
            align === "center"
              ? line * rowHeight - viewportHeight / 2 + rowHeight / 2
              : line * rowHeight - rowHeight * 2;
          const next = Math.min(maxScrollTop, Math.max(0, target));
          if (scrollRef.current) scrollRef.current.scrollTop = next;
          setScrollTop(next);
        },
        scrollToEnd() {
          if (scrollRef.current) scrollRef.current.scrollTop = maxScrollTop;
          setScrollTop(maxScrollTop);
        },
      }),
      [maxScrollTop, rowHeight, viewportHeight],
    );

    function handleScroll(event: UIEvent<HTMLDivElement>) {
      const next = event.currentTarget.scrollTop;
      setScrollTop(next);
      if (follow && maxScrollTop - next > rowHeight * 2) onFollowOff?.();
    }

    const pattern = useMemo(() => highlightPattern(highlight), [highlight]);

    const rows: ReactNode[] = [];
    for (let index = start; index < end; index += 1) {
      const pageIndex = Math.floor(index / pageSize);
      const line = pages.get(pageIndex)?.[index % pageSize];
      const style = { top: index * rowHeight, height: rowHeight };
      if (!line) {
        const unavailable = failedPages.has(pageIndex);
        rows.push(
          /*
           * The list is windowed: about thirty rows exist for a session of
           * twenty thousand lines. Without setsize/posinset assistive
           * technology announces "1 of 30" and the reader has no idea where
           * in the transcript they are.
           */
          <div
            className={`log-line log-line-pending${unavailable ? " log-line-unavailable" : ""}`}
            role="listitem"
            aria-label={`Line ${index + 1} ${unavailable ? "unavailable" : "loading"}`}
            aria-setsize={total}
            aria-posinset={index + 1}
            style={style}
            key={index}
          >
            <span className="log-line-time" />
            <span className="log-line-dir" />
            <span className="log-line-text">
              <i style={{ width: `${placeholderWidth(index)}%` }} />
            </span>
          </div>,
        );
        continue;
      }
      // The wire carries the marker as a bare `string`; narrow once per row
      // so the two lookup tables stay total.
      const direction = logDirection(line.direction);
      rows.push(
        <div
          className={`log-line ${DIRECTION_CLASS[direction]}${
            index === activeLine ? " log-line-active" : ""
          }`}
          role="listitem"
          aria-setsize={total}
          aria-posinset={index + 1}
          style={style}
          data-line={index}
          key={index}
        >
          <span className="log-line-time">{elapsedText(line.atMs)}</span>
          <span className="log-line-dir" title={DIRECTION_LABEL[direction]}>
            {line.direction}
          </span>
          {/* The canvas is widened to the longest cached line and the
              scroller pans horizontally, so nothing is ellipsised away; the
              title keeps a line readable without panning. */}
          <span className="log-line-text" title={line.text}>
            {highlightedText(line.text, pattern)}
          </span>
        </div>,
      );
    }

    return (
      <>
        {failedPages.size > 0 && (
          <div className="log-load-error" role="alert">
            <span>
              {failedPages.size === 1
                ? "A block of lines could not be read from this session."
                : `${failedPages.size} blocks of lines could not be read from this session.`}
            </span>
            <button type="button" onClick={retryFailedPages}>
              Retry
            </button>
          </div>
        )}
        <div
          className="log-scroll"
          ref={scrollRef}
          onScroll={handleScroll}
          role="region"
          aria-label={label}
          tabIndex={0}
        >
          {total === 0 ? (
            <p className="log-empty">{emptyMessage}</p>
          ) : (
            <div
              className="log-canvas"
              role="list"
              style={{
                height: total * rowHeight,
                minWidth:
                  widestLine > 0
                    ? `calc(var(--log-gutter) + ${widestLine + 2}ch)`
                    : undefined,
              }}
            >
              {rows}
            </div>
          )}
        </div>
      </>
    );
  },
);
