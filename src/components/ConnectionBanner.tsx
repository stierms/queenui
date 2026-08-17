import { PlugZap, RefreshCw, WifiOff } from "lucide-react";
import {
  connectionSummary,
  isStale,
  type ConnectionState,
} from "../lib/connection";
import { timeOfDayShort } from "../lib/format";
import { Button } from "../ui/primitives";

/**
 * The one place the app admits that what is on screen may be out of date.
 *
 * Rendered above every page whenever the runner link is degraded or the
 * backend is unreachable, so no screen can present frozen data as live.
 *
 * There is deliberately no branch for the embedded runner. A degraded link is
 * *state*, not a message log, and switching to this computer retires it in the
 * reducer — so this stops rendering because `isStale` is false again, not
 * because anything here special-cases the switch.
 */
export function ConnectionBanner({
  connection,
  onRetry,
}: {
  connection: ConnectionState;
  onRetry?: () => void;
}) {
  if (!isStale(connection)) return null;
  const summary = connectionSummary(connection);
  if (!summary.headline) return null;
  const Icon = connection.backendUnavailable
    ? PlugZap
    : connection.link === "disconnected"
      ? WifiOff
      : RefreshCw;
  /*
   * How old the screen is — which is only a fact worth stating while the screen
   * still shows something. A backend generation change empties it, and
   * `lastSnapshotAtMs` then timestamps a snapshot from the *previous* backend
   * that is no longer displayed anywhere; "Last update 14:02" beside "Waiting
   * for the game runner" would date an empty screen to a runner that is not this
   * app's any more. Same lesson as `lastOkAtMs` not surviving a switch, one
   * field over.
   */
  const lastSeen = connection.awaitingBackendData
    ? null
    : (connection.lastOkAtMs ?? connection.lastSnapshotAtMs);
  return (
    <div
      className={`connection-banner connection-banner-${summary.tone}`}
      role="status"
      aria-live="polite"
    >
      <span className="connection-banner-icon" aria-hidden="true">
        <Icon size={17} />
      </span>
      <p>
        <strong>{summary.headline}</strong>
        {summary.detail && <small>{summary.detail}</small>}
      </p>
      {lastSeen != null && (
        <span className="connection-banner-age">
          Last update {timeOfDayShort(lastSeen)}
        </span>
      )}
      {/*
       * Offered for the two states a fetch can actually resolve: an unreachable
       * backend, and a newly published one that has sent nothing yet — the
       * retry is a `get_snapshot` command, dispatched to whichever backend is
       * live, so it asks the new runner directly. A degraded *link* is not one
       * of them: nothing this app can call makes the runner reachable again.
       */}
      {onRetry &&
        (connection.backendUnavailable || connection.awaitingBackendData) && (
          <Button variant="secondary" onClick={onRetry}>
            Try again
          </Button>
        )}
    </div>
  );
}

/**
 * Inline marker for a single frozen surface — a board, a fleet row, a clock
 * group. The banner explains *why*; this says *which* pixels not to trust.
 */
export function StaleMark({ label = "Frozen" }: { label?: string }) {
  return (
    <span className="stale-mark" title="Waiting for the runner to reconnect">
      <i aria-hidden="true" />
      {label}
    </span>
  );
}
