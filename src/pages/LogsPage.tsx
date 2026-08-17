import { useEffect, useMemo, useRef, useState } from "react";
import { Search, ServerCog, TerminalSquare, Trash2 } from "lucide-react";
import { DiagnosticsPanel } from "../components/logs/DiagnosticsPanel";
import { SessionRow } from "../components/logs/SessionRow";
import { SessionViewer } from "../components/logs/SessionViewer";
import {
  MATCH_LIMIT,
  sessionLabel,
  useDebounced,
} from "../components/logs/shared";
import { backendSource, type LogsSource } from "../components/logs/source";
import { formatBytes, relativeDay } from "../lib/format";
import type { BusyKeys, RunAction } from "../hooks/useActionRunner";
import type { ShowNotice } from "../hooks/useNotices";
import type {
  AppSnapshot,
  LogFilter,
  LogSessionMatches,
  LogSessionSummary,
  LogsOverview,
} from "../types";
import {
  Button,
  ConfirmDialog,
  SelectField,
  Switch,
  Tabs,
} from "../ui/primitives";

export type { LogsSource };

const SESSION_LIMIT = 200;

const NO_SESSIONS: LogSessionSummary[] = [];
const NO_HITS: ReadonlyMap<string, LogSessionMatches> = new Map();

function useDocumentVisible() {
  const [visible, setVisible] = useState(
    () => document.visibilityState !== "hidden",
  );
  useEffect(() => {
    const update = () => setVisible(document.visibilityState !== "hidden");
    document.addEventListener("visibilitychange", update);
    return () => document.removeEventListener("visibilitychange", update);
  }, []);
  return visible;
}

