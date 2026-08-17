import { useRef, useState, type KeyboardEvent } from "react";
import { ListTree } from "lucide-react";
import { mateText, pawnsText, searchTime } from "../../lib/evaluation";
import type { LogSearchBlock } from "../../types";

/**
 * Centipawns as signed pawns; a forced mate wins over a score.
 *
 * `mateText`/`pawnsText` are shared with the board's evaluation readout, so a
 * forced mate reads `M3` in both places — the outline used to spell the same
 * thing `#3`, four inches from a board saying `M3`. The elapsed column moved
 * onto the shared `searchTime` for the same reason, which also means a
 * sub-second search now reads `640 ms` here rather than `0.64 s`.
 */
function blockScoreText(block: LogSearchBlock) {
  if (block.mateIn != null) return mateText(block.mateIn);
  if (block.scoreCp == null) return "—";
  return pawnsText(block.scoreCp);
}

/**
 * Search blocks report the side to move as the UCI-style "w"/"b" the note
 * line carries, while a session reports "white"/"black" as Lichess names it.
 * Both spellings resolve here so black's moves always read as `23…`.
 */
function isBlackToMove(color: string | null | undefined) {
  return color?.toLowerCase().startsWith("b") ?? false;
}

function blockMoveText(block: LogSearchBlock) {
  return `${block.moveNumber}${isBlackToMove(block.color) ? "…" : "."}`;
}

/**
 * The searches of one session, as a vertical toolbar.
 *
 * A real game leaves hundreds of blocks here. Every row being a tab stop put
 * the log canvas a hundred-odd presses away, so the rail uses a roving
 * tabindex: one row is tabbable and the arrow keys move between them.
 */
export function OutlineRail({
  blocks,
  activeLine,
  failed,
  onJump,
  onRetry,
}: {
  blocks: LogSearchBlock[];
  activeLine: number | null;
  failed: boolean;
  onJump: (line: number) => void;
  onRetry: () => void;
}) {
  const railRef = useRef<HTMLDivElement>(null);
  const [focusIndex, setFocusIndex] = useState(0);

  const activeIndex = blocks.findIndex(
    (block) =>
      activeLine != null &&
      activeLine >= block.startLine &&
      activeLine <= block.endLine,
  );

  // The single tab stop is wherever the operator last put it — clamped, so a
  // refresh that shortens the outline cannot strand it past the end.
  const roving = Math.min(
    Math.max(focusIndex, 0),
    Math.max(blocks.length - 1, 0),
  );

  function moveFocus(next: number) {
    if (blocks.length === 0) return;
    const clamped = Math.min(Math.max(next, 0), blocks.length - 1);
    setFocusIndex(clamped);
    const rows =
      railRef.current?.querySelectorAll<HTMLButtonElement>(".logs-outline-row");
    rows?.[clamped]?.focus();
  }

  function onRailKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    const step: Record<string, number> = {
      ArrowDown: 1,
      ArrowRight: 1,
      ArrowUp: -1,
      ArrowLeft: -1,
      PageDown: 10,
      PageUp: -10,
    };
    if (event.key === "Home" || event.key === "End") {
      event.preventDefault();
      moveFocus(event.key === "Home" ? 0 : blocks.length - 1);
      return;
    }
    const delta = step[event.key];
    if (delta === undefined) return;
    event.preventDefault();
    moveFocus(roving + delta);
  }

  return (
    <div className="logs-outline">
      <div className="logs-outline-head">
        <span className="eyebrow">
          <ListTree size={12} /> Searches
        </span>
        <em>{failed ? "—" : blocks.length}</em>
      </div>
      <div
        className="logs-outline-scroll"
        role="toolbar"
        aria-orientation="vertical"
        aria-label="Search blocks"
        ref={railRef}
        onKeyDown={onRailKeyDown}
      >
        {blocks.map((block, index) => {
          const active = index === activeIndex;
          return (
            <button
              type="button"
              className={`logs-outline-row${active ? " selected" : ""}`}
              // The row the canvas is parked on was distinguished by colour
              // alone inside a roving-tabindex toolbar.
              aria-current={active ? "true" : undefined}
              tabIndex={index === roving ? 0 : -1}
              title={`Jump to line ${block.startLine + 1} — ply ${block.ply}, lines ${block.startLine + 1}–${block.endLine + 1}`}
              onClick={() => {
                setFocusIndex(index);
                onJump(block.startLine);
              }}
              key={`${block.ply}-${block.startLine}`}
            >
              <span className="logs-outline-move">{blockMoveText(block)}</span>
              <span className="logs-outline-depth">
                {block.depth == null ? "d—" : `d${block.depth}`}
              </span>
              <span className="logs-outline-score">
                {blockScoreText(block)}
              </span>
              <span className="logs-outline-elapsed">
                {searchTime(block.elapsedMs)}
              </span>
              <span className="logs-outline-best">{block.bestMove ?? "—"}</span>
            </button>
          );
        })}
        {blocks.length === 0 &&
          (failed ? (
            <p className="logs-outline-empty logs-outline-failed">
              The search outline could not be read.
              <button type="button" onClick={onRetry}>
                Retry
              </button>
            </p>
          ) : (
            <p className="logs-outline-empty">No completed searches yet.</p>
          ))}
      </div>
    </div>
  );
}
