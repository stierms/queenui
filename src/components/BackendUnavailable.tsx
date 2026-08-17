import { PlugZap, RotateCw } from "lucide-react";
import { Button } from "../ui/primitives";

/**
 * Shown when the backend never answered and there is nothing to display.
 *
 * This branch has to come *before* the "no engines yet" onboarding branch: an
 * unreachable backend leaves the snapshot empty, and an empty snapshot used to
 * be read as a fresh install — so an operator whose remote runner had died was
 * told to set up their first engine, with no error anywhere on screen.
 */
export function BackendUnavailable({
  detail,
  retrying,
  onRetry,
}: {
  detail: string | null;
  retrying: boolean;
  onRetry: () => void;
}) {
  return (
    <section className="panel backend-unavailable" role="alert">
      <span className="empty-icon">
        <PlugZap />
      </span>
      <h2>QueenUI can't reach its backend</h2>
      <p>
        Nothing here is live. Accounts, engines and games could not be read, so
        this screen is empty because the service did not answer — not because
        nothing is configured.
      </p>
      {detail && <pre className="backend-unavailable-detail">{detail}</pre>}
      <p className="backend-unavailable-hint">
        If a remote runner is configured, check that it is running and
        reachable; otherwise restart QueenUI.
      </p>
      <Button variant="primary" disabled={retrying} onClick={onRetry}>
        <RotateCw size={16} />
        {retrying ? "Retrying…" : "Try again"}
      </Button>
    </section>
  );
}