export function LogsPage({
  snapshot,
  busy,
  runAction,
  showNotice,
  source = backendSource,
}: {
  snapshot: AppSnapshot;
  busy: BusyKeys;
  runAction: RunAction;
  showNotice: ShowNotice;
  source?: LogsSource;
}) {
  const [tab, setTab] = useState("sessions");
  const [accountId, setAccountId] = useState("");
  const [engineId, setEngineId] = useState("");
  const [text, setText] = useState("");
  const debouncedText = useDebounced(text);
  const [deepSearch, setDeepSearch] = useState(false);
  const [sessions, setSessions] = useState<LogSessionSummary[]>(NO_SESSIONS);
  const [hits, setHits] =
    useState<ReadonlyMap<string, LogSessionMatches>>(NO_HITS);
  const [loadedKey, setLoadedKey] = useState("");
  const [listFailed, setListFailed] = useState(false);
  const [overview, setOverview] = useState<LogsOverview | null>(null);
  const [overviewFailed, setOverviewFailed] = useState(false);
  /**
   * The whole summary, not just the id: a session the current filter no
   * longer lists must stay open rather than handing the pane to another game.
   */
  const [selection, setSelection] = useState<LogSessionSummary | null>(null);
  const [refreshToken, setRefreshToken] = useState(0);
  const [confirmClearAll, setConfirmClearAll] = useState(false);
  const documentVisible = useDocumentVisible();
  /** Where focus goes when the open session's own delete button unmounts. */
  const listRef = useRef<HTMLDivElement>(null);

  const deepText = debouncedText.trim();
  const filter = useMemo<LogFilter>(
    () => ({
      accountId: accountId || null,
      engineId: engineId || null,
      fromMs: null,
      toMs: null,
      query: deepSearch ? null : deepText || null,
      limit: SESSION_LIMIT,
    }),
    [accountId, engineId, deepSearch, deepText],
  );

  const requestKey = useMemo(
    () =>
      `${refreshToken}:${deepSearch ? deepText : ""}:${JSON.stringify(filter)}`,
    [filter, deepSearch, deepText, refreshToken],
  );
  const listLoading = loadedKey !== requestKey;

  useEffect(() => {
    let stale = false;
    const request: Promise<{
      rows: LogSessionSummary[];
      hits: ReadonlyMap<string, LogSessionMatches>;
    }> =
      deepSearch && deepText
        ? source
            .searchSessions(filter, {
              text: deepText,
              regex: false,
              caseSensitive: false,
              limit: MATCH_LIMIT,
            })
            .then((found) => ({
              rows: found.map((entry) => entry.session),
              hits: new Map(found.map((entry) => [entry.session.id, entry])),
            }))
        : source.listSessions(filter).then((rows) => ({ rows, hits: NO_HITS }));
    request
      .then((result) => {
        if (stale) return;
        setSessions(result.rows);
        setHits(result.hits);
        // Auto-selection only fills an empty slot. Re-deriving it on every
        // list change is what let a filter keystroke — or, in production, a
        // new game arriving at the top — take over the reader.
        setSelection((current) => current ?? result.rows[0] ?? null);
        setListFailed(false);
        setLoadedKey(requestKey);
      })
      .catch((error) => {
        console.error("list_log_sessions failed:", error);
        if (stale) return;
        setListFailed(true);
        setLoadedKey(requestKey);
      });
    return () => {
      stale = true;
    };
  }, [source, filter, deepSearch, deepText, requestKey]);

  useEffect(() => {
    let stale = false;
    source
      .getOverview()
      .then((value) => {
        if (stale) return;
        setOverview(value);
        setOverviewFailed(false);
      })
      .catch((error) => {
        console.error("get_logs_overview failed:", error);
        // Swallowing this deleted the whole stats strip, which reads as
        // "nothing is recorded" rather than "the call failed".
        if (!stale) setOverviewFailed(true);
      });
    return () => {
      stale = true;
    };
  }, [source, refreshToken]);

  useEffect(
    () => source.subscribeLogs(() => setRefreshToken((token) => token + 1)),
    [source],
  );

  // Prefer the freshest summary for the open session; fall back to the last
  // one seen so a filter that excludes it does not close it.
  const selected = useMemo(() => {
    if (!selection) return null;
    return sessions.find((row) => row.id === selection.id) ?? selection;
  }, [sessions, selection]);

  const selectionHidden =
    selection != null && !sessions.some((row) => row.id === selection.id);

  function clearFilters() {
    setAccountId("");
    setEngineId("");
    setText("");
    setDeepSearch(false);
  }

  function returnFocusToList() {
    // The delete button unmounts with the viewer, so Radix has nothing to
    // restore focus to; the session list is the stable landing place.
    window.setTimeout(() => listRef.current?.focus(), 0);
  }

  async function clearAllSessions() {
    setConfirmClearAll(false);
    const cleared = await runAction(
      "logs-clear",
      () => source.clearSessions(),
      "All recorded sessions deleted",
      "delete the recorded sessions",
    );
    if (cleared) {
      setSessions(NO_SESSIONS);
      setSelection(null);
      setRefreshToken((token) => token + 1);
      returnFocusToList();
    }
  }

  const listEmptyTitle = listLoading
    ? "Loading sessions…"
    : listFailed
      ? "The log service didn’t answer"
      : deepSearch && deepText
        ? "No session contains that text"
        : text || accountId || engineId
          ? "No session matches these filters"
          : "Nothing recorded yet";

  return (
    <div className="module-content logs-page">
      <section className="module-hero logs-hero">
        <div>
          <span className="eyebrow">Flight recorder</span>
          <h2>Logs</h2>
          <p>
            The complete UCI conversation of every game, plus QueenUI&rsquo;s
            own operational record.
          </p>
        </div>
        <div className="logs-hero-actions">
          <Button
            variant="ghost"
            className="text-[#dd7a6f] hover:bg-[rgba(221,122,111,.1)] hover:text-[#e28d84]"
            disabled={busy.has("logs-clear") || sessions.length === 0}
            onClick={() => setConfirmClearAll(true)}
          >
            <Trash2 size={14} />
            Delete all sessions
          </Button>
        </div>
      </section>

      {overview && (
        <section className="logs-overview" aria-label="Recording summary">
          <div>
            <span>Sessions</span>
            <strong>{overview.sessionCount.toLocaleString()}</strong>
          </div>
          <div>
            <span>On disk</span>
            <strong>{formatBytes(overview.compressedBytes)}</strong>
          </div>
          <div>
            <span>Uncompressed</span>
            <strong>{formatBytes(overview.rawBytes)}</strong>
          </div>
          <div>
            <span>Recording now</span>
            <strong className={overview.liveCount > 0 ? "logs-stat-live" : ""}>
              {overview.liveCount}
            </strong>
          </div>
          <div>
            <span>Oldest kept</span>
            <strong>
              {overview.oldestStartedAtMs
                ? relativeDay(overview.oldestStartedAtMs)
                : "—"}
            </strong>
          </div>
          <div>
            <span>Retention</span>
            {/* Same two numbers the Settings panel edits, in the units it
                edits them in — this strip used to render "2048 MB · 30 d"
                for a limit set as "2.0 GB" / "30 days". */}
            <strong>
              {(overview.retention.maxTotalMb / 1024).toFixed(1)} GB ·{" "}
              {overview.retention.maxAgeDays} day
              {overview.retention.maxAgeDays === 1 ? "" : "s"}
            </strong>
          </div>
        </section>
      )}

      {!overview && overviewFailed && (
        <section className="logs-overview-failed" role="status">
          <span>The recording summary couldn&rsquo;t be read.</span>
          <button
            type="button"
            onClick={() => setRefreshToken((token) => token + 1)}
          >
            Retry
          </button>
        </section>
      )}

      <Tabs.Root value={tab} onValueChange={setTab} className="logs-tabs">
        <Tabs.List
          className="config-tab-list logs-tab-list"
          aria-label="Log views"
        >
          <Tabs.Trigger value="sessions">
            <TerminalSquare size={15} /> Engine sessions
            <span>{sessions.length}</span>
          </Tabs.Trigger>
          <Tabs.Trigger value="diagnostics">
            <ServerCog size={15} /> App diagnostics
          </Tabs.Trigger>
        </Tabs.List>

        <Tabs.Content value="sessions" className="logs-tab-content">
          <div className="logs-split">
            <aside className="panel logs-list-pane">
              <div className="logs-filters">
                <SelectField
                  label="Filter sessions by account"
                  value={accountId}
                  onChange={setAccountId}
                >
                  <option value="">All accounts</option>
                  {snapshot.accounts.map((account) => (
                    <option value={account.id} key={account.id}>
                      {account.username}
                    </option>
                  ))}
                </SelectField>
                <SelectField
                  label="Filter sessions by engine"
                  value={engineId}
                  onChange={setEngineId}
                >
                  <option value="">All engines</option>
                  {snapshot.engines.map((engine) => (
                    <option value={engine.id} key={engine.id}>
                      {engine.name}
                    </option>
                  ))}
                </SelectField>
                <div className="input-wrap logs-filter-query">
                  <Search size={14} />
                  <input
                    aria-label="Filter sessions"
                    placeholder={
                      deepSearch
                        ? "Text to find inside every session"
                        : "Opponent, game id, or engine"
                    }
                    value={text}
                    onChange={(event) => setText(event.target.value)}
                  />
                </div>
                <label className="logs-deep-toggle">
                  <Switch
                    checked={deepSearch}
                    aria-label="Search inside logs"
                    onCheckedChange={setDeepSearch}
                  />
                  <span>Search inside logs</span>
                </label>
              </div>

              {/* `group` names the region for assistive tech — an aria-label
                  on a role-less div is dropped — and `tabIndex` gives the
                  delete flow somewhere to put focus. */}
              <div
                className="logs-session-list"
                role="group"
                aria-label="Recorded sessions"
                tabIndex={-1}
                ref={listRef}
              >
                {sessions.map((session) => (
                  <SessionRow
                    session={session}
                    hit={hits.get(session.id)}
                    selected={session.id === selected?.id}
                    onSelect={() => setSelection(session)}
                    key={session.id}
                  />
                ))}
                {/* The title cycles through loading / service-failed /
                    no-match as the operator types, so it is announced —
                    the sibling failure banner already is. */}
                {sessions.length === 0 && (
                  <div className="logs-list-empty" role="status">
                    <span>
                      <TerminalSquare size={19} />
                    </span>
                    <div>
                      <strong>{listEmptyTitle}</strong>
                      <small>
                        {listFailed
                          ? "Check that the QueenUI backend is running, then retry."
                          : "A session is written for every game your bots play."}
                      </small>
                    </div>
                    {listFailed && (
                      <button
                        type="button"
                        onClick={() => setRefreshToken((token) => token + 1)}
                      >
                        Retry
                      </button>
                    )}
                  </div>
                )}
              </div>
            </aside>

            <section className="panel logs-viewer-pane">
              {selectionHidden && selected && (
                <p className="logs-selection-note" role="status">
                  <span>
                    {sessionLabel(selected)} stays open even though the list
                    doesn&rsquo;t show it.
                  </span>
                  <button type="button" onClick={clearFilters}>
                    Clear filters
                  </button>
                </p>
              )}
              {selected ? (
                <SessionViewer
                  key={selected.id}
                  session={selected}
                  source={source}
                  busy={busy}
                  runAction={runAction}
                  showNotice={showNotice}
                  tailingAllowed={tab === "sessions" && documentVisible}
                  refreshToken={refreshToken}
                  onDeleted={() => {
                    const remaining = sessions.filter(
                      (row) => row.id !== selected.id,
                    );
                    setSessions(remaining);
                    setSelection(remaining[0] ?? null);
                    setRefreshToken((token) => token + 1);
                    returnFocusToList();
                  }}
                />
              ) : (
                <div className="logs-viewer-empty">
                  <div className="empty-icon">
                    <TerminalSquare />
                  </div>
                  <h2>
                    {listLoading ? "Loading sessions…" : "No session selected"}
                  </h2>
                  <p>
                    QueenUI records every command it sends, every line the
                    engine answers with, and everything the engine writes to
                    stderr. Pick a session to read it.
                  </p>
                </div>
              )}
            </section>
          </div>
        </Tabs.Content>

        <Tabs.Content value="diagnostics" className="logs-tab-content">
          <DiagnosticsPanel
            snapshot={snapshot}
            source={source}
            busy={busy}
            runAction={runAction}
            showNotice={showNotice}
          />
        </Tabs.Content>
      </Tabs.Root>

      <ConfirmDialog
        open={confirmClearAll}
        title="Delete every recorded session?"
        description="All engine session files are removed from disk. Diagnostics and game history are not affected."
        confirmLabel="Delete all sessions"
        pending={busy.has("logs-clear")}
        onCancel={() => setConfirmClearAll(false)}
        onConfirm={() => void clearAllSessions()}
      />
    </div>
  );
}
