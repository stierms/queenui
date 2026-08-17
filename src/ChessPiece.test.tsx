import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import {
  ChessPiece,
  type PieceColor,
  type PieceKind,
  type PieceSetId,
} from "./ChessPiece";
import { pieceSets, storedPieceSet } from "./lib/appearance";

const kinds: PieceKind[] = ["p", "n", "b", "r", "q", "k"];
const colors: PieceColor[] = ["w", "b"];
const setIds: PieceSetId[] = ["regal", "staunton", "ink", "blueprint", "deco"];

afterEach(() => {
  cleanup();
  localStorage.clear();
});

describe("piece set catalog", () => {
  it("offers all five sets with regal first", () => {
    expect(pieceSets.map((set) => set.id)).toEqual(setIds);
  });

  it("falls back to regal for retired stored set ids", () => {
    localStorage.setItem("queenui-piece-set", "pixel");
    expect(storedPieceSet()).toBe("regal");
    localStorage.setItem("queenui-piece-set", "crystal");
    expect(storedPieceSet()).toBe("regal");
    localStorage.setItem("queenui-piece-set", "staunton");
    expect(storedPieceSet()).toBe("staunton");
  });
});

describe.each(setIds)("ChessPiece set %s", (setId) => {
  it.each(kinds)("renders both colors of piece type %s", (kind) => {
    for (const color of colors) {
      const { container, unmount } = render(
        <ChessPiece type={kind} color={color} pieceSet={setId} />,
      );
      const svg = container.querySelector("svg.chess-piece");
      expect(svg).not.toBeNull();
      expect(svg).toHaveClass(`piece-set-${setId}`);
      expect(svg).toHaveClass(color === "w" ? "piece-white" : "piece-black");
      expect(svg!.querySelectorAll(".piece-body").length).toBeGreaterThan(0);
      expect(svg!.querySelector(".piece-halo-layer")).not.toBeNull();
      unmount();
    }
  });
});
