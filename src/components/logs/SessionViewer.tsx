import { useCallback, useEffect, useRef, useState } from "react";
import { ChevronDown, ChevronUp, Search, Trash2 } from "lucide-react";
import { LogViewer, type LogViewerHandle } from "../LogViewer";
import { formatBytes, relativeDay, titleCase } from "../../lib/format";
import type { BusyKeys, RunAction } from "../../hooks/useActionRunner";
import type { ShowNotice } from "../../hooks/useNotices";
import type {
  LogDirection,
  LogHeaderField,
  LogPage,
  LogSearchBlock,
  LogSessionSummary,
} from "../../types";
import { ConfirmDialog, Switch, TooltipButton } from "../../ui/primitives";
import { HeaderBlock } from "./HeaderBlock";
import { OutlineRail } from "./OutlineRail";
import { SessionExportMenu } from "./SessionExportMenu";
import { copyText, resultText, sessionLabel } from "./shared";
import type { LogsSource } from "./source";
import { useSessionSearch } from "./useSessionSearch";

/**
 * The viewer's row classes are absolutely positioned inside its canvas, so
 * the legend carries its own colour classes rather than reusing them.
 */
const LEGEND: Array<{
  direction: LogDirection;
  className: string;
  label: string;
}> = [
  { direction: ">", className: "logs-legend-sent", label: "Sent to engine" },
  { direction: "<", className: "logs-legend-received", label: "Received" },
  { direction: "!", className: "logs-legend-stderr", label: "Engine stderr" },
  { direction: "#", className: "logs-legend-note", label: "QueenUI note" },
];

/**
 * Reader for one session: header, outline rail, windowed lines, in-session
 * search, and the live tail. Mounted with `key={session.id}` so switching
 * sessions resets every piece of per-session state by remounting.
 */
