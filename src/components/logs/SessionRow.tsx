import { formatBytes, relativeDay, titleCase } from "../../lib/format";
import type { LogSessionMatches, LogSessionSummary } from "../../types";
import { rowResultText, sessionLabel } from "./shared";

/** One recorded session in the list pane, with its deep-search hit. */
export function SessionRow({
  session,
  hit,
  selected,
  onSelect,
}: {
  session: LogSessionSummary;
  hit?: LogSessionMatches;
  selected: boolean;
  onSelect: () => void;
}) {
  return (
    <button
      type="button"
      className={`logs-session-row${selected ? " selected" : ""}`}
      aria-pressed={selected}
      onClick={onSelect}
    >
      <span className="logs-session-top">
        <span
          className={`eyebrow ${session.live ? "live-eyebrow" : "finished-eyebrow"}`}
        >
          <i />
          {session.live ? "Live" : relativeDay(session.startedAtMs)}
        </span>
        <span className="logs-session-result">{rowResultText(session)}</span>
      </span>
      <strong className="logs-session-opponent">{sessionLabel(session)}</strong>
      <span className="logs-session-meta">
        {session.engineName}
        {session.clock ? ` · ${session.clock}` : ""}
        {session.color ? ` · ${titleCase(session.color)}` : ""}
      </span>
      <span className="logs-session-numbers">
        <span>{session.lineCount.toLocaleString()} lines</span>
        <span>{formatBytes(session.compressedBytes)}</span>
        {session.gameId && (
          <span className="logs-session-id">#{session.gameId}</span>
        )}
      </span>
      {hit && (
        <span className="logs-session-hit">
          <em>
            {hit.matchCount} match{hit.matchCount === 1 ? "" : "es"}
          </em>
          {hit.first && <code>{hit.first.text}</code>}
        </span>
      )}
    </button>
  );
}
