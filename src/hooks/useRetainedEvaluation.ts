import { useState } from "react";
import { hasEvaluation } from "../lib/evaluation";
import type { EngineTelemetry } from "../types";

/**
 * Keeps the engine's most recent scored evaluation on screen.
 *
 * Telemetry disappears twice per move: the backend clears it the moment our
 * turn starts a new search, and UCI `info` lines without a `score` field
 * (`currmove` progress, for instance) arrive scoreless. Rendering those states
 * directly snapped the eval bar back to 0.00 between every move, so the last
 * known evaluation is held until the engine reports a new one.
 *
 * State lives with the game panel that owns the hook, so one game's
 * evaluation never leaks into another.
 */
export function useRetainedEvaluation(info?: EngineTelemetry | null) {
  const [lastScored, setLastScored] = useState<EngineTelemetry | null>(null);
  if (hasEvaluation(info)) {
    // React's documented "adjust state while rendering" pattern: the new score
    // renders in the same pass, with no frame showing the previous one.
    if (info !== lastScored) setLastScored(info);
    return info;
  }
  return lastScored;
}
