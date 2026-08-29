import { describe, expect, it } from "vitest";
import {
  APPEARANCE_IDENTITIES,
  identityBoardStyle,
} from "./appearanceIdentities";

describe("appearance identities", () => {
  it("ships twelve paired board and piece languages", () => {
    expect(APPEARANCE_IDENTITIES.map((identity) => identity.id)).toEqual([
      "house",
      "scorebook",
      "night",
      "instrument",
      "relay",
      "basalt",
      "monotype",
      "optic",
      "switchgear",
      "kiln",
      "compositor",
      "paperwhite",
    ]);
    expect(APPEARANCE_IDENTITIES.map((identity) => identity.pieceSet)).toEqual([
      "horn",
      "nib",
      "lamp",
      "foundry",
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

  it("does not use moss as a last-move colour", () => {
    for (const identity of APPEARANCE_IDENTITIES) {
      expect(identity.highlight.toLowerCase()).not.toBe("#8fae62");
      expect(identity.accent.toLowerCase()).not.toBe("#c6ff62");
    }
  });

  it("preserves the four gallery identities that preceded this concept pass", () => {
    expect(
      APPEARANCE_IDENTITIES.slice(4, 8).map(
        ({ name, pieceName, light, dark }) => ({
          name,
          pieceName,
          light,
          dark,
        }),
      ),
    ).toEqual([
      {
        name: "Relay",
        pieceName: "Relay",
        light: "#c29462",
        dark: "#362114",
      },
      {
        name: "Basalt",
        pieceName: "Chisel",
        light: "#b8b0a2",
        dark: "#222426",
      },
      {
        name: "Monotype",
        pieceName: "Matrix",
        light: "#cfc9bc",
        dark: "#35323a",
      },
      {
        name: "Optic",
        pieceName: "Optic",
        light: "#85928e",
        dark: "#1e2424",
      },
    ]);
  });

  it("keeps the four new pairings and their materials explicit", () => {
    expect(
      APPEARANCE_IDENTITIES.slice(8).map(
        ({ name, pieceName, light, dark, highlight }) => ({
          name,
          pieceName,
          light,
          dark,
          highlight,
        }),
      ),
    ).toEqual([
      {
        name: "Switchgear",
        pieceName: "Contact",
        light: "#aaa39a",
        dark: "#3b332f",
        highlight: "#c9c2b8",
      },
      {
        name: "Kiln",
        pieceName: "Shard",
        light: "#b78368",
        dark: "#3a2e2a",
        highlight: "#d8b9a5",
      },
      {
        name: "Compositor",
        pieceName: "Sort",
        light: "#c9c4b9",
        dark: "#38343d",
        highlight: "#8b8490",
      },
      {
        name: "Paperwhite",
        pieceName: "Aperture",
        light: "#b5b2a8",
        dark: "#272927",
        highlight: "#d7d8cf",
      },
    ]);
  });

  it("keeps app role colours off the new boards and their move treatments", () => {
    const reserved = new Set([
      "#e9e4d6",
      "#141210",
      "#8fae62",
      "#d9a441",
      "#dd7a6f",
      "#8fb4d4",
      "#c6ff62",
    ]);

    for (const identity of APPEARANCE_IDENTITIES.slice(8)) {
      for (const color of [
        identity.light,
        identity.dark,
        identity.accent,
        identity.highlight,
      ]) {
        expect(reserved).not.toContain(color.toLowerCase());
      }
    }
  });

  it("exposes board CSS variables for the live renderer", () => {
    const style = identityBoardStyle(APPEARANCE_IDENTITIES[0]) as Record<
      string,
      string
    >;
    expect(style["--board-light"]).toBe("#e9e4d6");
    expect(style["--board-dark"]).toBe("#141210");
    expect(style["--board-check"]).toBe("#dd7a6f");
  });
});
