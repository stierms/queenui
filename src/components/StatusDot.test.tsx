import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { StatusDot } from "./StatusDot";

afterEach(cleanup);

describe("StatusDot", () => {
  it("styles the statuses this build knows", () => {
    render(<StatusDot status="playing" />);
    expect(screen.getByRole("img", { name: "Playing" })).toHaveClass(
      "status-playing",
    );
  });

  it("announces the same word the screen shows", () => {
    // `online` is displayed as "Connected" everywhere it is written out, so
    // the dot beside that text must not announce a different word.
    render(<StatusDot status="online" />);
    expect(screen.getByRole("img", { name: "Connected" })).toHaveClass(
      "status-online",
    );
  });

  it("never puts an unrecognised backend string in the class attribute", () => {
    // `BotRuntime.status` is a bare `string` in the generated contract, so a
    // status added on the Rust side arrives here before this build knows it.
    render(<StatusDot status='" onmouseover="x' />);
    const dot = screen.getByRole("img", { name: '" onmouseover="x' });
    expect(dot).toHaveClass("status-unknown");
    expect(dot.className).toBe("status-dot status-unknown");
  });
});
