import { fullDate, titleCase } from "../../lib/format";
import type { ScorebookLab } from "../../types";
import { percentText } from "./format";

/** How deep the engine got at each speed. */
export function DepthByPerfTable({
  rows,
}: {
  rows: ScorebookLab["depthByPerf"];
}) {
  return (
    <section className="panel scorebook-engine-table lab-table">
      <div className="panel-heading">
        <div>
          <span className="eyebrow">Search</span>
          <h2>Search depth by speed</h2>
        </div>
      </div>
      <div
        className="scorebook-table lab-depth-table"
        role="table"
        aria-label="Search depth by speed"
      >
        <div className="scorebook-table-head" role="row">
          <span role="columnheader">Speed</span>
          <span role="columnheader">Games</span>
          <span role="columnheader">Avg depth</span>
          <span role="columnheader">Min depth</span>
        </div>
        {rows.map((row) => (
          <div className="scorebook-table-row" role="row" key={row.perf}>
            <span role="cell" className="scorebook-cell-name">
              {titleCase(row.perf)}
            </span>
            <span role="cell">{row.games}</span>
            <span role="cell">
              {row.avgDepth == null ? "—" : row.avgDepth.toFixed(1)}
            </span>
            <span role="cell">{row.minDepth ?? "—"}</span>
          </div>
        ))}
      </div>
    </section>
  );
}

/** Results grouped by the options + book hash the games were played under. */
export function ConfigCohortsTable({
  rows,
}: {
  rows: ScorebookLab["byConfig"];
}) {
  return (
    <section className="panel scorebook-engine-table lab-table">
      <div className="panel-heading">
        <div>
          <span className="eyebrow">Cohorts</span>
          <h2>Config cohorts</h2>
        </div>
      </div>
      <div
        className="scorebook-table lab-config-table"
        role="table"
        aria-label="Config cohorts"
      >
        <div className="scorebook-table-head" role="row">
          <span role="columnheader">Fingerprint</span>
          <span role="columnheader">Engine</span>
          <span role="columnheader">Games</span>
          <span role="columnheader">Score%</span>
          <span role="columnheader">First–last seen</span>
        </div>
        {rows.map((row) => (
          <div className="scorebook-table-row" role="row" key={row.fingerprint}>
            <span
              role="cell"
              className="lab-fingerprint"
              title="Engine options + book settings hash — rows with different fingerprints played under different configurations."
            >
              {row.fingerprint.slice(0, 12)}
            </span>
            <span role="cell" className="scorebook-cell-name">
              {row.engineName}
            </span>
            <span role="cell">{row.games}</span>
            <span role="cell">{row.scorePercent.toFixed(1)}%</span>
            <span role="cell" className="lab-seen">
              {fullDate(row.firstSeenMs)} – {fullDate(row.lastSeenMs)}
            </span>
          </div>
        ))}
      </div>
    </section>
  );
}

/** Search internals per engine — the half of the record Lichess cannot see. */
export function EngineInternalsTable({
  rows,
}: {
  rows: ScorebookLab["byEngineLab"];
}) {
  return (
    <section className="panel scorebook-engine-table lab-table">
      <div className="panel-heading">
        <div>
          <span className="eyebrow">Engines</span>
          <h2>Engine internals</h2>
        </div>
      </div>
      <div
        className="scorebook-table lab-internals-table"
        role="table"
        aria-label="Engine internals"
      >
        <div className="scorebook-table-head" role="row">
          <span role="columnheader">Engine</span>
          <span role="columnheader">Games</span>
          <span role="columnheader">Avg depth</span>
          <span role="columnheader">Blunders/g</span>
          <span role="columnheader">Conversion</span>
          <span role="columnheader">Avg move</span>
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
            <span role="cell">
              {row.avgDepth == null ? "—" : row.avgDepth.toFixed(1)}
            </span>
            <span role="cell">
              {row.avgBlunders == null ? "—" : row.avgBlunders.toFixed(2)}
            </span>
            <span role="cell">{percentText(row.conversionRate)}</span>
            <span role="cell">
              {row.avgMoveTimeMs == null
                ? "—"
                : `${(row.avgMoveTimeMs / 1000).toFixed(1)}s`}
            </span>
          </div>
        ))}
      </div>
    </section>
  );
}
