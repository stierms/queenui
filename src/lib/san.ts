/**
 * SAN tokenizing, split out of `Figurine` so that component file exports a
 * component and nothing else (React Fast Refresh gives up on a module that
 * mixes the two, and the whole file stops hot-reloading).
 */

/** The five piece letters SAN uses, and the only ones with a glyph. */
export const FIGURINE_PIECES = ["K", "Q", "R", "B", "N"] as const;

export type GlyphPiece = (typeof FIGURINE_PIECES)[number];

export type SanToken =
  { kind: "piece"; piece: GlyphPiece } | { kind: "text"; text: string };

const PIECE_LETTERS: ReadonlySet<string> = new Set<string>(FIGURINE_PIECES);

/**
 * Splits a SAN move string into piece-letter and literal-text tokens.
 * Uppercase K/Q/R/B/N only ever denote pieces in SAN (files are lowercase,
 * castling uses the letter O), so a promotion target like "e8=Q" tokenizes
 * to the text "e8=" followed by the queen piece.
 */
export function sanTokens(san: string): SanToken[] {
  const tokens: SanToken[] = [];
  let text = "";
  for (const character of san) {
    if (PIECE_LETTERS.has(character)) {
      if (text) tokens.push({ kind: "text", text });
      text = "";
      tokens.push({ kind: "piece", piece: character as GlyphPiece });
    } else {
      text += character;
    }
  }
  if (text) tokens.push({ kind: "text", text });
  return tokens;
}
