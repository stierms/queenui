import type { CSSProperties } from "react";
import type { PieceSetId } from "../ChessPiece";

export type AppearanceIdentity = {
  id: string;
  name: string;
  thesis: string;
  pieceSet: PieceSetId;
  pieceName: string;
  pieceDescription: string;
  light: string;
  dark: string;
  accent: string;
  highlight: string;
};

export const APPEARANCE_IDENTITIES = [
  {
    id: "house",
    name: "House",
    thesis: "The console’s own bone and ebony, on the board.",
    pieceSet: "horn",
    pieceName: "Horn",
    pieceDescription: "Lathe-turned ivory",
    light: "#e9e4d6",
    dark: "#141210",
    accent: "#e9e4d6",
    highlight: "#e9e4d6",
  },
  {
    id: "scorebook",
    name: "Scorebook",
    thesis: "Parchment and oak-gall. Pieces are the figurine alphabet.",
    pieceSet: "nib",
    pieceName: "Nib",
    pieceDescription: "Printed chessmen",
    light: "#e4dcc8",
    dark: "#5c4a38",
    accent: "#3e3428",
    highlight: "#3e3428",
  },
  {
    id: "night",
    name: "Night desk",
    thesis: "Two close ebonies. Attention sits on clocks, not the chequer.",
    pieceSet: "lamp",
    pieceName: "Lamp",
    pieceDescription: "Barley-sugar turnings",
    light: "#2a2622",
    dark: "#1a1816",
    accent: "#e9e4d6",
    highlight: "#e9e4d6",
  },
  {
    id: "instrument",
    name: "Instrument",
    thesis: "Pewter and graphite. Pieces look milled, not drawn.",
    pieceSet: "foundry",
    pieceName: "Foundry",
    pieceDescription: "Milled metal planes",
    light: "#9a958c",
    dark: "#3e3b36",
    accent: "#d4cfc4",
    highlight: "#d4cfc4",
  },
  {
    id: "relay",
    name: "Relay",
    thesis:
      "Phenolic ground and copper wire-wound coils from telephone exchanges.",
    pieceSet: "relay",
    pieceName: "Relay",
    pieceDescription: "Wire-wound insulators",
    light: "#c29462",
    dark: "#362114",
    accent: "#e0a858",
    highlight: "#e0a858",
  },
  {
    id: "basalt",
    name: "Basalt",
    thesis:
      "Honed travertine and raw volcanic rock split along natural cleavage planes.",
    pieceSet: "chisel",
    pieceName: "Chisel",
    pieceDescription: "Cleaved stone planes",
    light: "#b8b0a2",
    dark: "#222426",
    accent: "#d8d2c4",
    highlight: "#d8d2c4",
  },
  {
    id: "monotype",
    name: "Monotype",
    thesis:
      "Punch-cut hot-metal type sorts and ink-trap matrix geometry matching Plex Mono.",
    pieceSet: "matrix",
    pieceName: "Matrix",
    pieceDescription: "Hot-metal type sorts",
    light: "#cfc9bc",
    dark: "#35323a",
    accent: "#b8b0c4",
    highlight: "#b8b0c4",
  },
  {
    id: "optic",
    name: "Optic",
    thesis:
      "High-density 8-board ops grid with anti-reflective ground and reticle apertures.",
    pieceSet: "optic",
    pieceName: "Optic",
    pieceDescription: "Reticle apertures",
    light: "#85928e",
    dark: "#1e2424",
    accent: "#d0e0dc",
    highlight: "#d0e0dc",
  },
  {
    id: "switchgear",
    name: "Switchgear",
    thesis: "Putty enamel and hard rubber. Pieces are porcelain relay stacks.",
    pieceSet: "switchgear",
    pieceName: "Contact",
    pieceDescription: "Porcelain contact stacks",
    light: "#aaa39a",
    dark: "#3b332f",
    accent: "#c9c2b8",
    highlight: "#c9c2b8",
  },
  {
    id: "kiln",
    name: "Kiln",
    thesis: "Fired clay and manganese slip. Pieces are pressed stoneware.",
    pieceSet: "kiln",
    pieceName: "Shard",
    pieceDescription: "Pressed stoneware",
    light: "#b78368",
    dark: "#3a2e2a",
    accent: "#d8b9a5",
    highlight: "#d8b9a5",
  },
  {
    id: "compositor",
    name: "Compositor",
    thesis: "Composing stone and ink-violet. Pieces are slab-cut type sorts.",
    pieceSet: "compositor",
    pieceName: "Sort",
    pieceDescription: "Slab-cut type sorts",
    light: "#c9c4b9",
    dark: "#38343d",
    accent: "#8b8490",
    highlight: "#8b8490",
  },
  {
    id: "paperwhite",
    name: "Paperwhite",
    thesis:
      "E-paper and carbon. Aperture pieces stay legible in an 8-board grid.",
    pieceSet: "aperture",
    pieceName: "Aperture",
    pieceDescription: "E-paper cutouts",
    light: "#b5b2a8",
    dark: "#272927",
    accent: "#d7d8cf",
    highlight: "#d7d8cf",
  },
] as const satisfies readonly AppearanceIdentity[];

/** The approved identities promoted from the DEV gallery into Settings. */
export const INTEGRATED_APPEARANCE_IDENTITIES = [
  APPEARANCE_IDENTITIES[4],
  APPEARANCE_IDENTITIES[5],
  APPEARANCE_IDENTITIES[6],
  APPEARANCE_IDENTITIES[7],
  APPEARANCE_IDENTITIES[8],
  APPEARANCE_IDENTITIES[9],
  APPEARANCE_IDENTITIES[10],
  APPEARANCE_IDENTITIES[11],
] as const;

export type AppearanceIdentityId = (typeof APPEARANCE_IDENTITIES)[number]["id"];

export function identityBoardStyle(
  identity: AppearanceIdentity,
): CSSProperties {
  return {
    "--board-light": identity.light,
    "--board-dark": identity.dark,
    "--board-accent": identity.accent,
    "--board-highlight": identity.highlight,
    "--board-check": "#dd7a6f",
  } as CSSProperties;
}
