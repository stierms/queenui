import type { ShowNotice } from "../hooks/useNotices";
import { errorText } from "./errors";

/**
 * Run a Tauri file-dialog call and report a failure instead of losing it.
 *
 * `useActionRunner` guarantees that a *command* failure reaches the operator,
 * but the file dialog is opened before the command and was outside that
 * guarantee: every call site did a bare `await open(...)` / `await save(...)`
 * inside a function invoked as `void fn()`. When the dialog plugin rejects —
 * no portal on the host, a denied path, a webview that lost its window — the
 * rejection became an unhandled promise and the button simply did nothing, on
 * a screen whose whole contract is that a control's outcome is visible.
 *
 * `null` means "no path": either the operator cancelled, or the dialog failed
 * and has already said so. Both mean the caller stops, which is why they share
 * a return value.
 */
export async function pickPath<T extends string | string[] | null>(
  dialog: () => Promise<T>,
  showNotice: ShowNotice,
  /** What the dialog was for, e.g. "save the PGN". Named in the failure. */
  purpose: string,
): Promise<T | null> {
  try {
    return await dialog();
  } catch (cause) {
    console.error(`file dialog failed (${purpose}):`, cause);
    showNotice(
      "error",
      `The file dialog to ${purpose} could not be opened — ${errorText(cause)}. Try again, or restart QueenUI if it keeps failing.`,
    );
    return null;
  }
}
