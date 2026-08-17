import { save } from "@tauri-apps/plugin-dialog";
import { ChevronDown, Download } from "lucide-react";
import { errorText } from "../../lib/errors";
import type { BusyKeys, RunAction } from "../../hooks/useActionRunner";
import type { ShowNotice } from "../../hooks/useNotices";
import type { ExportMode, LogSessionSummary } from "../../types";
import { Button, Popover } from "../../ui/primitives";
import { sessionLabel } from "./shared";
import type { LogsSource } from "./source";

/**
 * `suffix` is the suggested file name ending; `extension` is what the save
 * dialog filters on. Sessions are stored as `<id>.uci.gz` (see docs/logs.md),
 * so the archive keeps that shape.
 */
const EXPORT_MODES: Array<{
  value: ExportMode;
  label: string;
  hint: string;
  suffix: string;
  extension: string;
}> = [
  {
    value: "annotated",
    label: "Annotated",
    hint: "Timestamps, direction markers, and the session header.",
    suffix: "log",
    extension: "log",
  },
  {
    value: "plain",
    label: "Plain UCI transcript",
    hint: "The command stream alone — replayable against any engine.",
    suffix: "uci",
    extension: "uci",
  },
  {
    value: "archive",
    label: "Compressed archive",
    hint: "The recorded file exactly as it sits on disk.",
    suffix: "uci.gz",
    extension: "gz",
  },
];

/** Export-format menu for one session, and the save dialog behind it. */
export function SessionExportMenu({
  session,
  source,
  busy,
  runAction,
  showNotice,
}: {
  session: LogSessionSummary;
  source: LogsSource;
  /**
   * Exporting decompresses and rewrites the whole recording, which is not
   * instant on a long session. Without this the menu looked inert until the
   * success toast arrived, and nothing stopped a second click starting it
   * again.
   */
  busy: BusyKeys;
  runAction: RunAction;
  showNotice: ShowNotice;
}) {
  const exporting = busy.has(`log-export-${session.id}`);
  async function exportSession(mode: ExportMode) {
    const config =
      EXPORT_MODES.find((entry) => entry.value === mode) ?? EXPORT_MODES[0];
    const safeName = sessionLabel(session).replace(/[^a-zA-Z0-9_-]+/g, "_");
    let path: string | null;
    try {
      path = await save({
        defaultPath: `${session.botUsername}_vs_${safeName}_${session.gameId ?? session.id}.${config.suffix}`,
        filters: [{ name: config.label, extensions: [config.extension] }],
      });
    } catch (error) {
      // A rejected dialog used to escape as an unhandled rejection, leaving
      // the operator with a menu that quietly did nothing.
      console.error("save dialog failed:", error);
      showNotice(
        "error",
        `The save dialog could not be opened — ${errorText(error)}`,
      );
      return;
    }
    if (!path) return;
    const target = path;
    await runAction(
      `log-export-${session.id}`,
      () => source.exportSession(session.id, target, mode),
      `Session exported — ${config.label}`,
      "export the session",
    );
  }

  return (
    <Popover.Root>
      <Popover.Trigger asChild>
        <Button variant="secondary" disabled={exporting}>
          <Download size={15} />
          {exporting ? "Exporting…" : "Export"}
          <ChevronDown size={14} />
        </Button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          className="logs-export-menu"
          sideOffset={8}
          align="end"
          collisionPadding={16}
        >
          <span className="picker-label">Export session</span>
          {EXPORT_MODES.map((mode) => (
            <button
              type="button"
              disabled={exporting}
              onClick={() => void exportSession(mode.value)}
              key={mode.value}
            >
              <strong>{mode.label}</strong>
              <small>{mode.hint}</small>
            </button>
          ))}
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}
