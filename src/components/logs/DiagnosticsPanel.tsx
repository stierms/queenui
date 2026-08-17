import { useEffect, useMemo, useRef, useState } from "react";
import {
  ChevronDown,
  ChevronUp,
  ClipboardCopy,
  Search,
  Trash2,
} from "lucide-react";
import { timeOfDay } from "../../lib/format";
import type { BusyKeys, RunAction } from "../../hooks/useActionRunner";
import type { ShowNotice } from "../../hooks/useNotices";
import {
  diagnosticLevel,
  type AppSnapshot,
  type DiagnosticEntry,
  type DiagnosticFilter,
  type DiagnosticLevel,
} from "../../types";
import { Button, ConfirmDialog, SelectField } from "../../ui/primitives";
import { copyText, useDebounced } from "./shared";
import type { LogsSource } from "./source";

const DIAGNOSTIC_LIMIT = 1000;

const LEVEL_OPTIONS: Array<{ value: DiagnosticLevel; label: string }> = [
  { value: "info", label: "Info and above" },
  { value: "warn", label: "Warnings and above" },
  { value: "error", label: "Errors only" },
];

/** The backend ranks levels the same way; "warn" means warn *and* error. */
const LEVEL_RANK: Record<DiagnosticLevel, number> = {
  info: 0,
  warn: 1,
  error: 2,
};

/**
 * Live entries that landed while a refetch was in flight are not in the
 * answer, so they are folded back in rather than overwritten. Both lists are
 * newest first.
 */
function mergeDiagnostics(live: DiagnosticEntry[], rows: DiagnosticEntry[]) {
  if (live.length === 0) return rows;
  const known = new Set(rows.map((row) => row.id));
  const extra = live.filter((entry) => !known.has(entry.id));
  if (extra.length === 0) return rows;
  return [...extra, ...rows].slice(0, DIAGNOSTIC_LIMIT);
}

/** Mirrors the backend's minimum-level + substring rules for live events. */
function matchesDiagnostic(entry: DiagnosticEntry, filter: DiagnosticFilter) {
  // Both levels are `string` on the wire; narrowed here so the rank table
  // stays total and an unrecognised level is ranked, never dropped.
  if (
    filter.level &&
    LEVEL_RANK[diagnosticLevel(entry.level)] <
      LEVEL_RANK[diagnosticLevel(filter.level)]
  ) {
    return false;
  }
  if (filter.source && entry.source !== filter.source) return false;
  if (filter.accountId && entry.accountId !== filter.accountId) return false;
  if (filter.query) {
    const haystack =
      `${entry.source} ${entry.message} ${entry.detail ?? ""}`.toLowerCase();
    if (!haystack.includes(filter.query.toLowerCase())) return false;
  }
  return true;
}

