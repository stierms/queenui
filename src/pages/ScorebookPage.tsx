import { useEffect, useState } from "react";
import { BookOpen, Download } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import * as commands from "../api/commands";
import { onHistoryUpdated } from "../api/events";
import { type TimeSelection } from "../components/charts";
import { bucketEndMs } from "../lib/buckets";
import { ActivityPanel } from "../components/scorebook/ActivityPanel";
import {
  ByColorPanel,
  OpponentStrengthPanel,
  TerminationsPanel,
} from "../components/scorebook/Breakdowns";
import { EngineTable } from "../components/scorebook/EngineTable";
import { LabPanel } from "../components/scorebook/LabPanel";
import { RatingPanel } from "../components/scorebook/RatingPanel";
import {
  OpeningsPanel,
  TopOpponentsPanel,
} from "../components/scorebook/RivalsAndOpenings";
import { ScorebookHero } from "../components/scorebook/ScorebookHero";
import { SummaryKpis } from "../components/scorebook/SummaryKpis";
import { errorText } from "../lib/errors";
import type { BusyKeys, RunAction } from "../hooks/useActionRunner";
import type { ShowNotice } from "../hooks/useNotices";
import {
  activityBucket,
  type ImportReport,
  type ScorebookStats,
} from "../types";
import { Button } from "../ui/primitives";

const IMPORT_KEY = "scorebook-import";

export function ScorebookPage({
  busy,
  runAction,
  showNotice,
}: {
  busy: BusyKeys;
  runAction: RunAction;
  showNotice: ShowNotice;
}) {
  const [accountId, setAccountId] = useState("");
  const [engineId, setEngineId] = useState("");
  const [perf, setPerf] = useState("");
  const [timeRange, setTimeRange] = useState<TimeSelection | null>(null);
  const [stats, setStats] = useState<ScorebookStats | null>(null);
  const [failed, setFailed] = useState(false);
  const [refreshToken, setRefreshToken] = useState(0);

  useEffect(() => {
    let stale = false;
    commands
      .getScorebookStats({
        accountId: accountId || null,
        engineId: engineId || null,
        perf: perf || null,
        fromMs: timeRange?.fromMs ?? null,
        toMs: timeRange?.toMs ?? null,
      })
      .then((value) => {
        if (stale) return;
        setStats(value);
        setFailed(false);
        // The selection survives account/engine/perf changes, but a history
        // whose full span no longer overlaps it can't support it — clear.
        if (timeRange) {
          const span = value.activity;
          const overlaps =
            span.length > 0 &&
            timeRange.fromMs <=
              bucketEndMs(
                span[span.length - 1].dayStartMs,
                activityBucket(value.activityBucket),
              ) &&
            timeRange.toMs >= span[0].dayStartMs;
          if (!overlaps) setTimeRange(null);
        }
      })
      .catch((error) => {
        console.error("get_scorebook_stats failed:", error);
        if (!stale) setFailed(true);
      });
    return () => {
      stale = true;
    };
  }, [accountId, engineId, perf, timeRange, refreshToken]);

  useEffect(
    () => onHistoryUpdated(() => setRefreshToken((token) => token + 1)),
    [],
  );

  const importing = busy.has(IMPORT_KEY);
  const importAccountId = accountId || (stats?.accounts[0]?.id ?? "");

  async function runImport() {
    if (!importAccountId || importing) return;
    await runAction(
      IMPORT_KEY,
      async () => {
        const report: ImportReport =
          await commands.importLichessHistory(importAccountId);
        showNotice(
          "success",
          `Imported ${report.imported} game${report.imported === 1 ? "" : "s"} from Lichess (${report.skipped} already recorded)`,
        );
        setRefreshToken((token) => token + 1);
      },
      undefined,
      "import the Lichess history",
    );
  }

  /**
   * Open the game on Lichess via the system browser. A missing or refused
   * opener used to be swallowed, leaving a chip that silently did nothing.
   */
  async function openLichessGame(id: string) {
    try {
      await openUrl(`https://lichess.org/${id}`);
    } catch (cause) {
      showNotice(
        "error",
        `lichess.org/${id} could not be opened — ${errorText(cause)}`,
      );
    }
  }

  if (!stats && !failed) {
    return <div className="app-loading">Loading the scorebook…</div>;
  }

  if (!stats) {
    return (
      <div className="module-content scorebook">
        <section className="panel empty-view">
          <div className="empty-icon">
            <BookOpen />
          </div>
          <h2>Scorebook unavailable</h2>
          <p>
            The game history service didn&rsquo;t answer. Check that the QueenUI
            backend is running, then try again.
          </p>
          <Button
            variant="primary"
            onClick={() => setRefreshToken((token) => token + 1)}
          >
            Retry
          </Button>
        </section>
      </div>
    );
  }

  const importButton = (variant: "primary" | "secondary") => (
    <Button
      variant={variant}
      disabled={importing || !importAccountId}
      onClick={() => void runImport()}
    >
      <Download size={15} />
      {importing ? "Importing…" : "Import from Lichess"}
    </Button>
  );

  const openings = stats.openings;
  // `activityBucket` is `string` in the generated contract; narrowed once.
  const bucket = activityBucket(stats.activityBucket);

  return (
    <div className="module-content scorebook">
      <ScorebookHero
        accounts={stats.accounts}
        engines={stats.engines}
        accountId={accountId}
        engineId={engineId}
        perf={perf}
        onAccountChange={setAccountId}
        onEngineChange={setEngineId}
        onPerfChange={setPerf}
        importButton={importButton("secondary")}
      />

      {stats.totalGames === 0 ? (
        <section className="panel empty-view">
          <div className="empty-icon">
            <BookOpen />
          </div>
          <h2>Your scorebook is empty</h2>
          <p>
            Finished games are recorded automatically — or pull your bot&rsquo;s
            existing history from Lichess.
          </p>
          {importButton("primary")}
        </section>
      ) : (
        <>
          <SummaryKpis stats={stats} />

          <div className="scorebook-grid">
            <div className="scorebook-column">
              <ActivityPanel
                days={stats.activity}
                bucket={bucket}
                selection={timeRange}
                onSelect={setTimeRange}
              />
              <RatingPanel points={stats.ratingSeries} />
            </div>

            <div className="scorebook-column">
              <OpponentStrengthPanel bands={stats.byOpponentBand} />
              <TerminationsPanel
                terminations={stats.byTermination}
                timeLosses={stats.timeLosses}
              />
              <ByColorPanel byColor={stats.byColor} />
            </div>
          </div>

          <EngineTable rows={stats.byEngine} />

          <div className="scorebook-two-up">
            <TopOpponentsPanel opponents={stats.topOpponents} />
            <OpeningsPanel openings={openings} />
          </div>

          <LabPanel
            lab={stats.lab ?? null}
            onOpenGame={(id) => void openLichessGame(id)}
          />
        </>
      )}
    </div>
  );
}
