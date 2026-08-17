import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ActivityBars, ScoreMeter, StackedRow, SteppedLine } from "./charts";

// Without a ResizeObserver (jsdom) the charts lay out at their fallback
// width, which keeps every pixel computation deterministic.
const WIDTH = 640;

afterEach(cleanup);

describe("StackedRow", () => {
  it("renders the label, counts, score and proportional segments", () => {
    const { container } = render(
      <StackedRow
        label="1800–1999"
        wins={6}
        draws={2}
        losses={2}
        scorePercent={70}
      />,
    );

    expect(screen.getByText("1800–1999")).toBeInTheDocument();
    expect(screen.getByText("6–2–2")).toBeInTheDocument();
    expect(screen.getByText("70%")).toBeInTheDocument();
    expect(
      screen.getByRole("img", { name: "1800–1999: 6 wins, 2 draws, 2 losses" }),
    ).toBeInTheDocument();

    const widths = ["seg-win", "seg-draw", "seg-loss"].map((className) => {
      const segment = container.querySelector(`.${className}`);
      expect(segment).not.toBeNull();
      return Number(segment?.getAttribute("width"));
    });
    // 60% / 20% / 20% of the track, inset by the two 2px gaps.
    expect(widths[0]).toBeCloseTo(WIDTH * 0.6 - 1);
    expect(widths[1]).toBeCloseTo(WIDTH * 0.2 - 2);
    expect(widths[2]).toBeCloseTo(WIDTH * 0.2 - 1);
    expect(widths.reduce((sum, width) => sum + width, 0)).toBeCloseTo(
      WIDTH - 4,
    );
  });

  it("shows the game total when no score is given and skips empty segments", () => {
    const { container } = render(
      <StackedRow label="Mate" wins={14} draws={0} losses={4} />,
    );

    expect(screen.getByText("18 g")).toBeInTheDocument();
    expect(container.querySelector(".seg-draw")).toBeNull();
    const win = Number(
      container.querySelector(".seg-win")?.getAttribute("width"),
    );
    const loss = Number(
      container.querySelector(".seg-loss")?.getAttribute("width"),
    );
    expect(win + loss).toBeCloseTo(WIDTH - 2);
  });
});

describe("ActivityBars", () => {
  const day = (offset: number, games: number, scorePoints: number) => ({
    dayStartMs: Date.UTC(2026, 5, 1) + offset * 86_400_000,
    games,
    scorePoints,
  });

  it("renders one bar per played day with a native title tooltip", () => {
    const days = [day(0, 3, 2), day(1, 0, 0), day(2, 4, 3)];
    const { container } = render(<ActivityBars days={days} />);

    expect(
      screen.getByRole("img", { name: "Games per day over the last 3 days" }),
    ).toBeInTheDocument();
    expect(container.querySelectorAll(".chart-bar")).toHaveLength(2);

    const titles = Array.from(container.querySelectorAll("title")).map(
      (node) => node.textContent,
    );
    expect(titles.some((text) => /— 4 games · 75%$/.test(text ?? ""))).toBe(
      true,
    );
    expect(titles.some((text) => /— 0 games · 0%$/.test(text ?? ""))).toBe(
      true,
    );
  });

  it("labels every 7th day and scales the tallest bar to the plot", () => {
    const days = Array.from({ length: 14 }, (_, index) =>
      day(index, index === 5 ? 8 : 2, 1),
    );
    const { container } = render(<ActivityBars days={days} height={132} />);

    const axisTexts = Array.from(
      container.querySelectorAll("text.chart-axis"),
    ).map((node) => node.textContent);
    // Two sparse date labels (day 0 and day 7) plus the y-grid value labels.
    expect(
      axisTexts.filter((text) => /^[A-Z][a-z]{2} \d+$/.test(text ?? "")),
    ).toHaveLength(2);

    const heights = Array.from(container.querySelectorAll(".chart-bar")).map(
      (bar) => Number(bar.getAttribute("height")),
    );
    // Tallest bar fills the plot area (132 - 8 top - 18 axis, +4 clip bleed).
    expect(Math.max(...heights)).toBeCloseTo(132 - 8 - 18 + 4);
  });
});

