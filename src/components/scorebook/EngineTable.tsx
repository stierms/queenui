import type { ScorebookStats } from "../../types";
import { ratingText } from "./format";

/** Per-engine results; the imported-history row is muted, not attributed. */
export function EngineTable({ rows }: { rows: ScorebookStats["byEngine"] }) {
  return (
    <section className="panel scorebook-engine-table">
      <div className="panel-heading">
        <div>
          <span className="eyebrow">Engines</span>
          <h2>Results by engine</h2>
        </div>
      </div>
      <div
        className="scorebook-table"
        role="table"
        aria-label="Results by engine"
      >
        <div className="scorebook-table-head" role="row">
          <span role="columnheader">Engine</span>
          <span role="columnheader">Games</span>
          <span role="columnheader">W</span>
          <span role="columnheader">D</span>
          <span role="columnheader">L</span>
          <span role="columnheader">Score%</span>
          <span role="columnheader">Avg opp</span>
          <span role="columnheader">Perf</span>
        </div>
        {rows.map((row) => (
          <div
            className={`scorebook-table-row${row.engineId == null ? " scorebook-muted" : ""}`}
            role="row"
            key={row.engineId ?? "unknown"}
          >
            <span role="cell" className="scorebook-cell-name">
              {row.engineName}
            </span>
            <span role="cell">{row.games}</span>
            <span role="cell">{row.wins}</span>
            <span role="cell">{row.draws}</span>
            <span role="cell">{row.losses}</span>
            <span role="cell">{row.scorePercent.toFixed(1)}%</span>
            <span role="cell">{ratingText(row.avgOpponentRating)}</span>
            <span role="cell">{ratingText(row.performanceRating)}</span>
          </div>
        ))}
      </div>
    </section>
  );
}
