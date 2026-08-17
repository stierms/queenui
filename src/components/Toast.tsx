import { createPortal } from "react-dom";
import type { Notice } from "../types";

/**
 * Rendered through a portal onto `document.body` so an open modal dialog
 * (which aria-hides the rest of the app) cannot hide the announcement.
 * The toast is `position: fixed`, so its visual placement is unchanged.
 */
export function Toast({
  notice,
  onDismiss,
}: {
  notice: Notice;
  onDismiss: () => void;
}) {
  /*
   * A warning is graded with the error, not with the receipt. It reports a
   * capability the operator does not have — a connected Lichess token that
   * cannot run matchmaking — which is exactly as easy to miss as a failure and
   * exactly as pointless to announce politely five seconds before erasing it.
   */
  const receipt = notice.kind === "success";
  return createPortal(
    <div
      className={`toast toast-${notice.kind}`}
      role={receipt ? "status" : "alert"}
      aria-live={receipt ? "polite" : "assertive"}
    >
      <span aria-hidden="true">{receipt ? "✓" : "!"}</span>
      {notice.message}
      {/* Only the receipt clears itself, after five seconds; everything that
          persists needs a way out, and the label names what is being closed. */}
      {!receipt && (
        <button
          type="button"
          className="toast-dismiss"
          aria-label={`Dismiss ${notice.kind}`}
          onClick={onDismiss}
        >
          ×
        </button>
      )}
    </div>,
    document.body,
  );
}