describe("ActivityBars brush", () => {
  const DAY = 86_400_000;
  const base = Date.UTC(2026, 5, 1);
  const days = Array.from({ length: 10 }, (_, index) => ({
    dayStartMs: base + index * DAY,
    games: 2,
    scorePoints: 1,
  }));

  // jsdom lacks PointerEvent; a MouseEvent with the pointer type name
  // carries clientX and triggers React's onPointer* handlers just the same.
  function pointer(target: Element, type: string, clientX: number) {
    fireEvent(
      target,
      new MouseEvent(type, { bubbles: true, cancelable: true, clientX }),
    );
  }

  function getBrushSvg() {
    return screen.getByRole("img", { name: /drag to select a time range/ });
  }

  it("selects a bucket range snapped to bucket boundaries by dragging", () => {
    const onSelect = vi.fn();
    render(
      <ActivityBars
        days={days}
        bucket="day"
        selection={null}
        onSelect={onSelect}
      />,
    );

    // Fallback width 640 / 10 buckets = 64px per slot.
    const svg = getBrushSvg();
    pointer(svg, "pointerdown", 100); // bucket index 1
    pointer(svg, "pointermove", 300); // bucket index 4
    pointer(svg, "pointerup", 300);

    expect(onSelect).toHaveBeenCalledTimes(1);
    expect(onSelect).toHaveBeenCalledWith({
      fromMs: base + DAY,
      toMs: base + 5 * DAY - 1,
    });
  });

  it("snaps week buckets to their full seven days", () => {
    const weeks = Array.from({ length: 8 }, (_, index) => ({
      dayStartMs: base + index * 7 * DAY,
      games: 3,
      scorePoints: 2,
    }));
    const onSelect = vi.fn();
    render(
      <ActivityBars
        days={weeks}
        bucket="week"
        selection={null}
        onSelect={onSelect}
      />,
    );

    const svg = getBrushSvg();
    pointer(svg, "pointerdown", 10);
    pointer(svg, "pointerup", 10);

    expect(onSelect).toHaveBeenCalledWith({
      fromMs: base,
      toMs: base + 7 * DAY - 1,
    });
  });

  it("overlays the selection and dims the bars outside it", () => {
    const { container } = render(
      <ActivityBars
        days={days}
        bucket="day"
        selection={{ fromMs: base + 2 * DAY, toMs: base + 5 * DAY - 1 }}
        onSelect={() => undefined}
      />,
    );

    expect(container.querySelector(".chart-brush")).not.toBeNull();
    // Buckets 2–4 are selected out of 10 → the other 7 bars are dimmed.
    expect(container.querySelectorAll(".chart-bar")).toHaveLength(10);
    expect(container.querySelectorAll(".chart-bar-dim")).toHaveLength(7);
  });

  it("renders undimmed without an overlay when nothing is selected", () => {
    const { container } = render(
      <ActivityBars
        days={days}
        bucket="day"
        selection={null}
        onSelect={() => undefined}
      />,
    );

    expect(container.querySelector(".chart-brush")).toBeNull();
    expect(container.querySelectorAll(".chart-bar-dim")).toHaveLength(0);
  });

  it("selects, extends and clears the range from the keyboard", () => {
    const onSelect = vi.fn();
    render(
      <ActivityBars
        days={days}
        bucket="day"
        selection={null}
        onSelect={onSelect}
      />,
    );

    // The plot is a tab stop only while it is brushable.
    const svg = getBrushSvg();
    expect(svg).toHaveAttribute("tabindex", "0");

    // A bare arrow moves a one-bucket selection…
    fireEvent.keyDown(svg, { key: "ArrowRight" });
    expect(onSelect).toHaveBeenLastCalledWith({
      fromMs: base + DAY,
      toMs: base + 2 * DAY - 1,
    });

    // …Shift+arrow extends it from where that move started…
    fireEvent.keyDown(svg, { key: "ArrowRight", shiftKey: true });
    expect(onSelect).toHaveBeenLastCalledWith({
      fromMs: base + DAY,
      toMs: base + 3 * DAY - 1,
    });

    // …End jumps to the last bucket, and Escape clears.
    fireEvent.keyDown(svg, { key: "End" });
    expect(onSelect).toHaveBeenLastCalledWith({
      fromMs: base + 9 * DAY,
      toMs: base + 10 * DAY - 1,
    });
    fireEvent.keyDown(svg, { key: "Escape" });
    expect(onSelect).toHaveBeenLastCalledWith(null);
  });

  it("survives a history that shrinks under the keyboard cursor", () => {
    const onSelect = vi.fn();
    const { rerender } = render(
      <ActivityBars
        days={days}
        bucket="day"
        selection={null}
        onSelect={onSelect}
      />,
    );

    // Put the cursor near the end of a ten-bucket history…
    fireEvent.keyDown(getBrushSvg(), { key: "End" });
    // …then re-filter to three buckets without remounting the chart.
    rerender(
      <ActivityBars
        days={days.slice(0, 3)}
        bucket="day"
        selection={null}
        onSelect={onSelect}
      />,
    );
    fireEvent.keyDown(getBrushSvg(), { key: "ArrowLeft", shiftKey: true });

    expect(onSelect).toHaveBeenLastCalledWith({
      fromMs: base + 2 * DAY,
      toMs: base + 3 * DAY - 1,
    });
  });

  it("is not a tab stop when it cannot be brushed", () => {
    render(<ActivityBars days={days} bucket="day" />);
    expect(
      screen.getByRole("img", { name: /Games per day/ }),
    ).not.toHaveAttribute("tabindex");
  });

  it("clears the selection on double-click", () => {
    const onSelect = vi.fn();
    render(
      <ActivityBars
        days={days}
        bucket="day"
        selection={{ fromMs: base, toMs: base + DAY - 1 }}
        onSelect={onSelect}
      />,
    );

    fireEvent.dblClick(getBrushSvg());
    expect(onSelect).toHaveBeenCalledWith(null);
  });
});

