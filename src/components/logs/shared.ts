/**
 * The pieces of the Logs page more than one of its panels needs: how a
 * session names itself, how a result reads, and the two utilities the
 * viewer and the diagnostics list both reach for.
 */
import { useEffect, useState } from "react";
import { relativeDay, titleCase } from "../../lib/format";
import type { LogSessionSummary } from "../../types";

/** Matches returned by one in-session or cross-session search. */
export const MATCH_LIMIT = 500;

/** Debounce for the two free-text filters, so typing is not a command per key. */
export const TEXT_DEBOUNCE_MS = 220;

export function resultText(session: LogSessionSummary) {
  if (session.live) return "In progress";
  if (!session.result) return session.status ? titleCase(session.status) : "—";
  return session.result === "1/2-1/2" ? "½–½" : session.result;
}

/**
 * The row's eyebrow already carries the live dot, so the result slot shows
 * when the game started instead of repeating it.
 */
export function rowResultText(session: LogSessionSummary) {
  return session.live ? relativeDay(session.startedAtMs) : resultText(session);
}

export function sessionLabel(session: LogSessionSummary) {
  return session.opponent || session.gameId || session.botUsername;
}

/** Resolves false when there is no clipboard (sandboxes, jsdom). */
export async function copyText(value: string) {
  const clipboard = navigator.clipboard;
  if (!clipboard?.writeText) return false;
  try {
    await clipboard.writeText(value);
    return true;
  } catch {
    return false;
  }
}

/** Debounced mirror of a text field, so typing is not one command per key. */
export function useDebounced(value: string) {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const timer = window.setTimeout(
      () => setDebounced(value),
      TEXT_DEBOUNCE_MS,
    );
    return () => window.clearTimeout(timer);
  }, [value]);
  return debounced;
}
