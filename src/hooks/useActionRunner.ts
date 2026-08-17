import { useCallback, useState } from "react";
import { errorText } from "../lib/errors";
import type { ShowNotice } from "./useNotices";

export type BusyKeys = ReadonlySet<string>;

export type RunAction = (
  key: string,
  action: () => Promise<unknown>,
  success?: string,
  /**
   * What the action was trying to do, as a verb phrase ("add the engine").
   * The failure notice is built from it, because the backend's own message
   * describes a cause and never an intent: `permission denied (os error 13)`
   * on its own does not tell an operator which of several concurrent actions
   * just failed. Omitting it falls back to the bare backend text.
   */
  failure?: string,
) => Promise<boolean>;

/**
 * Runs backend actions with per-key busy tracking.
 *
 * `runAction` never throws: failures surface as an error notice and a
 * `false` return value, so onClick handlers cannot produce unhandled
 * promise rejections. Concurrent actions keep independent busy keys.
 */
export function useActionRunner(showNotice: ShowNotice) {
  const [busy, setBusy] = useState<BusyKeys>(new Set());

  const runAction = useCallback<RunAction>(
    async (key, action, success, failure) => {
      setBusy((current) => new Set(current).add(key));
      try {
        await action();
        if (success) showNotice("success", success);
        return true;
      } catch (error) {
        showNotice(
          "error",
          failure
            ? `Could not ${failure} — ${errorText(error)}`
            : errorText(error),
        );
        return false;
      } finally {
        setBusy((current) => {
          const next = new Set(current);
          next.delete(key);
          return next;
        });
      }
    },
    [showNotice],
  );

  return { busy, runAction };
}
