import { useCallback, useEffect, useState } from "react";
import * as commands from "../api/commands";
import { errorText } from "../lib/errors";
import type { RunnerSettingsView } from "../types";

/**
 * The runner settings, owned in one place.
 *
 * `App` used to fetch these at mount with `.catch(() => {})` while the settings
 * panel fetched and mutated its own independent copy. A transient failure left
 * `App` permanently in embedded mode — choosing a native file dialog for a
 * machine that has no such file — while Settings showed remote. One owner, one
 * fetch, and the failure is reported rather than swallowed.
 */
export function useRunnerSettings(enabled = true) {
  const [settings, setSettings] = useState<RunnerSettingsView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(enabled);

  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    if (!enabled) return;
    let active = true;
    commands
      .getRunnerSettings()
      .then((next) => {
        if (!active) return;
        setSettings(next);
        setError(null);
      })
      .catch((cause: unknown) => {
        if (active) setError(errorText(cause));
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [enabled, attempt]);

  const refresh = useCallback(() => {
    setLoading(true);
    setAttempt((value) => value + 1);
  }, []);

  return { settings, error, loading, refresh, setSettings };
}
