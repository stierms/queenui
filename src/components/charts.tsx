import {
  useEffect,
  useId,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
  type RefObject,
} from "react";
import { bucketEndMs } from "../lib/buckets";
import { MONTHS, MONTHS_FULL, monthDay } from "../lib/format";
import type { ActivityBucket, DayLine } from "../types";

/**
 * Hand-rolled SVG chart primitives for the Scorebook page.
 *
 * House dataviz rules: win = moss, draw = --chart-draw, loss = claret,
 * single-series marks = bone; grid lines recessive (--line-1); every text
 * element wears text tokens (11px mono uppercase for axes) — identity is
 * never carried by color alone.
 */

const FALLBACK_WIDTH = 640;

/**
 * Width-responsive charts without a chart library: measure the wrapping
 * element with a ResizeObserver and lay marks out in real pixels. Falls
 * back to a fixed width where ResizeObserver is unavailable (jsdom).
 */
function useMeasuredWidth(): [RefObject<HTMLDivElement | null>, number] {
  const ref = useRef<HTMLDivElement | null>(null);
  const [width, setWidth] = useState(FALLBACK_WIDTH);
  useEffect(() => {
    const node = ref.current;
    if (!node || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver((entries) => {
      const measured = entries[0]?.contentRect.width;
      if (measured) setWidth(measured);
    });
    observer.observe(node);
    return () => observer.disconnect();
  }, []);
  return [ref, width];
}

/** `useId` output made safe for SVG `url(#…)` references. */
function useClipId(prefix: string) {
  return `${prefix}-${useId().replace(/[^a-zA-Z0-9_-]/g, "")}`;
}

const WEEKDAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

export type StackedRowProps = {
  label: string;
  wins: number;
  draws: number;
  losses: number;
  /** Right-aligned score; when omitted the total game count is shown. */
  scorePercent?: number | null;
};

const STACK_HEIGHT = 12;
const STACK_GAP = 2;

/**
 * Horizontal W/D/L stacked row: 2px gaps rendered by inset, 4px rounded
 * outer ends only (via clip), mono count label plus right-aligned score.
 */
export function StackedRow({
  label,
  wins,
  draws,
  losses,
  scorePercent,
}: StackedRowProps) {
  const [ref, width] = useMeasuredWidth();
  const clipId = useClipId("stacked-row");
  const games = wins + draws + losses;
  const parts = [
    { name: "wins", value: wins, className: "seg-win" },
    { name: "draws", value: draws, className: "seg-draw" },
    { name: "losses", value: losses, className: "seg-loss" },
  ].filter((part) => part.value > 0);
  let cursor = 0;
  const inset = STACK_GAP / 2;
  const segments = parts.map((part, index) => {
    const raw = games > 0 ? (part.value / games) * width : 0;
    const leftInset = index > 0 ? inset : 0;
    const rightInset = index < parts.length - 1 ? inset : 0;
    const segment = {
      ...part,
      x: cursor + leftInset,
      width: Math.max(0, raw - leftInset - rightInset),
    };
    cursor += raw;
    return segment;
  });
  return (
    <div className="stacked-row">
      <span className="stacked-row-label">{label}</span>
      <div className="stacked-row-track" ref={ref}>
        <svg
          width="100%"
          height={STACK_HEIGHT}
          role="img"
          aria-label={`${label}: ${wins} wins, ${draws} draws, ${losses} losses`}
        >
          <clipPath id={clipId}>
            <rect x={0} y={0} width={width} height={STACK_HEIGHT} rx={4} />
          </clipPath>
          <rect
            className="stacked-row-bed"
            x={0}
            y={0}
            width={width}
            height={STACK_HEIGHT}
            rx={4}
          />
          <g clipPath={`url(#${clipId})`}>
            {segments.map((segment) => (
              <rect
                key={segment.name}
                className={segment.className}
                x={segment.x}
                y={0}
                width={segment.width}
                height={STACK_HEIGHT}
              />
            ))}
          </g>
        </svg>
      </div>
      <span className="stacked-row-counts">
        {wins}–{draws}–{losses}
      </span>
      <span className="stacked-row-score">
        {scorePercent == null ? `${games} g` : `${Math.round(scorePercent)}%`}
      </span>
    </div>
  );
}

/** A brushed range. UI-only: nothing on the wire carries a selection. */
export type TimeSelection = { fromMs: number; toMs: number };

function bucketTitle(date: Date, bucket: ActivityBucket) {
  if (bucket === "week") return `Week of ${monthDay(date.getTime())}`;
  if (bucket === "month")
    return `${MONTHS_FULL[date.getMonth()]} ${date.getFullYear()}`;
  return `${WEEKDAYS[date.getDay()]} ${date.getDate()}`;
}

/**
 * Sparse x labels: every 7th day, every 4th week, or — for months — the
 * year at each January and the month name at each remaining quarter start.
 */
function tickLabel(date: Date, index: number, bucket: ActivityBucket) {
  if (bucket === "day")
    return index % 7 === 0 ? monthDay(date.getTime()) : null;
  if (bucket === "week")
    return index % 4 === 0 ? monthDay(date.getTime()) : null;
  const month = date.getMonth();
  if (month === 0) return String(date.getFullYear());
  return month % 3 === 0 ? MONTHS[month] : null;
}

export type ActivityBarsProps = {
  /*
   * The generated `DayLine` — the chart draws exactly what
   * `ScorebookStats.activity` carries, so there is no second definition of an
   * activity row to keep in step with the backend.
   */
  days: DayLine[];
  height?: number;
  /** Bucket size the entries represent; drives tooltips, ticks, snapping. */
  bucket?: ActivityBucket;
  /** Committed selection, rendered as overlay + dimmed outside bars. */
  selection?: TimeSelection | null;
  /**
   * Presence makes the chart brushable: pointer-drag selects a contiguous
   * bucket range snapped to bucket boundaries; double-click clears (null).
   */
  onSelect?: (selection: TimeSelection | null) => void;
};

/**
 * Thin vertical bars (bone at 85% opacity, 4px rounded tops) with native
 * SVG title tooltips per bucket and sparse x labels. With `onSelect` it
 * doubles as a brushable time-range selector.
 */
export function ActivityBars({
  days,
  height = 132,
  bucket = "day",
  selection = null,
  onSelect,
}: ActivityBarsProps) {
  const [ref, width] = useMeasuredWidth();
  const clipId = useClipId("activity");
  const [drag, setDrag] = useState<{ start: number; end: number } | null>(null);
  /** Where the keyboard left the selection: `head` is the end that moves. */
  const [keyCursor, setKeyCursor] = useState({ anchor: 0, head: 0 });
  const axisBand = 18;
  const top = 8;
  const plotHeight = height - axisBand - top;
  const baseline = top + plotHeight;
  const maxGames = Math.max(1, ...days.map((day) => day.games));
  const slot = width / Math.max(1, days.length);
  const barWidth = Math.max(2, Math.min(18, slot - 4));
  const gridValues = [...new Set([maxGames, Math.ceil(maxGames / 2)])];

  const brushable = onSelect != null && days.length > 0;

  const indexAtEvent = (event: {
    clientX: number;
    currentTarget: SVGSVGElement;
  }) => {
    const rect = event.currentTarget.getBoundingClientRect();
    const raw = Math.floor((event.clientX - rect.left) / Math.max(1e-6, slot));
    const index = Number.isFinite(raw) ? raw : 0;
    return Math.max(0, Math.min(days.length - 1, index));
  };

  const handlePointerDown = (event: ReactPointerEvent<SVGSVGElement>) => {
    if (!brushable) return;
    const index = indexAtEvent(event);
    setDrag({ start: index, end: index });
    try {
      event.currentTarget.setPointerCapture?.(event.pointerId);
    } catch {
      // Pointer capture is unavailable in jsdom; dragging still works.
    }
  };

  const handlePointerMove = (event: ReactPointerEvent<SVGSVGElement>) => {
    if (drag == null) return;
    const index = indexAtEvent(event);
    setDrag((current) =>
      current == null ? current : { start: current.start, end: index },
    );
  };

  const handlePointerUp = () => {
    if (drag == null || !onSelect) return;
    const lo = Math.min(drag.start, drag.end);
    const hi = Math.max(drag.start, drag.end);
    setDrag(null);
    // A keyboard adjustment carries on from where the drag finished.
    setKeyCursor({ anchor: drag.start, head: drag.end });
    onSelect({
      fromMs: days[lo].dayStartMs,
      toMs: bucketEndMs(days[hi].dayStartMs, bucket),
    });
  };

  /**
   * The same selection from the keyboard: the arrows move a one-bucket
   * range, Shift+arrow extends it from the anchor, Home/End jump to the
   * ends, and Escape clears. Without this the brush was pointer-only, so a
   * keyboard operator could not narrow the page to a period at all.
   */
  const handleKeyDown = (event: ReactKeyboardEvent<SVGSVGElement>) => {
    if (!brushable || !onSelect) return;
    if (
      event.key === "Escape" ||
      event.key === "Delete" ||
      event.key === "Backspace"
    ) {
      event.preventDefault();
      setKeyCursor({ anchor: 0, head: 0 });
      onSelect(null);
      return;
    }
    const last = days.length - 1;
    const head =
      event.key === "Home"
        ? 0
        : event.key === "End"
          ? last
          : event.key === "ArrowLeft"
            ? keyCursor.head - 1
            : event.key === "ArrowRight"
              ? keyCursor.head + 1
              : null;
    if (head == null) return;
    event.preventDefault();
    const clamped = Math.max(0, Math.min(last, head));
    // The anchor is clamped too: a filter change can shorten `days` under a
    // cursor left behind by an earlier, longer history.
    const anchor = event.shiftKey
      ? Math.max(0, Math.min(last, keyCursor.anchor))
      : clamped;
    setKeyCursor({ anchor, head: clamped });
    onSelect({
      fromMs: days[Math.min(anchor, clamped)].dayStartMs,
      toMs: bucketEndMs(days[Math.max(anchor, clamped)].dayStartMs, bucket),
    });
  };

  // While dragging, preview the drag range; otherwise show the committed
  // selection. Buckets belong to the range when their start falls inside.
  const active: TimeSelection | null =
    drag != null
      ? {
          fromMs: days[Math.min(drag.start, drag.end)].dayStartMs,
          toMs: bucketEndMs(
            days[Math.max(drag.start, drag.end)].dayStartMs,
            bucket,
          ),
        }
      : selection;
  const inSelection = (ms: number) =>
    active != null && ms >= active.fromMs && ms <= active.toMs;
  let selStart = -1;
  let selEnd = -1;
  days.forEach((day, index) => {
    if (inSelection(day.dayStartMs)) {
      if (selStart < 0) selStart = index;
      selEnd = index;
    }
  });

  const spanText =
    bucket === "day"
      ? `over the last ${days.length} day${days.length === 1 ? "" : "s"}`
      : `over ${days.length} ${bucket}${days.length === 1 ? "" : "s"}`;

  return (
    <div className="chart-frame" ref={ref}>
      <svg
        width="100%"
        height={height}
        role="img"
        aria-label={`Games per ${bucket} ${spanText}${
          brushable
            ? " — drag to select a time range, or use the arrow keys; Shift+arrow extends the range and Escape clears it"
            : ""
        }`}
        className={brushable ? "activity-brush" : undefined}
        tabIndex={brushable ? 0 : undefined}
        onPointerDown={handlePointerDown}
        onPointerMove={handlePointerMove}
        onPointerUp={handlePointerUp}
        onPointerCancel={() => setDrag(null)}
        onKeyDown={brushable ? handleKeyDown : undefined}
        onDoubleClick={brushable ? () => onSelect?.(null) : undefined}
      >
        <clipPath id={clipId}>
          <rect x={0} y={top} width={width} height={plotHeight} />
        </clipPath>
        {gridValues.map((value) => {
          const y = top + (1 - value / maxGames) * plotHeight;
          return (
            <g key={value}>
              <line
                className="chart-grid-line"
                x1={0}
                y1={y}
                x2={width}
                y2={y}
              />
              <text className="chart-axis" x={0} y={y - 3}>
                {value}
              </text>
            </g>
          );
        })}
        <line
          className="chart-grid-line"
          x1={0}
          y1={baseline}
          x2={width}
          y2={baseline}
        />
        {selStart >= 0 && (
          <rect
            className="chart-brush"
            x={selStart * slot}
            y={top}
            width={(selEnd - selStart + 1) * slot}
            height={plotHeight}
            rx={4}
          />
        )}
        {days.map((day, index) => {
          const date = new Date(day.dayStartMs);
          const percent =
            day.games > 0 ? Math.round((day.scorePoints / day.games) * 100) : 0;
          const barHeight = (day.games / maxGames) * plotHeight;
          const dimmed = active != null && !inSelection(day.dayStartMs);
          const tick = tickLabel(date, index, bucket);
          return (
            <g key={day.dayStartMs}>
              <title>
                {`${bucketTitle(date, bucket)} — ${day.games} game${
                  day.games === 1 ? "" : "s"
                } · ${percent}%`}
              </title>
              <rect
                className="chart-hover"
                x={index * slot}
                y={0}
                width={slot}
                height={height}
              />
              {day.games > 0 && (
                <g clipPath={`url(#${clipId})`}>
                  <rect
                    className={`chart-bar${dimmed ? " chart-bar-dim" : ""}`}
                    x={index * slot + (slot - barWidth) / 2}
                    y={baseline - barHeight}
                    width={barWidth}
                    height={barHeight + 4}
                    rx={Math.min(4, barWidth / 2)}
                  />
                </g>
              )}
              {tick != null && (
                <text className="chart-axis" x={index * slot} y={height - 4}>
                  {tick}
                </text>
              )}
            </g>
          );
        })}
      </svg>
    </div>
  );
}

export type RatingPoint = { atMs: number; rating: number };

export type SteppedLineProps = { points: RatingPoint[]; height?: number };

/**
 * Step-after rating line: 2px bone stroke, no fill, recessive grid,
 * endpoint dot with a direct label, invisible hover circles per point.
 */
export function SteppedLine({ points, height = 150 }: SteppedLineProps) {
  const [ref, width] = useMeasuredWidth();
  if (points.length === 0) {
    return <p className="chart-empty">No rating data yet.</p>;
  }
  const pad = { top: 12, bottom: 8, right: 48 };
  const innerWidth = Math.max(1, width - pad.right);
  const innerHeight = Math.max(1, height - pad.top - pad.bottom);
  const minMs = points[0].atMs;
  const maxMs = points[points.length - 1].atMs;
  const spanMs = Math.max(1, maxMs - minMs);
  const ratings = points.map((point) => point.rating);
  const minRating = Math.min(...ratings);
  const maxRating = Math.max(...ratings);
  const spanRating = Math.max(1, maxRating - minRating);
  const x = (ms: number) => ((ms - minMs) / spanMs) * innerWidth;
  const y = (rating: number) =>
    pad.top + (1 - (rating - minRating) / spanRating) * innerHeight;
  const path = points
    .map((point, index) =>
      index === 0
        ? `M ${x(point.atMs).toFixed(1)} ${y(point.rating).toFixed(1)}`
        : `H ${x(point.atMs).toFixed(1)} V ${y(point.rating).toFixed(1)}`,
    )
    .join(" ");
  const last = points[points.length - 1];
  const gridRatings = [...new Set([maxRating, minRating])];
  return (
    <div className="chart-frame" ref={ref}>
      <svg
        width="100%"
        height={height}
        role="img"
        aria-label={`Rating over ${points.length} games, currently ${last.rating}`}
      >
        {gridRatings.map((rating) => (
          <g key={rating}>
            <line
              className="chart-grid-line"
              x1={0}
              y1={y(rating)}
              x2={innerWidth}
              y2={y(rating)}
            />
            <text className="chart-axis" x={0} y={y(rating) - 3}>
              {rating}
            </text>
          </g>
        ))}
        <path className="chart-step" d={path} />
        {points.map((point) => (
          <circle
            key={point.atMs}
            className="chart-hover"
            cx={x(point.atMs)}
            cy={y(point.rating)}
            r={8}
          >
            <title>{`${monthDay(point.atMs)} — ${point.rating}`}</title>
          </circle>
        ))}
        <circle
          className="chart-dot"
          cx={x(last.atMs)}
          cy={y(last.rating)}
          r={3}
        />
        <text
          className="chart-axis chart-final-label"
          x={Math.min(x(last.atMs) + 8, width - 4)}
          y={y(last.rating) + 4}
        >
          {last.rating}
        </text>
      </svg>
    </div>
  );
}

export type ScoreMeterProps = {
  label: string;
  scorePercent: number;
  games: number;
};

const METER_HEIGHT = 10;

/** Labelled single-series meter: bone fill on a recessive bed. */
export function ScoreMeter({ label, scorePercent, games }: ScoreMeterProps) {
  const [ref, width] = useMeasuredWidth();
  const clamped = Math.max(0, Math.min(100, scorePercent));
  return (
    <div className="score-meter">
      <span className="score-meter-label">{label}</span>
      <div className="score-meter-track" ref={ref}>
        <svg
          width="100%"
          height={METER_HEIGHT}
          role="img"
          aria-label={`${label}: ${Math.round(clamped)} percent score over ${games} games`}
        >
          <rect
            className="meter-bed"
            x={0}
            y={0}
            width={width}
            height={METER_HEIGHT}
            rx={4}
          />
          {clamped > 0 && (
            <rect
              className="meter-fill"
              x={0}
              y={0}
              width={(clamped / 100) * width}
              height={METER_HEIGHT}
              rx={4}
            />
          )}
        </svg>
      </div>
      <span className="score-meter-value">{Math.round(clamped)}%</span>
      <span className="score-meter-games">{games} g</span>
    </div>
  );
}
