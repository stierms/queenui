import { afterEach, describe, expect, it } from "vitest";
import {
  boardAppearanceStyle,
  boardThemes,
  pieceSets,
  storedBoardTheme,
  storedPieceSet,
} from "./appearance";
import { INTEGRATED_APPEARANCE_IDENTITIES } from "./appearanceIdentities";

afterEach(() => {
  localStorage.clear();
});

describe("integrated appearance catalog", () => {
  it("adds the eight approved identities after the original board themes", () => {
    expect(boardThemes.map((theme) => theme.id)).toEqual([
      "forest",
      "walnut",
      "midnight",
      "marble",
      "plum",
      "charcoal",
      "relay",
      "basalt",
      "monotype",
      "optic",
      "switchgear",
      "kiln",
      "compositor",
      "paperwhite",
    ]);
    expect(pieceSets.map((set) => set.id)).toEqual([
      "regal",
      "staunton",
      "ink",
      "blueprint",
      "deco",
      "relay",
      "chisel",
      "matrix",
      "optic",
      "switchgear",
      "kiln",
      "compositor",
      "aperture",
    ]);
  });

  it("keeps each approved board paired with its intended piece language", () => {
    expect(
      INTEGRATED_APPEARANCE_IDENTITIES.map(
        ({ id, pieceSet }) => `${id}:${pieceSet}`,
      ),
    ).toEqual([
      "relay:relay",
      "basalt:chisel",
      "monotype:matrix",
      "optic:optic",
      "switchgear:switchgear",
      "kiln:kiln",
      "compositor:compositor",
      "paperwhite:aperture",
    ]);
  });

  it("persists approved choices but keeps gallery-only concepts out", () => {
    localStorage.setItem("queenui-board-theme", "kiln");
    localStorage.setItem("queenui-piece-set", "aperture");
    expect(storedBoardTheme()).toBe("kiln");
    expect(storedPieceSet()).toBe("aperture");

    localStorage.setItem("queenui-board-theme", "house");
    localStorage.setItem("queenui-piece-set", "horn");
    expect(storedBoardTheme()).toBe("forest");
    expect(storedPieceSet()).toBe("regal");
  });

  it("feeds the approved board and claret check colours into real boards", () => {
    expect(boardAppearanceStyle("kiln")).toMatchObject({
      "--board-light": "#b78368",
      "--board-dark": "#3a2e2a",
      "--board-highlight": "#d8b9a5",
      "--board-check": "#dd7a6f",
    });
  });
});
