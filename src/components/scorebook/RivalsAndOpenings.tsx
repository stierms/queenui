import { relativeDay } from "../../lib/format";
import type { ScorebookStats } from "../../types";
import { percentText } from "./format";

/** The opponents seen most often, newest contact last on the right. */
export function TopOpponentsPanel({
  opponents,
}: {
  opponents: ScorebookStats["topOpponents"];
}) {
  return (
    <section className="panel scorebook-panel">
      <div className="panel-heading">
        <div>
          <span className="eyebrow">Rivals</span>
          <h2>Most played opponents</h2>
        </div>
      </div>
      <div className="scorebook-panel-body scorebook-list">
        {opponents.map((opponent) => (
          <div className="scorebook-list-row" key={opponent.name}>
            <span className="scorebook-cell-name">{opponent.name}</span>
            <span className="scorebook-record-cell">
              {opponent.wins}–{opponent.draws}–{opponent.losses}
            </span>
            <span>{percentText(opponent.scorePercent)}</span>
            <span className="scorebook-cell-when">
              {relativeDay(opponent.lastPlayedAtMs)}
            </span>
          </div>
        ))}
        {opponents.length === 0 && (
          <p className="chart-empty">No opponents yet.</p>
        )}
      </div>
    </section>
  );
}

/** Named openings the bots actually reached, with their score. */
export function OpeningsPanel({
  openings,
}: {
  openings: ScorebookStats["openings"];
}) {
  return (
    <section className="panel scorebook-panel">
      <div className="panel-heading">
        <div>
          <span className="eyebrow">Repertoire</span>
          <h2>Openings</h2>
        </div>
      </div>
      <div className="scorebook-panel-body scorebook-list">
        {openings.map((opening) => (
          <div className="scorebook-list-row" key={opening.name}>
            <span className="scorebook-opening-name">{opening.name}</span>
            {/* "g" appeared here and nowhere else; every table on this page
                calls the same quantity Games. */}
            <span>
              {opening.games} game{opening.games === 1 ? "" : "s"}
            </span>
            <span>{percentText(opening.scorePercent)}</span>
          </div>
        ))}
        {/* The parent used to unmount this whole panel when the list was
            empty, which reads as "this build has no Openings" rather than
            "no named opening has been reached yet". */}
        {openings.length === 0 && (
          <p className="chart-empty">
            No named openings yet — imported games carry no opening name.
          </p>
        )}
      </div>
    </section>
  );
}
