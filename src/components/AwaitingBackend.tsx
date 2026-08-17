import { PlugZap, RotateCw } from "lucide-react";
import { Button } from "../ui/primitives";

/**
 * Shown when a different backend is running and none of its data has arrived.
 *
 * The third way this app can have nothing to show, and the one it used to get
 * wrong in silence. Publishing a backend — a runner switch, or a same-endpoint
 * pairing that adopts a rotated credential — replaces what this app dispatches
 * to; the accounts, engines and games on screen came from before that. Keeping
 * them is how a switch to an unreachable remote runner displayed the *previous*
 * runner's fleet as though it were the new one's, so they are dropped and this
 * says why the screen is empty.
 *
 * Sits between the other two: it must come before the onboarding branch, which
 * reads an empty snapshot as a fresh install, and after the unreachable-backend
 * branch, which is a different failure — there, nothing answers at all and every
 * command fails; here, the backend is answering as a runner this app has not
 * heard from yet.
 *
 * `onRetry` is the same initial fetch the unreachable panel offers, and it is
 * the honest recovery: `get_snapshot` is dispatched to whichever backend is live
 * now, so its answer is this runner's own state rather than a re-run of a stale
 * subscription.
 */
export function AwaitingBackend({
  detail,
  retrying,
  onRetry,
}: {
  /** The backend's own account of the new runner's link, when it gave one. */
  detail: string | null;
  retrying: boolean;
  onRetry: () => void;
}) {
  return (
    <section className="panel backend-unavailable" role="status">
      <span className="empty-icon">
        <PlugZap />
      </span>
      <h2>Waiting for the game runner</h2>
      <p>
        What QueenUI dispatches games to has changed, and nothing has arrived
        from it yet. This screen is empty on purpose: every account, engine and
        game it was showing was reported before the change, so none of it can be
        presented as current.
      </p>
      {detail && <pre className="backend-unavailable-detail">{detail}</pre>}
      <p className="backend-unavailable-hint">
        Games the previous runner had in flight keep playing on that machine.
        Check that the runner QueenUI is using now is running and reachable.
      </p>
      <Button variant="primary" disabled={retrying} onClick={onRetry}>
        <RotateCw size={16} />
        {retrying ? "Retrying…" : "Try again"}
      </Button>
    </section>
  );
}