/** QueenUI's own operational record: filters, live tail, copy and clear. */
export function DiagnosticsPanel({
  snapshot,
  source,
  busy,
  runAction,
  showNotice,
}: {
  snapshot: AppSnapshot;
  source: LogsSource;
  busy: BusyKeys;
  runAction: RunAction;
  showNotice: ShowNotice;
}) {
  const [level, setLevel] = useState<DiagnosticLevel>("info");
  const [sourceName, setSourceName] = useState("");
  const [accountId, setAccountId] = useState("");
  const [text, setText] = useState("");
  const debouncedText = useDebounced(text);
  const [entries, setEntries] = useState<DiagnosticEntry[]>([]);
  const [knownSources, setKnownSources] = useState<string[]>([]);
  const [loadedKey, setLoadedKey] = useState("");
  const [failed, setFailed] = useState(false);
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(
    () => new Set(),
  );
  const [confirmClear, setConfirmClear] = useState(false);
  const [refreshToken, setRefreshToken] = useState(0);

  const filter = useMemo<DiagnosticFilter>(
    () => ({
      level,
      source: sourceName || null,
      accountId: accountId || null,
      query: debouncedText.trim() || null,
      limit: DIAGNOSTIC_LIMIT,
    }),
    [level, sourceName, accountId, debouncedText],
  );

  // Loading is derived rather than a flag flipped inside the effect body.
  const requestKey = useMemo(
    () => `${refreshToken}:${JSON.stringify(filter)}`,
    [filter, refreshToken],
  );
  const loading = loadedKey !== requestKey;

  // The live subscription is registered once per source; reading the filter
  // through a ref keeps a re-filter from tearing the listener down and
  // losing every event that arrives in the gap.
  const filterRef = useRef(filter);
  useEffect(() => {
    filterRef.current = filter;
  }, [filter]);
  const liveDuringRequestRef = useRef<DiagnosticEntry[]>([]);

  useEffect(() => {
    let stale = false;
    liveDuringRequestRef.current = [];
    source
      .getDiagnostics(filter)
      .then((rows) => {
        if (stale) return;
        setEntries(mergeDiagnostics(liveDuringRequestRef.current, rows));
        setKnownSources((current) => {
          const merged = new Set(current);
          for (const row of rows) merged.add(row.source);
          return merged.size === current.length
            ? current
            : Array.from(merged).sort();
        });
        setFailed(false);
        setLoadedKey(requestKey);
      })
      .catch((error) => {
        console.error("get_diagnostics failed:", error);
        if (stale) return;
        setFailed(true);
        setLoadedKey(requestKey);
      });
    return () => {
      stale = true;
    };
  }, [source, filter, requestKey]);

  useEffect(
    () =>
      source.subscribeDiagnostics((entry) => {
        setKnownSources((current) =>
          current.includes(entry.source)
            ? current
            : [...current, entry.source].sort(),
        );
        if (!matchesDiagnostic(entry, filterRef.current)) return;
        const pending = liveDuringRequestRef.current;
        pending.unshift(entry);
        if (pending.length > DIAGNOSTIC_LIMIT)
          pending.length = DIAGNOSTIC_LIMIT;
        setEntries((current) =>
          current.some((row) => row.id === entry.id)
            ? current
            : [entry, ...current].slice(0, DIAGNOSTIC_LIMIT),
        );
      }),
    [source],
  );

  function toggleDetail(id: string) {
    setExpanded((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }

  async function copyAll() {
    const body = entries
      .map(
        (entry) =>
          `${timeOfDay(entry.atMs)}  ${entry.level.toUpperCase().padEnd(5)} ${entry.source}  ${entry.message}${entry.detail ? `\n        ${entry.detail}` : ""}`,
      )
      .join("\n");
    const copied = await copyText(body);
    showNotice(
      copied ? "success" : "error",
      copied
        ? `Copied ${entries.length} diagnostic${entries.length === 1 ? "" : "s"}`
        : "The clipboard is not available in this window",
    );
  }

  async function clearAll() {
    setConfirmClear(false);
    const cleared = await runAction(
      "diagnostics-clear",
      () => source.clearDiagnostics(),
      "Diagnostics cleared",
      "clear the diagnostics",
    );
    if (!cleared) return;
    // Sources are only learned from what has been recorded, so the record
    // going away has to take the dropdown with it — otherwise it keeps
    // offering filters that can only ever come back empty.
    setEntries([]);
    setKnownSources([]);
    setSourceName("");
    setRefreshToken((token) => token + 1);
  }

  return (
    <div className="panel logs-diagnostics">
      <div className="panel-heading logs-diag-heading">
        <div>
          <span className="eyebrow">Operational record</span>
          <h2>App diagnostics</h2>
        </div>
        <div className="logs-diag-actions">
          <Button
            variant="secondary"
            disabled={entries.length === 0}
            onClick={() => void copyAll()}
          >
            <ClipboardCopy size={14} />
            Copy all
          </Button>
          <Button
            variant="ghost"
            className="text-[#dd7a6f] hover:bg-[rgba(221,122,111,.1)] hover:text-[#e28d84]"
            disabled={busy.has("diagnostics-clear")}
            onClick={() => setConfirmClear(true)}
          >
            <Trash2 size={14} />
            Clear
          </Button>
        </div>
      </div>

      <div className="logs-diag-filters">
        <SelectField
          label="Minimum level"
          value={level}
          onChange={(value) => setLevel(value as DiagnosticLevel)}
        >
          {LEVEL_OPTIONS.map((option) => (
            <option value={option.value} key={option.value}>
              {option.label}
            </option>
          ))}
        </SelectField>
        <SelectField
          label="Filter by source"
          value={sourceName}
          onChange={setSourceName}
        >
          <option value="">All sources</option>
          {knownSources.map((name) => (
            <option value={name} key={name}>
              {name}
            </option>
          ))}
        </SelectField>
        <SelectField
          label="Filter diagnostics by account"
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
        <div className="input-wrap logs-diag-query">
          <Search size={14} />
          <input
            aria-label="Search diagnostics"
            placeholder="Message or detail"
            value={text}
            onChange={(event) => setText(event.target.value)}
          />
        </div>
      </div>

      {/* `group` rather than a bare div: an aria-label on a role-less
          element is dropped, so the region had no name at all. */}
      <div
        className="logs-diag-list"
        role="group"
        aria-label="Diagnostic entries"
      >
        {entries.map((entry) => {
          const open = expanded.has(entry.id);
          return (
            <div
              className={`logs-diag-row logs-diag-${entry.level}`}
              key={entry.id}
            >
              <button
                type="button"
                className="logs-diag-main"
                aria-expanded={entry.detail ? open : undefined}
                onClick={() => entry.detail && toggleDetail(entry.id)}
              >
                <span className="logs-diag-time">{timeOfDay(entry.atMs)}</span>
                <span className="logs-diag-level">{entry.level}</span>
                <span className="logs-diag-source">{entry.source}</span>
                <span className="logs-diag-message">{entry.message}</span>
                <span className="logs-diag-caret">
                  {entry.detail ? (
                    open ? (
                      <ChevronUp size={14} />
                    ) : (
                      <ChevronDown size={14} />
                    )
                  ) : null}
                </span>
              </button>
              {open && entry.detail && (
                <pre className="logs-diag-detail">{entry.detail}</pre>
              )}
            </div>
          );
        })}
        {/*
         * Announced, because which of the three states this is decides
         * whether "nothing here" means anything. The failure arm also gets a
         * way out: every other failure surface on this page offers a retry,
         * and this one already owns the token that performs one.
         */}
        {entries.length === 0 && (
          <p className="logs-diag-empty" role="status">
            {loading
              ? "Loading diagnostics…"
              : failed
                ? "The diagnostics service didn’t answer, so this list is not the record — it is empty because the read failed."
                : "Nothing recorded — QueenUI has had a quiet run."}
            {!loading && failed && (
              <Button
                variant="secondary"
                onClick={() => setRefreshToken((token) => token + 1)}
              >
                Try again
              </Button>
            )}
          </p>
        )}
      </div>

      <ConfirmDialog
        open={confirmClear}
        title="Clear all diagnostics?"
        description="Every recorded note, warning, and error is discarded. Engine session logs are not affected."
        confirmLabel="Clear diagnostics"
        pending={busy.has("diagnostics-clear")}
        onCancel={() => setConfirmClear(false)}
        onConfirm={() => void clearAll()}
      />
    </div>
  );
}
