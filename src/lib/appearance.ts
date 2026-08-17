import type { CSSProperties } from "react";
import type { PieceSetId } from "../ChessPiece";

export const boardThemes = [
  {
    id: "forest",
    name: "Forest",
    light: "#bdc4a6",
    dark: "#55684f",
    accent: "#c6ff62",
    highlight: "#9fbf3a",
  },
  {
    id: "walnut",
    name: "Walnut",
    light: "#dcc4a4",
    dark: "#7a563f",
    accent: "#ffd36a",
    highlight: "#d99a2b",
  },
  {
    id: "midnight",
    name: "Slate",
    light: "#7f8fa3",
    dark: "#39485c",
    accent: "#77dcff",
    highlight: "#3fa8d6",
  },
  {
    id: "marble",
    name: "Marble",
    light: "#d9d5c9",
    dark: "#77756f",
    accent: "#e8b84b",
    highlight: "#a9c23c",
  },
  {
    id: "plum",
    name: "Plum",
    light: "#cbb8ce",
    dark: "#755f79",
    accent: "#d678f0",
    highlight: "#b45cc9",
  },
  {
    id: "charcoal",
    name: "Charcoal",
    light: "#6a7076",
    dark: "#3a3f46",
    accent: "#c6ff62",
    highlight: "#8fae3a",
  },
] as const;

export type BoardThemeId = (typeof boardThemes)[number]["id"];

export const pieceSets: {
  id: PieceSetId;
  name: string;
  description: string;
}[] = [
  { id: "regal", name: "Regal", description: "Sculpted tournament" },
  { id: "staunton", name: "Staunton", description: "Classic club profile" },
  { id: "ink", name: "Ink", description: "Printed figurine" },
  { id: "blueprint", name: "Blueprint", description: "Schematic linework" },
  { id: "deco", name: "Deco", description: "Brass-age geometry" },
];

export function storedBoardTheme(): BoardThemeId {
  const stored = localStorage.getItem("queenui-board-theme");
  return boardThemes.some((theme) => theme.id === stored)
    ? (stored as BoardThemeId)
    : "forest";
}

export function storedPieceSet(): PieceSetId {
  const stored = localStorage.getItem("queenui-piece-set");
  return pieceSets.some((set) => set.id === stored)
    ? (stored as PieceSetId)
    : "regal";
}

export function boardAppearanceStyle(themeId: BoardThemeId) {
  const theme =
    boardThemes.find((item) => item.id === themeId) ?? boardThemes[0];
  return {
    "--board-light": theme.light,
    "--board-dark": theme.dark,
    "--board-accent": theme.accent,
    "--board-highlight": theme.highlight,
  } as CSSProperties;
}
