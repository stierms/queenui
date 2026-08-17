import { BookOpen } from "lucide-react";
import { durationMmSs } from "../../lib/format";
import { ScoreMeter } from "../charts";
import type { ScorebookLab } from "../../types";
import { evalText, percentText } from "./format";
import { LabGameRows } from "./LabGameRows";
import {
  ConfigCohortsTable,
  DepthByPerfTable,
  EngineInternalsTable,
} from "./LabTables";

/** Book-vs-no-book score, with a hint when the preparation is losing games. */
function BookPanel({ book }: { book: NonNullable<ScorebookLab["book"]> }) {
  return (
    <section className="panel scorebook-panel lab-book">
      <div className="panel-heading">
        <div>
          <span className="eyebrow">Preparation</span>
          <h2>Opening book</h2>
        </div>
      </div>
      <div className="scorebook-panel-body">
        <ScoreMeter
          label="With book"
          scorePercent={book.scoreWith}
          games={book.gamesWithBook}
        />
        {book.scoreWithout != null && (
          <ScoreMeter
            label="Without"
            scorePercent={book.scoreWithout}
            games={book.gamesWithout}
          />
        )}
        <p className="lab-book-facts">
          Avg book plies{" "}
          {book.avgBookPlies == null ? "—" : book.avgBookPlies.toFixed(1)} · Avg
          exit eval{" "}
          {book.avgExitEvalCp == null ? "—" : evalText(book.avgExitEvalCp)}
        </p>
        {book.scoreWithout != null &&
          book.scoreWith < book.scoreWithout - 5 && (
            <p className="scorebook-callout">
              <BookOpen size={14} />
              Book lines may be underperforming — check exit evals.
            </p>
          )}
      </div>
    </section>
  );
}

/** Restarts, retries, reconnects — what went wrong around the games. */
function ReliabilityStrip({
  reliability,
}: {
  reliability: ScorebookLab["reliability"];
}) {
  return (
    <section className="lab-reliability" aria-label="Reliability">
      <span className="eyebrow">Reliability</span>
      <div className="scorebook-kpis lab-reliability-strip">
        <div>
          <span>Engine restarts</span>
          <strong
            className={
              reliability.engineRestarts > 0 ? "lab-stat-brass" : undefined
            }
          >
            {reliability.engineRestarts}
          </strong>
        </div>
        <div>
          <span>Submission retries</span>
          <strong>{reliability.submissionRetries}</strong>
        </div>
        <div>
          <span>Stream reconnects</span>
          <strong>{reliability.streamReconnects}</strong>
        </div>
        <div>
          <span>Failure resigns</span>
          <strong
            className={
              reliability.failureResigns > 0 ? "lab-stat-claret" : undefined
            }
          >
            {reliability.failureResigns}
          </strong>
        </div>
      </div>
    </section>
  );
}

/**
 * Everything the games played through QueenUI know that Lichess does not:
 * engine attribution, search internals, conversion and reliability.
 */
export function LabPanel({
  lab,
  onOpenGame,
}: {
  lab: ScorebookLab | null;
  onOpenGame: (id: string) => void;
}) {
  return (
    <section className="lab-section" aria-label="Engine lab">
      <header className="lab-header">
        <span className="eyebrow">Engine lab</span>
        <h2>What Lichess can&rsquo;t see</h2>
        <p>
          Telemetry from games played through QueenUI — engine attribution,
          search internals, and reliability.
        </p>
      </header>

      {lab == null ? (
        <div className="panel lab-quiet">
          <p>
            The Engine lab fills in as your bots play games through QueenUI.
            Imported history has no engine telemetry.
          </p>
        </div>
      ) : (
        <>
          {/* `section`, not `div`: a named section is a region and keeps the
              label, which a bare div drops. The other two KPI strips
              (SummaryKpis, ReliabilityStrip) already do it this way. */}
          <section
            className="scorebook-kpis lab-kpis"
            aria-label="Engine lab summary"
          >
            <div>
              <span>Telemetry games</span>
              <strong>{lab.telemetryGames}</strong>
            </div>
            <div title="Of positions held at +3.00 or better, the share converted to wins">
              <span>Conversion</span>
              <strong>{percentText(lab.conversionRate)}</strong>
            </div>
            <div title="Of positions at −3.00 or worse, the share saved to a draw or win">
              <span>Defense</span>
              <strong>{percentText(lab.defenseRate)}</strong>
            </div>
            <div>
              <span>Blunders/game</span>
              <strong>
                {lab.avgBlundersPerGame == null
                  ? "—"
                  : lab.avgBlundersPerGame.toFixed(2)}
              </strong>
            </div>
            <div title="Games lost on time while standing at +3.00 or better — engine time management failed">
              <span>Flagged winning</span>
              <strong
                className={
                  lab.flaggedWinning > 0 ? "lab-stat-claret" : undefined
                }
              >
                {lab.flaggedWinning}
              </strong>
            </div>
            <div>
              <span>Avg clock left</span>
              <strong>{durationMmSs(lab.avgEndClockMs)}</strong>
            </div>
          </section>

          <div className="lab-two-up">
            <section className="panel scorebook-panel lab-thrown">
              <div className="panel-heading">
                <div>
                  <span className="eyebrow">Conversion failures</span>
                  <h2>Thrown wins</h2>
                </div>
              </div>
              <div className="scorebook-panel-body lab-game-list">
                <LabGameRows
                  games={lab.thrownWins}
                  empty="No thrown wins — clean conversions."
                  onOpenGame={onOpenGame}
                />
              </div>
            </section>
            <section className="panel scorebook-panel lab-steals">
              <div className="panel-heading">
                <div>
                  <span className="eyebrow">Defense</span>
                  <h2>Great escapes</h2>
                </div>
              </div>
              <div className="scorebook-panel-body lab-game-list">
                <LabGameRows
                  games={lab.steals}
                  empty="No escapes yet."
                  onOpenGame={onOpenGame}
                />
              </div>
            </section>
          </div>

          <DepthByPerfTable rows={lab.depthByPerf} />

          {lab.book && <BookPanel book={lab.book} />}

          {lab.byConfig.length >= 2 && (
            <ConfigCohortsTable rows={lab.byConfig} />
          )}

          <ReliabilityStrip reliability={lab.reliability} />

          <EngineInternalsTable rows={lab.byEngineLab} />
        </>
      )}
    </section>
  );
}
