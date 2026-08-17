import type { ScorebookStats } from "../../types";
import { ratingText, streakText } from "./format";

/** The six-figure summary strip under the hero. */
export function SummaryKpis({ stats }: { stats: ScorebookStats }) {
  return (
    <section className="scorebook-kpis" aria-label="Scorebook summary">
      <div>
        <span>Games</span>
        <strong>{stats.totalGames}</strong>
      </div>
      <div>
        <span>Score</span>
        <strong>{stats.scorePercent.toFixed(1)}%</strong>
      </div>
      <div>
        <span>Record</span>
        <strong className="scorebook-record">
          <b className="record-win">{stats.wins}</b>–
          <b className="record-draw">{stats.draws}</b>–
          <b className="record-loss">{stats.losses}</b>
        </strong>
      </div>
      <div>
        <span>Streak</span>
        <strong>{streakText(stats.streak)}</strong>
      </div>
      <div>
        <span>Avg opponent</span>
        <strong>{ratingText(stats.avgOpponentRating)}</strong>
      </div>
      <div>
        <span>Performance</span>
        <strong>{ratingText(stats.performanceRating)}</strong>
      </div>
    </section>
  );
}