describe("SteppedLine", () => {
  const points = [
    { atMs: 1_700_000_000_000, rating: 2300 },
    { atMs: 1_700_086_400_000, rating: 2350 },
    { atMs: 1_700_172_800_000, rating: 2412 },
  ];

  it("draws a step path with hover titles and a final direct label", () => {
    const { container } = render(<SteppedLine points={points} />);

    const path = container.querySelector("path.chart-step");
    expect(path).not.toBeNull();
    expect(path?.getAttribute("d")).toMatch(/^M .+ H .+ V .+/);
    expect(container.querySelectorAll("circle.chart-hover")).toHaveLength(3);
    expect(
      Array.from(container.querySelectorAll("title")).some((node) =>
        /— 2350$/.test(node.textContent ?? ""),
      ),
    ).toBe(true);
    expect(container.querySelector(".chart-final-label")?.textContent).toBe(
      "2412",
    );
    expect(container.querySelector("circle.chart-dot")).not.toBeNull();
  });

  it("renders an empty note without data", () => {
    render(<SteppedLine points={[]} />);
    expect(screen.getByText("No rating data yet.")).toBeInTheDocument();
  });
});

describe("ScoreMeter", () => {
  it("fills the track proportionally and labels the value", () => {
    const { container } = render(
      <ScoreMeter label="White" scorePercent={55} games={17} />,
    );

    expect(screen.getByText("White")).toBeInTheDocument();
    expect(screen.getByText("55%")).toBeInTheDocument();
    expect(screen.getByText("17 g")).toBeInTheDocument();
    expect(
      Number(container.querySelector(".meter-fill")?.getAttribute("width")),
    ).toBeCloseTo(WIDTH * 0.55);
  });
});
