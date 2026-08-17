import type { EngineTelemetry } from "../types";

/** True when the engine actually reported a score with this telemetry. */
export function hasEvaluation(
  info?: EngineTelemetry | null,
): info is EngineTelemetry {
  return info != null && (info.scoreCp != null || info.mateIn != null);
}

/**
 * A forced mate: `M3` / `−M3`.
 *
 * Exported because the log outline drew its own, spelling the same thing
 * `#3` a few inches from a board saying `M3`. One glyph, one place.
 */
export function mateText(mateIn: number) {
  return `${mateIn < 0 ? "−" : ""}M${Math.abs(mateIn)}`;
}

/** Centipawns as signed pawns to two decimals: `+3.00` / `−4.12` / `0.00`. */
export function pawnsText(centipawns: number) {
  const pawns = centipawns / 100;
  return `${pawns > 0 ? "+" : pawns < 0 ? "−" : ""}${Math.abs(pawns).toFixed(2)}`;
}

export function evaluationLabel(info?: EngineTelemetry | null) {
  if (info?.mateIn != null) return mateText(info.mateIn);
  if (info?.scoreCp == null) return "0.00";
  return pawnsText(info.scoreCp);
}

export function evaluationPercent(info?: EngineTelemetry | null) {
  if (info?.mateIn != null) return info.mateIn >= 0 ? 96 : 4;
  const centipawns = info?.scoreCp ?? 0;
  return Math.max(4, Math.min(96, 50 + 46 * Math.tanh(centipawns / 400)));
}

export function compactNumber(value?: number | null) {
  if (value == null) return "—";
  return new Intl.NumberFormat(undefined, {
    notation: "compact",
    maximumFractionDigits: 1,
  }).format(value);
}

export function searchTime(milliseconds?: number | null) {
  if (milliseconds == null) return "—";
  return milliseconds < 1000
    ? `${milliseconds} ms`
    : `${(milliseconds / 1000).toFixed(2)} s`;
}
