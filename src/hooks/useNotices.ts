import { useCallback, useEffect, useRef, useState } from "react";
import type { Notice } from "../types";

/**
 * Toast notice state.
 *
 * A *success* notice is a receipt: it can expire, and each new one restarts
 * the 5 s timer. Nothing else expires. This is the app's only report that an
 * action did not happen — or happened with less capability than asked for —
 * and five seconds is easily less time than an operator spends looking at a
 * board: a failed disconnect or a failed retention write used to erase itself
 * while nobody was watching, leaving a screen that looked exactly like a
 * screen where nothing had gone wrong. Errors and warnings stay until they are
 * dismissed or replaced.
 */
export function useNotices() {
  const [notice, setNotice] = useState<Notice | null>(null);
  const timerRef = useRef<number | undefined>(undefined);

  useEffect(() => () => window.clearTimeout(timerRef.current), []);

  const showNotice = useCallback((kind: Notice["kind"], message: string) => {
    setNotice({ kind, message });
    window.clearTimeout(timerRef.current);
    if (kind !== "success") return;
    timerRef.current = window.setTimeout(() => setNotice(null), 5000);
  }, []);

  const dismissNotice = useCallback(() => {
    window.clearTimeout(timerRef.current);
    setNotice(null);
  }, []);

  return { notice, showNotice, dismissNotice };
}

export type ShowNotice = ReturnType<typeof useNotices>["showNotice"];
