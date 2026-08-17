import { Timer } from "lucide-react";
import { titleCase } from "../../lib/format";
import { ScoreMeter, StackedRow } from "../charts";
import type { ScorebookStats } from "../../types";

/** Score against each rating band of the field. */
export function OpponentStrengthPanel({
  bands,
}: {
  bands: ScorebookStats["byOpponentBand"];
}) {
  return (
    <section className="panel scorebook-panel">
      <div className="panel-heading">
        <div>
          <span className="eyebrow">Field</span>
          <h2>Opponent strength</h2>
        </div>
      </div>
      <div className="scorebook-panel-body">
        {bands.map((band) => (
          <StackedRow
            key={band.label}
            label={band.label}
            wins={band.wins}
            draws={band.draws}
            losses={band.losses}
            scorePercent={band.scorePercent}
          />
        ))}
        {bands.length === 0 && (
          <p className="chart-empty">No rated opponents recorded yet.</p>
        )}
      </div>
    </section>
  );
}

/** How games finish, with the time-forfeit count called out. */
export function TerminationsPanel({
  terminations,
  timeLosses,
}: {
  terminations: ScorebookStats["byTermination"];
  timeLosses: number;
}) {
  return (
    <section className="panel scorebook-panel">
      <div className="panel-heading">
        <div>
          <span className="eyebrow">Terminations</span>
          <h2>How games end</h2>
        </div>
      </div>
      <div className="scorebook-panel-body">
        {terminations.map((row) => (
          <StackedRow
            key={row.status}
            label={titleCase(row.status)}
            wins={row.wins}
            draws={row.draws}
            losses={row.losses}
          />
        ))}
        {terminations.length === 0 && (
          <p className="chart-empty">No finished games recorded yet.</p>
        )}
        {timeLosses > 0 && (
          <p className="scorebook-callout">
            <Timer size={14} />
            {timeLosses} loss
            {timeLosses === 1 ? "" : "es"} on time — check engine time
            management
          </p>
        )}
      </div>
    </section>
  );
}

/** White vs. black score. */
export function ByColorPanel({
  byColor,
}: {
  byColor: ScorebookStats["byColor"];
}) {
  const whiteSplit = byColor.find((row) => row.color === "white");
  const blackSplit = byColor.find((row) => row.color === "black");
  return (
    <section className="panel scorebook-panel">
      <div className="panel-heading">
        <div>
          <span className="eyebrow">Sides</span>
          <h2>By color</h2>
        </div>
      </div>
      <div className="scorebook-panel-body">
        <ScoreMeter
          label="White"
          scorePercent={whiteSplit?.scorePercent ?? 0}
          games={whiteSplit?.games ?? 0}
        />
        <ScoreMeter
          label="Black"
          scorePercent={blackSplit?.scorePercent ?? 0}
          games={blackSplit?.games ?? 0}
        />
      </div>
    </section>
  );
}
