import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { Figurine } from "./Figurine";
import { sanTokens } from "../lib/san";

afterEach(cleanup);

describe("sanTokens", () => {
  it("splits a piece move into a glyph and the destination", () => {
    expect(sanTokens("Nf6")).toEqual([
      { kind: "piece", piece: "N" },
      { kind: "text", text: "f6" },
    ]);
  });

  it("keeps pawn moves and captures as plain text", () => {
    expect(sanTokens("e4")).toEqual([{ kind: "text", text: "e4" }]);
    expect(sanTokens("exf5")).toEqual([{ kind: "text", text: "exf5" }]);
  });

  it("keeps castling literal (the letter O is not a piece)", () => {
    expect(sanTokens("O-O")).toEqual([{ kind: "text", text: "O-O" }]);
    expect(sanTokens("O-O-O")).toEqual([{ kind: "text", text: "O-O-O" }]);
  });

  it("keeps capture and check markers around the glyph", () => {
    expect(sanTokens("Qxe7+")).toEqual([
      { kind: "piece", piece: "Q" },
      { kind: "text", text: "xe7+" },
    ]);
  });

  it("renders the promotion piece as a glyph after the equals sign", () => {
    expect(sanTokens("e8=Q")).toEqual([
      { kind: "text", text: "e8=" },
      { kind: "piece", piece: "Q" },
    ]);
  });
});

describe("Figurine", () => {
  it("keeps the exact SAN string as accessible text content", () => {
    const { container } = render(<Figurine san="Nf6" />);
    expect(container.textContent).toBe("Nf6");
    expect(container.querySelectorAll("svg")).toHaveLength(1);
    expect(container.querySelector("svg")).toHaveAttribute(
      "aria-hidden",
      "true",
    );
  });

  it("renders glyph-free moves without any svg", () => {
    const { container } = render(<Figurine san="O-O-O" />);
    expect(container.textContent).toBe("O-O-O");
    expect(container.querySelector("svg")).toBeNull();
  });
});
