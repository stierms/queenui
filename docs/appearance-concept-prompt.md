# Prompt — new QueenUI board + piece identities

Use this when a session is asked to invent or build new chess **board and piece** concepts for QueenUI. Do not restyle the app chrome. Do not put a new set in Settings unless the user asks to integrate.

QueenUI is a Lichess **bot operations console**, not a casual chess website. The live board sits in a warm ebony shell (Ebony & Bone). Four concurrent rapid games on a 2560×1440 window is the composition target. `docs/frontend-architecture.md` owns the chrome rules; this file owns board/piece concepts.

---

## What to produce

A **paired identity**, not a palette plus a Staunton clone.

1. **Name** the pairing (board identity + piece language), one sentence **thesis**, and why it belongs in this app (material, job, or vernacular — not “looks cool”).
2. **Board:** light, dark, last-move treatment, check (claret unless there is a strong reason), coordinate contrast. Hex values.
3. **Pieces:** six silhouettes (K, Q, R, B, N, P) × two colours. A **material** (how fill, edge, and inner cuts behave), not only a silhouette.
4. If building: a working preview on the gallery, judged at **grid scale and Focus scale**, with a knight-heavy position on screen.

Default count is **four identities** unless the user names a number. Invent new ones; do not re-skin House / Scorebook / Night desk / Instrument.

---

## Product constraints (non-negotiable)

Ebony & Bone role colours — do not reuse them as decoration on the board:

| Token  | Hex       | Job                                                                                                                 |
| ------ | --------- | ------------------------------------------------------------------------------------------------------------------- |
| Bone   | `#e9e4d6` | Primary action + strong text. Allowed as a House light square / last-move inset.                                    |
| Ebony  | `#141210` | App ground. Allowed as a House/Night dark square.                                                                   |
| Moss   | `#8fae62` | **Alive and nothing else** (THINKING, live dots, active clocks). Never last-move, never a green tournament board.   |
| Brass  | `#d9a441` | Warning / stale. Last-move may not look like a warning. Deco pieces already use brass stroke — new sets should not. |
| Claret | `#dd7a6f` | Error / danger. Check may use it. Last-move may not.                                                                |
| Slate  | `#8fb4d4` | Info. Not a board colour.                                                                                           |

Other hard rules:

- Last-move is a **treatment** (inset hairline, underline, bevel), not a lime wash. Forest/Charcoal’s `#c6ff62` is the anti-pattern.
- Black pieces keep the **halo rim** (`piece-halo-layer`) so they read on dark squares.
- Geometry must differ, not only CSS filters. A fourth “Staunton with a drop shadow” is how the catalog got okayish.
- Pieces are **SVG in `ChessPiece.tsx`**, viewBox `0 0 100 100`. Do not generate raster sprites or Imagine assets for these.
- Preview at ~52px (strip) and ~70–90px (board). A knight that only reads at 200px fails.
- Low-contrast boards (Night desk class) still need readable file/rank labels — override `.rank`/`.file` if both squares are dark.

---

## Quality bar for pieces

The first pass of Horn/Nib/Lamp/Foundry shipped as blobs with an eye. That failed review. The bar now:

- **Knight is the test.** Head in profile, ear, muzzle, jaw, mane or mill-mark, eye, neck into a chest, then a base. If it reads as a pebble, it is not done.
- **Every piece has construction:** pawn collar + waist; rook merlons and at least two body rings; bishop mitre **cleft**; queen coronet (beads or mills); king cross or equivalent; a pedestal that matches the set (turned, printed foot, tall stem, or milled plinth).
- Inner cuts use `piece-detail` / `piece-eye` (halo layer already hides those). Body mass uses `piece-body` (tests require at least one).
- White and black are the **same cut**, different fill. White must survive a light square; black must survive a dark square with the halo.
- Do **not** blow up `Figurine.tsx` glyphs for board pieces. Those paths are for 12px SAN. Nib had to be redrawn as printed _chessmen_ with interior hairlines.

Staunton in the same file is the complexity floor for a knight, not the style to copy.

---

## What already exists (do not clone)

**In Settings today:** the original boards Forest, Walnut, Slate, Marble, Plum,
Charcoal and pieces Regal, Staunton, Ink, Blueprint, Deco, plus these approved
paired identities:

| Board       | Pieces   |
| ----------- | -------- |
| Relay board | Relay    |
| Basalt      | Chisel   |
| Monotype    | Matrix   |
| Optic       | Optic    |
| Switchgear  | Contact  |
| Kiln        | Shard    |
| Compositor  | Sort     |
| Paperwhite  | Aperture |

Retired ids: pixel, crystal, neo.

**In the gallery only** (`src/lib/appearanceIdentities.ts`):

| Identity   | Board                | Pieces  | Thesis                             |
| ---------- | -------------------- | ------- | ---------------------------------- |
| House      | bone / ebony         | Horn    | Chrome colours on the board        |
| Scorebook  | parchment / oak-gall | Nib     | Score sheet; printed chessmen      |
| Night desk | two close ebonies    | Lamp    | Stare-able; clocks carry attention |
| Instrument | pewter / graphite    | Foundry | Milled, not drawn                  |

Do not add: another wood, another green, a blue “ice” board, neon last-move, Pixel/Crystal/Neo by name without a material.

Fresh directions that are still empty (examples, not a mandate): inlay/brass-free metal other than pewter; stone that is not Marble grey; night-blue is banned (Slate already failed); a set that matches Plex Mono / Spectral; a set for 8-board grids that stays legible at 40px.

---

## How to build (preview, not Settings)

Keep new work off the Settings picker until the user likes it.

1. Add a `PieceSetId` in `src/ChessPiece.tsx` and a `*Geometry` function. Wire it in `PieceGeometry`.
2. Add a row to `APPEARANCE_IDENTITIES` in `src/lib/appearanceIdentities.ts` (id, name, thesis, pieceSet, pieceName, light, dark, accent, highlight).
3. Piece finish CSS in `src/App.css` under `.piece-set-<id>` (white/black `--piece-main`, `--piece-edge`, `--piece-halo`). Last-move CSS under `.board-identity-<id> .square.last-move::after`.
4. `identityBoardStyle()` already feeds `--board-light` / `--board-dark` into the real `Chessboard`.
5. Gallery: `src/dev/SkinsGallery.tsx`, mounted from `src/main.tsx` when `?skins-preview` (DEV only, `hasPreviewParam`).
6. Tests: `src/ChessPiece.test.tsx` preview set ids; `src/lib/appearanceIdentities.test.ts` for the catalog. Every geometry must include `.piece-body`.
7. Run `npm run dev`, open `http://localhost:<port>/?skins-preview`. Positions: Start, Ruy Lopez (knights on the board), Ending. Enlarge = Focus scale.

Do not add the set to `boardThemes` / `pieceSets` in `src/lib/appearance.ts` unless asked to integrate.

---

## Session method

1. Read this file. Skim `ChessPiece.tsx` finishes for the four gallery sets so you do not copy them.
2. Write the identities in prose first (name, thesis, hex, last-move, material). Reject any that could be a Lichess/chess.com theme pack.
3. Implement in the gallery only. Knights first.
4. Look at Ruy Lopez at grid scale, then Enlarge. If the knight fails, fix geometry, not CSS shadow.
5. Stop at the gallery unless the user asks to ship into Settings.

When the user says “same thing as last time,” they mean: four paired identities, preview-only, real SVG on the real board, judged in the gallery — not a mood board and not a Settings dump.
