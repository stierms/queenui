import { memo, useEffect, useRef } from "react";
import {
  Activity,
  Bot,
  ChevronDown,
  ChevronUp,
  CircleDot,
  Gauge,
  Zap,
} from "lucide-react";
import { moveRows, principalVariationSan } from "../lib/chess";
import { compactNumber, evaluationLabel, searchTime } from "../lib/evaluation";
import type { CollapsedWidgets, GameWidget } from "../lib/gameView";
import type { EngineTelemetry, LiveGame } from "../types";
import { Figurine } from "./Figurine";

/**
 * The chevron that puts a widget away.
 *
 * `aria-expanded` and a name that says which way the press goes, because a
 * bare chevron is unreadable to anything that cannot see it — and this control
 * is the only way back to a widget the operator has collapsed.
 */
function CollapseToggle({
  widget,
  label,
  collapsed,
  onToggle,
}: {
  widget: GameWidget;
  label: string;
  collapsed: boolean;
  onToggle: (widget: GameWidget) => void;
}) {
  return (
    <button
      type="button"
      className="widget-collapse"
      aria-expanded={!collapsed}
      aria-label={`${collapsed ? "Expand" : "Collapse"} ${label}`}
      onClick={() => onToggle(widget)}
    >
      {collapsed ? <ChevronDown size={16} /> : <ChevronUp size={16} />}
    </button>
  );
}

export const EngineTelemetryPanel = memo(function EngineTelemetryPanel({
  game,
  engineName,
  evaluation,
  frozen = false,
  collapsed,
  onToggleWidget,
}: {
  game: LiveGame;
  engineName: string;
  /**
   * Last scored evaluation, which outlives the search that produced it. The
   * per-search figures below (depth, nodes, speed) still reset with the
   * search itself.
   */
  evaluation?: EngineTelemetry | null;
  /** The snapshot is out of date; "Thinking" is no longer a claim we can make. */
  frozen?: boolean;
  /**
   * Which widgets are put away. Only ever set by the focus view: the Overview
   * board passes neither of these and keeps both widgets open, with no
   * chevrons to press.
   */
  collapsed?: CollapsedWidgets;
  onToggleWidget?: (widget: GameWidget) => void;
}) {
  const info = game.engineInfo;
  // `engineThinking` freezes wherever it happened to be when the link dropped,
  // so the pulse would otherwise run forever on a search that has long ended.
  const thinking = game.engineThinking && !frozen;
  const pv = principalVariationSan(game);
  const rows = moveRows(game);
  const plyCount = game.moves.split(/\s+/).filter(Boolean).length;
  const latestMove =
    rows[rows.length - 1]?.black ?? rows[rows.length - 1]?.white;
  const movesScrollRef = useRef<HTMLDivElement | null>(null);
  const analysisPutAway = collapsed?.analysis === true;
  const movesPutAway = collapsed?.moves === true;

  // Keep the latest move visible as new moves stream in.
  useEffect(() => {
    const scroller = movesScrollRef.current;
    if (scroller) scroller.scrollTop = scroller.scrollHeight;
  }, [rows.length, latestMove, movesPutAway]);
  return (
    <div className="game-details">
      <section
        className={`engine-telemetry ${thinking ? "is-thinking" : ""} ${
          analysisPutAway ? "is-collapsed" : ""
        }`}
      >
        <header className="telemetry-heading">
          <span className="engine-glyph">
            <Bot size={18} />
          </span>
          <div>
            <span>{thinking ? "Calculating" : "Engine analysis"}</span>
            <strong>{engineName}</strong>
          </div>
          <em>
            <i />
            {frozen
              ? "Last seen"
              : thinking
                ? "Thinking"
                : info
                  ? "Last search"
                  : "Standing by"}
          </em>
          {onToggleWidget && (
            <CollapseToggle
              widget="analysis"
              label="engine analysis"
              collapsed={analysisPutAway}
              onToggle={onToggleWidget}
            />
          )}
        </header>
        {/*
         * The heading survives the collapse on purpose: it carries the engine's
         * name and whether it is thinking, which is the one thing a put-away
         * widget must not stop reporting.
         */}
        {!analysisPutAway && (
          <>
            <div className="telemetry-primary">
              <div>
                <span>Evaluation</span>
                <strong>{evaluationLabel(evaluation)}</strong>
                <small>
                  our perspective
                  {evaluation?.scoreBound ? ` · ${evaluation.scoreBound}` : ""}
                </small>
              </div>
              <div className="depth-visual">
                <span>Depth</span>
                <strong>
                  {info?.depth ?? "—"}
                  <small>
                    {info?.selectiveDepth ? ` / ${info.selectiveDepth}` : ""}
                  </small>
                </strong>
                <small>plies{info?.selectiveDepth ? " / selective" : ""}</small>
              </div>
            </div>
            <div className="telemetry-grid">
              <div>
                <Zap size={13} />
                <span>Speed</span>
                <strong>
                  {info?.nodesPerSecond == null
                    ? "—"
                    : `${compactNumber(info.nodesPerSecond)}/s`}
                </strong>
              </div>
              <div>
                <Activity size={13} />
                <span>Nodes</span>
                <strong>{compactNumber(info?.nodes)}</strong>
              </div>
              <div>
                <Gauge size={13} />
                <span>Search</span>
                <strong>{searchTime(info?.timeMs)}</strong>
              </div>
              <div>
                <CircleDot size={13} />
                <span>Hash</span>
                <strong>
                  {info?.hashFull == null
                    ? "—"
                    : `${(info.hashFull / 10).toFixed(1)}%`}
                </strong>
              </div>
            </div>
            <div className="pv-line">
              <span>Principal variation</span>
              <code>
                {pv
                  ? pv.split(" ").map((san, index) => (
                      <span key={index}>
                        {index > 0 && " "}
                        <Figurine san={san} />
                      </span>
                    ))
                  : thinking
                    ? "Waiting for the first completed depth…"
                    : "No search line available yet"}
              </code>
            </div>
          </>
        )}
      </section>
      <section className={`moves-list ${movesPutAway ? "is-collapsed" : ""}`}>
        <div className="moves-header">
          <span>Moves</span>
          {/* "plies", not "plys" — and not a bare "ply" for every count,
              which is what this read before. `countText`'s naive +"s" cannot
              spell this one. */}
          <small>{plyCount === 1 ? "1 ply" : `${plyCount} plies`}</small>
          {onToggleWidget && (
            <CollapseToggle
              widget="moves"
              label="moves"
              collapsed={movesPutAway}
              onToggle={onToggleWidget}
            />
          )}
        </div>
        {!movesPutAway && (
          <>
            <div className="move-columns">
              <span>#</span>
              <span>White</span>
              <span>Black</span>
            </div>
            {rows.length ? (
              <div className="moves-scroll" ref={movesScrollRef}>
                {rows.map((row, index) => (
                  <div
                    className={index === rows.length - 1 ? "current-move" : ""}
                    key={row.number}
                  >
                    <span>{row.number}.</span>
                    <strong>
                      {row.white ? <Figurine san={row.white} /> : "—"}
                    </strong>
                    <strong>
                      {row.black ? <Figurine san={row.black} /> : "—"}
                    </strong>
                  </div>
                ))}
              </div>
            ) : (
              <div className="moves-empty">Waiting for the first move</div>
            )}
          </>
        )}
      </section>
      {/* An engine failing mid-game arrives by snapshot push; name what
          broke and announce it rather than dropping a backend string in. */}
      {game.error && (
        <p className="game-error" role="alert">
          <strong>Engine problem</strong> {game.error}
        </p>
      )}
    </div>
  );
});
