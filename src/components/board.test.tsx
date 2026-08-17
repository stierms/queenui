import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { Chessboard, EvalRail, LiveClock } from "./board";
import type { LiveGame } from "../types";

const whiteInCheckFen = "4k3/8/8/8/8/8/4q3/4K3 w - - 0 1";

function makeGame(overrides: Partial<LiveGame> = {}): LiveGame {
  return {
    id: "board-game",
    accountId: "acct",
    botUsername: "QueenBot",
    opponent: "Rival",
    color: "white",
    initialFen: "startpos",
    moves: "",
    status: "started",
    whiteTime: 60_000,
    blackTime: 60_000,
    whiteIncrement: 0,
    blackIncrement: 0,
    clockUpdatedAt: Date.now(),
    botRating: null,
    opponentRating: null,
    result: null,
    engineLine: null,
    engineInfo: null,
    error: null,
    engineThinking: false,
    ...overrides,
  };
}

afterEach(cleanup);

describe("Chessboard check indicator", () => {
  it("marks the checked king's square with in-check", () => {
    const { container } = render(
      <Chessboard
        game={makeGame({ initialFen: whiteInCheckFen })}
        pieceSet="regal"
      />,
    );
    const checked = container.querySelector(".square.in-check");
    expect(checked).toHaveAttribute("aria-label", "e1: white king");
    expect(container.querySelectorAll(".square.in-check")).toHaveLength(1);
  });

  it("does not mark any square when nobody is in check", () => {
    const { container } = render(
      <Chessboard game={makeGame()} pieceSet="regal" />,
    );
    expect(container.querySelector(".square.in-check")).toBeNull();
  });
});

describe("LiveClock low-time flagging", () => {
  it("adds clock-low under 20 seconds while active", () => {
    const { container } = render(
      <LiveClock milliseconds={15_000} active clockUpdatedAt={Date.now()} />,
    );
    const clock = container.querySelector("time");
    expect(clock).toHaveClass("active-clock", "clock-low");
    expect(clock).not.toHaveClass("clock-critical");
  });

  it("adds clock-critical under 10 seconds while active", () => {
    const { container } = render(
      <LiveClock milliseconds={8_000} active clockUpdatedAt={Date.now()} />,
    );
    expect(container.querySelector("time")).toHaveClass(
      "clock-low",
      "clock-critical",
    );
  });

  it("never flags an inactive clock", () => {
    const { container } = render(
      <LiveClock
        milliseconds={8_000}
        active={false}
        clockUpdatedAt={Date.now()}
      />,
    );
    expect(container.querySelector("time")?.className).toBe("");
  });
});

describe("EvalRail white-perspective fill", () => {
  const info = { scoreCp: 200 } as LiveGame["engineInfo"];

  it("keeps our-perspective fill from the bottom when we are white", () => {
    const { container } = render(<EvalRail info={info} ourColor="white" />);
    const fill = container.querySelector<HTMLElement>(".eval-fill");
    expect(Number.parseFloat(fill!.style.height)).toBeGreaterThan(50);
    expect(fill!.style.top).toBe("");
  });

  it("inverts and top-anchors the fill when we are black", () => {
    const { container } = render(<EvalRail info={info} ourColor="black" />);
    const fill = container.querySelector<HTMLElement>(".eval-fill");
    expect(Number.parseFloat(fill!.style.height)).toBeLessThan(50);
    expect(fill!.style.top).toBe("0px");
  });
});
