import { useCallback, useEffect, useState } from "react";
import {
  collapsedWidgetsStorageKey,
  storedCollapsedWidgets,
  type CollapsedWidgets,
  type GameWidget,
} from "../lib/gameView";

/**
 * The collapsed/expanded state of the focus view's two widgets.
 *
 * Seeded from disk and written back on every change, so the choice outlives a
 * game switch, a trip to another page, and a restart. The state lives above
 * the panel that renders it — one record for the whole surface — which is what
 * makes it global rather than per game.
 */
export function useCollapsedWidgets() {
  const [collapsed, setCollapsed] = useState<CollapsedWidgets>(
    storedCollapsedWidgets,
  );

  useEffect(() => {
    localStorage.setItem(collapsedWidgetsStorageKey, JSON.stringify(collapsed));
  }, [collapsed]);

  const toggleWidget = useCallback((widget: GameWidget) => {
    setCollapsed((current) => ({ ...current, [widget]: !current[widget] }));
  }, []);

  return { collapsed, toggleWidget };
}
