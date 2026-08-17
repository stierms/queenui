import { relativeDay, titleCase } from "../../lib/format";
import type { LabGame } from "../../types";
import { evalText } from "./format";

/** The games behind a lab statistic, each openable on Lichess. */
export function LabGameRows({
  games,
  empty,
  onOpenGame,
}: {
  games: LabGame[];
  empty: string;
  onOpenGame: (id: string) => void;
}) {
  if (games.length === 0) {
    return <p className="chart-empty">{empty}</p>;
  }
  return (
    <>
      {games.map((game) => (
        <div className="lab-game-row" key={game.id}>
          <span className="lab-game-opponent">
            {game.opponent}
            {game.opponentRating != null && (
              <em className="lab-game-rating">
                {Math.round(game.opponentRating)}
              </em>
            )}
          </span>
          <span className="lab-eval-chip">{evalText(game.peakEvalCp)}</span>
          <span className="lab-game-result">{titleCase(game.result)}</span>
          <span className="lab-game-when">
            {relativeDay(game.finishedAtMs)}
          </span>
          <button
            type="button"
            className="lab-game-id"
            title={`Open lichess.org/${game.id} in your browser`}
            onClick={() => onOpenGame(game.id)}
          >
            {game.id}
          </button>
        </div>
      ))}
    </>
  );
}