export function SessionViewer({
  session,
  source,
  busy,
  runAction,
  showNotice,
  tailingAllowed,
  refreshToken,
  onDeleted,
}: {
  session: LogSessionSummary;
  source: LogsSource;
  busy: BusyKeys;
  runAction: RunAction;
  showNotice: ShowNotice;
  tailingAllowed: boolean;
  refreshToken: number;
  onDeleted: () => void;
}) {
  const [outline, setOutline] = useState<LogSearchBlock[]>([]);
  const [outlineFailed, setOutlineFailed] = useState(false);
  const [outlineToken, setOutlineToken] = useState(0);
  const [header, setHeader] = useState<LogHeaderField[]>([]);
  /** The count the viewer can actually read; null until the first page. */
  const [viewerTotal, setViewerTotal] = useState<number | null>(null);
  const [pageLive, setPageLive] = useState<boolean | null>(null);
  const [activeLine, setActiveLine] = useState<number | null>(null);
  const [follow, setFollow] = useState(true);

  const [confirmDelete, setConfirmDelete] = useState(false);

  const viewerRef = useRef<LogViewerHandle>(null);

  useEffect(() => {
    let stale = false;
    source
      .getOutline(session.id)
      .then((blocks) => {
        if (stale) return;
        setOutline(blocks);
        setOutlineFailed(false);
      })
      .catch((error) => {
        console.error("get_log_outline failed:", error);
        if (stale) return;
        // An unreadable outline is not an outline with nothing in it: the
        // rail says so rather than claiming the engine never searched.
        setOutline([]);
        setOutlineFailed(true);
      });
    return () => {
      stale = true;
    };
  }, [source, session.id, refreshToken, outlineToken]);

  const fetchPage = useCallback(
    (sessionId: string, offset: number, limit: number) =>
      source.getPage(sessionId, offset, limit),
    [source],
  );

  const handlePage = useCallback((page: LogPage) => {
    // Header fields are fixed for the life of a session: keep the first set
    // so a tail poll never re-renders the metadata block.
    setHeader((current) => (current.length > 0 ? current : page.header));
    setViewerTotal(page.totalLines);
    setPageLive(page.live);
  }, []);

  const jumpToLine = useCallback(
    (line: number, align: "start" | "center" = "start") => {
      setActiveLine(line);
      setFollow(false);
      viewerRef.current?.scrollToLine(line, align);
    },
    [],
  );

  const search = useSessionSearch({
    source,
    sessionId: session.id,
    onJump: jumpToLine,
  });

  const live = pageLive ?? session.live;
  // The summary counts lines written, a page counts lines the gzip can give
  // back; during a search the difference is the whole in-flight block.
  const readableLines = viewerTotal ?? session.lineCount;
  const unflushedLines = Math.max(0, session.lineCount - readableLines);
  const facts = [
    session.clock,
    session.color ? titleCase(session.color) : null,
    `${readableLines.toLocaleString()} lines`,
    unflushedLines > 0 ? `+${unflushedLines.toLocaleString()} unflushed` : null,
    formatBytes(session.compressedBytes),
    `started ${relativeDay(session.startedAtMs)}`,
    session.droppedLines > 0 ? `${session.droppedLines} dropped` : null,
  ]
    .filter(Boolean)
    .join(" · ");
  const factsTitle =
    unflushedLines > 0
      ? `${facts}\nThe recorder flushes once per completed move, so the lines of the search in progress cannot be read back yet.`
      : facts;

  async function deleteSession() {
    setConfirmDelete(false);
    const removed = await runAction(
      `log-delete-${session.id}`,
      () => source.deleteSession(session.id),
      "Log session deleted",
      "delete the log session",
    );
    if (removed) onDeleted();
  }

  async function copyHeader() {
    const copied = await copyText(
      header.map((field) => `${field.key}: ${field.value}`).join("\n"),
    );
    showNotice(
      copied ? "success" : "error",
      copied
        ? "Session header copied"
        : "The clipboard is not available in this window",
    );
  }

  return (
    <>
      <div className="panel-heading logs-viewer-head">
        <div className="logs-viewer-title">
          <span
            className={`eyebrow ${live ? "live-eyebrow" : "finished-eyebrow"}`}
          >
            <i />
            {live ? "Recording" : `Finished · ${resultText(session)}`}
          </span>
          <h2>
            {session.engineName} <span>vs.</span> {sessionLabel(session)}
          </h2>
          <p className="logs-viewer-facts" title={factsTitle}>
            {facts}
          </p>
        </div>
        <div className="logs-viewer-actions">
          <SessionExportMenu
            session={session}
            source={source}
            busy={busy}
            runAction={runAction}
            showNotice={showNotice}
          />
          <TooltipButton
            variant="icon"
            aria-label="Delete session"
            tooltip="Delete session"
            disabled={busy.has(`log-delete-${session.id}`)}
            onClick={() => setConfirmDelete(true)}
          >
            <Trash2 size={16} />
          </TooltipButton>
        </div>
      </div>

      <div className="logs-toolbar">
        <div className="input-wrap logs-search-input">
          <Search size={14} />
          <input
            aria-label="Search inside this session"
            placeholder="Find in session — Enter for next, Shift+Enter for previous"
            value={search.queryText}
            onChange={(event) => search.setQueryText(event.target.value)}
            onKeyDown={search.onKeyDown}
          />
        </div>
        <div className="logs-search-toggles">
          <button
            type="button"
            className={search.regex ? "selected" : ""}
            aria-pressed={search.regex}
            aria-label="Regular expression"
            title="Regular expression"
            onClick={() => search.setRegex((value) => !value)}
          >
            .*
          </button>
          <button
            type="button"
            className={search.caseSensitive ? "selected" : ""}
            aria-pressed={search.caseSensitive}
            aria-label="Match case"
            title="Match case"
            onClick={() => search.setCaseSensitive((value) => !value)}
          >
            Aa
          </button>
        </div>
        <span
          className={`logs-search-count${search.searchError ? " logs-search-failed" : ""}`}
          role="status"
          title={search.searchError ?? undefined}
        >
          {search.matchSummary}
        </span>
        <TooltipButton
          variant="icon"
          aria-label="Previous match"
          tooltip="Previous match (Shift+Enter)"
          disabled={!search.hasMatches}
          onClick={() => search.stepMatch(-1)}
        >
          <ChevronUp size={16} />
        </TooltipButton>
        <TooltipButton
          variant="icon"
          aria-label="Next match"
          tooltip="Next match (Enter)"
          disabled={!search.hasMatches}
          onClick={() => search.stepMatch(1)}
        >
          <ChevronDown size={16} />
        </TooltipButton>
        {live && (
          <label className="logs-follow">
            <Switch
              checked={follow}
              aria-label="Follow live output"
              onCheckedChange={(checked) => {
                setFollow(checked);
                if (checked) viewerRef.current?.scrollToEnd();
              }}
            />
            <span>Follow</span>
          </label>
        )}
      </div>

      {header.length > 0 && (
        <HeaderBlock fields={header} onCopy={() => void copyHeader()} />
      )}

      <div className="logs-viewer-body">
        <OutlineRail
          blocks={outline}
          activeLine={activeLine}
          failed={outlineFailed}
          onJump={(line) => jumpToLine(line)}
          onRetry={() => setOutlineToken((token) => token + 1)}
        />
        <div className="logs-lines">
          <LogViewer
            ref={viewerRef}
            sessionId={session.id}
            totalLines={session.lineCount}
            fetchPage={fetchPage}
            onPage={handlePage}
            tailing={live && tailingAllowed}
            follow={live && follow}
            onFollowOff={() => setFollow(false)}
            activeLine={activeLine}
            highlight={search.highlight}
            label={`Engine log for ${sessionLabel(session)}`}
          />
          <div className="logs-legend">
            {LEGEND.map((entry) => (
              <span
                className={`logs-legend-item ${entry.className}`}
                key={entry.direction}
              >
                <b>{entry.direction}</b>
                {entry.label}
              </span>
            ))}
          </div>
        </div>
      </div>

      <ConfirmDialog
        open={confirmDelete}
        title="Delete this log session?"
        description={`The recorded conversation for ${sessionLabel(session)} is removed from disk. This cannot be undone.`}
        confirmLabel="Delete session"
        pending={busy.has(`log-delete-${session.id}`)}
        onCancel={() => setConfirmDelete(false)}
        onConfirm={() => void deleteSession()}
      />
    </>
  );
}
