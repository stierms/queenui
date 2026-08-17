import { TriangleAlert } from "lucide-react";
import { Button } from "../ui/primitives";
import type { LiveGame } from "../types";

/**
 * A game whose task died, kept on screen until the operator dismisses it.
 *
 * The failure this card exists for: a board vanished mid-game and every screen
 * agreed nothing was wrong. `is_live` excluded the failed game so no panel drew
 * it, and pruning swept it away with the finished ones a moment later, so there
 * was nothing left to click on and no text anywhere naming a cause. The backend
 * retains these now; this is the surface that shows one, and `dismiss_game_error`
 * is the only thing that removes it.
 *
 * Deliberately not a `LiveGamePanel`. That component would draw a board, print
 * "Finished · *" over a half-played position, and give the error text nowhere
 * to go — which is a prettier version of the same lie.
 *
 * No timestamp: the record's only clock field is `clockUpdatedAt`, which is
 * when Lichess last updated the *clock*, not when the task failed. Ordering the
 * retention cap by it is fine; printing "failed 4m ago" from it would not be.
 */
export function FailedGameCard({
  game,
  pending,
  onDismiss,
}: {
  game: LiveGame;
  pending: boolean;
  onDismiss: (game: LiveGame) => void;
}) {
  /*
   * The opponent is copied from the game context, which starts empty and is
   * filled in from Lichess's `gameFull`. A task that died before that arrived
   * has no opponent to name, and inventing one — or printing an empty "vs." —
   * would be worse than saying so.
   */
  const opponent = game.opponent.trim();
  return (
    // `alert`, not `status`: this arrives by snapshot push, and it is the
    // announcement of a game that has stopped playing.
    <article className="panel failed-game" role="alert">
      <div className="failed-game-mark" aria-hidden="true">
        <TriangleAlert size={19} />
      </div>
      <div className="failed-game-copy">
        <span className="eyebrow failed-eyebrow">Game failed</span>
        <h3>
          {opponent ? (
            <>
              {game.botUsername} <span>vs.</span> {opponent}
            </>
          ) : (
            <>{game.botUsername} — opponent unknown</>
          )}
        </h3>
        {/*
         * The backend's own text, verbatim and unclipped. It is the only
         * record of what went wrong that reaches this screen at all.
         */}
        <p className="failed-game-detail">
          {game.error ?? "QueenUI recorded no reason for this failure."}
        </p>
        <small>lichess.org/{game.id}</small>
      </div>
      <Button
        variant="secondary"
        // Distinct per card: several games can fail, and "Dismiss" three times
        // over is three controls an operator cannot tell apart by name.
        aria-label={`Dismiss the failed game ${game.id}`}
        disabled={pending}
        onClick={() => onDismiss(game)}
      >
        {pending ? "Dismissing…" : "Dismiss"}
      </Button>
    </article>
  );
}
