/**
 * Scorebook-only display rules.
 *
 * The general formatters (`titleCase`, `relativeDay`, `monthDay`,
 * `durationMmSs`, …) live in `src/lib/format.ts`; what stays here is the
 * handful of shapes only this page speaks — signed pawns, percentages, the
 * date-input round trip and the range label.
 */
import { fullDate, monthDay } from "../../lib/format";
import type { TimeSelection } from "../charts";
import type { ActivityBucket } from "../../types";
import type { ScorebookStats } from "../../types";

export const DAY_MS = 86_400_000;

export const BUCKET_CADENCE: Record<ActivityBucket, string> = {
  day: "Daily cadence",
  week: "Weekly cadence",
  month: "Monthly cadence",
};

export function streakText(streak: ScorebookStats["streak"]) {
  if (streak.kind === "none" || streak.length === 0) return "—";
  return `${streak.kind[0].toUpperCase()}${streak.length}`;
}

export function ratingText(rating: number | null) {
  return rating == null ? "—" : String(Math.round(rating));
}

/** Centipawns rendered as signed pawns with one decimal: +3.4 / −4.1. */
export function evalText(cp: number) {
  return `${cp < 0 ? "−" : "+"}${(Math.abs(cp) / 100).toFixed(1)}`;
}

export function percentText(value: number | null) {
  return value == null ? "—" : `${value.toFixed(1)}%`;
}

export function rangeLabel(range: TimeSelection) {
  const sameYear =
    new Date(range.fromMs).getFullYear() === new Date(range.toMs).getFullYear();
  return sameYear
    ? `${monthDay(range.fromMs)} – ${monthDay(range.toMs)}`
    : `${fullDate(range.fromMs)} – ${fullDate(range.toMs)}`;
}

/** Local calendar date of `ms` as a yyyy-mm-dd date-input value. */
export function dateInputValue(ms: number) {
  const date = new Date(ms);
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${date.getFullYear()}-${month}-${day}`;
}

/** yyyy-mm-dd → local midnight epoch ms, or null when incomplete. */
export function parseDateInput(value: string) {
  const [year, month, day] = value.split("-").map(Number);
  if (!year || !month || !day) return null;
  return new Date(year, month - 1, day).getTime();
}
