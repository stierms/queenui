import { AlertTriangle } from "lucide-react";
import { formatClock, remainingClock, sideToMove } from "../lib/chess";
import type { LiveGame } from "../types";
import { Button, Dialog } from "../ui/primitives";

/**
 * Warns before closing QueenUI while it is still playing.
 *
 * Closing does not resign anything — the games stay open on Lichess with their
 * clocks running, so each one is eventually lost on time. That consequence is
 * the whole reason this dialog exists, so it is stated plainly rather than
 * asked as a bare "are you sure?".
 */
export function CloseGuard({
  games,
  reportedCount = 0,
  pending,
  onKeepPlaying,
  onClose,
}: {
  games: LiveGame[];
  /**
   * How many games the backend counted when it blocked the close. The
   * snapshot can trail that count by a moment, which is exactly why the close
   * event carries it — the warning must be raisable before the games arrive.
   */
  reportedCount?: number;
  pending: boolean;
  onKeepPlaying: () => void;
  onClose: () => void;
}) {
  const count = Math.max(games.length, reportedCount);
  return (
    <Dialog.Root
      open
      onOpenChange={(open) => {
        if (!open) onKeepPlaying();
      }}
    >
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-20 bg-black/70 backdrop-blur-[7px]" />
        <Dialog.Content className="close-guard-modal fixed left-1/2 top-1/2 z-[21] -translate-x-1/2 -translate-y-1/2 focus:outline-none">
          <div className="modal-head">
            <div className="modal-icon close-guard-icon">
              <AlertTriangle size={20} />
            </div>
            <div>
              <span className="eyebrow">Games in progress</span>
              <Dialog.Title>
                {count === 1
                  ? "A game is still being played"
                  : `${count} games are still being played`}
              </Dialog.Title>
              <Dialog.Description>
                Closing QueenUI stops playing {count === 1 ? "it" : "them"}.
                Lichess keeps the clocks running, so{" "}
                {count === 1 ? "it is" : "they are"} lost on time.
              </Dialog.Description>
            </div>
          </div>
          {games.length === 0 && (
            <p className="close-guard-pending">
              Their details have not arrived yet — the count comes from the
              service that is still playing them.
            </p>
          )}
          <ul className="close-guard-games">
            {games.map((game) => {
              const ourTurn =
                (game.color === "white") === (sideToMove(game) === "w");
              const ourClock =
                game.color === "white" ? game.whiteTime : game.blackTime;
              return (
                <li key={`${game.accountId}-${game.id}`}>
                  <span className="close-guard-avatar">{game.opponent[0]}</span>
                  <div>
                    <strong>{game.opponent}</strong>
                    <small>
                      {game.botUsername} ·{" "}
                      {game.color === "white" ? "White" : "Black"} ·{" "}
                      {ourTurn ? "your move" : "opponent to move"}
                    </small>
                  </div>
                  <time className={ourTurn ? "close-guard-ticking" : ""}>
                    {formatClock(
                      remainingClock(ourClock, ourTurn, game.clockUpdatedAt),
                    )}
                  </time>
                </li>
              );
            })}
          </ul>
          <div className="modal-actions">
            <Button variant="primary" onClick={onKeepPlaying} autoFocus>
              Keep playing
            </Button>
            <Button variant="danger" disabled={pending} onClick={onClose}>
              Close and abandon {count === 1 ? "the game" : "them"}
            </Button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
