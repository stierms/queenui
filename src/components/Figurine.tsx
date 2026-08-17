import { memo } from "react";
import { sanTokens, type GlyphPiece } from "../lib/san";

/**
 * Original minimalist piece silhouettes, one closed path each,
 * drawn for a 0 0 100 100 viewBox and filled with currentColor.
 */
const glyphPaths: Record<GlyphPiece, string> = {
  K: "M44 6h12v12h12v12H56v8c13 2 22 12 22 25l-3 17h9v14H16V80h9l-3-17c0-13 9-23 22-25v-8H32V18h12V6z",
  Q: "M50 4l8 22 18-14-5 32c8 4 11 12 9 20l-4 16h8v14H16V80h8l-4-16c-2-8 1-16 9-20l-5-32 18 14 8-22z",
  R: "M20 8h14v10h9V8h14v10h9V8h14v24l-8 8v30l8 8v2h4v14H16V80h4v-2l8-8V40l-8-8V8z",
  B: "M50 6c13 11 21 25 21 39 0 11-7 19-14 22l4 11h13v14H26V78h13l4-11c-7-3-14-11-14-22 0-14 8-28 21-39z",
  N: "M27 92V81c0-15 8-23 17-29l-13 6c-6 3-12 0-14-6l-2-6 15-19c8-10 18-14 28-13l2-9 7 11c12 6 17 18 17 34v42H27z",
};

/**
 * A SAN move rendered with figurine piece glyphs ("Nf6" becomes ♞-glyph +
 * "f6"). The piece letter is kept as visually hidden text, so the element's
 * accessible text content is exactly the SAN string.
 *
 * There is deliberately no `color` prop: the glyphs are one silhouette filled
 * with `currentColor` and do not differ by side. The prop used to be declared
 * and never destructured, and two call sites computed side-alternation
 * expressions purely to feed it — dead work that read as a feature.
 */
export const Figurine = memo(function Figurine({ san }: { san: string }) {
  return (
    <span className="figurine">
      {sanTokens(san).map((token, index) =>
        token.kind === "piece" ? (
          <span key={index}>
            <span className="sr-only">{token.piece}</span>
            <svg viewBox="0 0 100 100" aria-hidden="true" focusable="false">
              <path d={glyphPaths[token.piece]} fill="currentColor" />
            </svg>
          </span>
        ) : (
          <span key={index}>{token.text}</span>
        ),
      )}
    </span>
  );
});
