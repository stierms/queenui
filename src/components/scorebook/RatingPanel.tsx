import { SteppedLine, type RatingPoint } from "../charts";

/** The rating trend for whatever slice the filters currently describe. */
export function RatingPanel({ points }: { points: RatingPoint[] }) {
  return (
    <section className="panel scorebook-panel">
      <div className="panel-heading">
        <div>
          <span className="eyebrow">Trend</span>
          <h2>Rating</h2>
        </div>
      </div>
      <div className="scorebook-panel-body">
        <SteppedLine points={points} />
      </div>
    </section>
  );
}
