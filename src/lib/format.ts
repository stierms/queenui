/**
 * Shared display formatters.
 *
 * These were copied between LogsPage, ScorebookPage, SettingsPage and charts,
 * and the copies had drifted: two different byte roundings printed the same
 * file size differently on two screens, and two *different* functions were
 * both called `clockText` — one a wall-clock time of day, one an mm:ss
 * duration. They are named apart here (`timeOfDay` / `durationMmSs`) so the
 * collision cannot come back by import.
 */

import { isBotStatus } from "../types/helpers";
import type { BotStatus } from "../types/models";

export const MONTHS = [
  "Jan",
  "Feb",
  "Mar",
  "Apr",
  "May",
  "Jun",
  "Jul",
  "Aug",
  "Sep",
  "Oct",
  "Nov",
  "Dec",
] as const;

export const MONTHS_FULL = [
  "January",
  "February",
  "March",
  "April",
  "May",
  "June",
  "July",
  "August",
  "September",
  "October",
  "November",
  "December",
] as const;

export function titleCase(value: string) {
  return value ? value[0].toUpperCase() + value.slice(1) : value;
}

const DAY_MS = 86_400_000;

/** Coarse age of a timestamp: "today", "3d ago", "2mo ago", "1y ago". */
export function relativeDay(ms: number, now = Date.now()) {
  const days = Math.floor((now - ms) / DAY_MS);
  if (days <= 0) return "today";
  if (days === 1) return "yesterday";
  if (days < 30) return `${days}d ago`;
  if (days < 365) return `${Math.floor(days / 30)}mo ago`;
  return `${Math.floor(days / 365)}y ago`;
}

const MINUTE_MS = 60_000;
const HOUR_MS = 3_600_000;

/**
 * Age of a claim whose freshness is the point: "just now", "7m ago", "5h ago",
 * then `relativeDay` from a day out ("yesterday", "9d ago").
 *
 * `relativeDay` alone flattens everything inside 24 h into "today", which is
 * the wrong resolution for evidence taken minutes ago — an engine probed on
 * this launch and one probed before breakfast both read "today". A timestamp
 * ahead of `now` (backend clock skew, or a probe recorded during the same
 * millisecond) reads "just now" rather than a negative age.
 */
export function relativeSince(ms: number, now = Date.now()) {
  const elapsed = now - ms;
  if (elapsed < MINUTE_MS) return "just now";
  if (elapsed < HOUR_MS) return `${Math.floor(elapsed / MINUTE_MS)}m ago`;
  if (elapsed < DAY_MS) return `${Math.floor(elapsed / HOUR_MS)}h ago`;
  return relativeDay(ms, now);
}

/**
 * Bytes in binary units. One decimal below 100 of a unit, whole numbers above
 * — the Logs-page rounding, kept because a 45.3 MB recording reading "45 MB"
 * loses information the operator uses to reason about retention caps.
 */
export function formatBytes(bytes: number) {
  if (bytes < 1024) return `${Math.max(0, Math.round(bytes))} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value >= 100 ? Math.round(value) : value.toFixed(1)} ${units[unit]}`;
}

/** Wall-clock time of day, `HH:MM:SS`, in the operator's local zone. */
export function timeOfDay(ms: number) {
  const date = new Date(ms);
  return `${String(date.getHours()).padStart(2, "0")}:${String(date.getMinutes()).padStart(2, "0")}:${String(date.getSeconds()).padStart(2, "0")}`;
}

/** Wall-clock time of day without seconds, `HH:MM`. */
export function timeOfDayShort(ms: number) {
  const date = new Date(ms);
  return `${String(date.getHours()).padStart(2, "0")}:${String(date.getMinutes()).padStart(2, "0")}`;
}

/** Compact countdown that remains readable for a rolling daily limit. */
export function durationShortSeconds(seconds: number) {
  const total = Math.max(0, Math.ceil(seconds));
  if (total < 60) return `${total}s`;
  const minutes = Math.floor(total / 60);
  const remainingSeconds = total % 60;
  if (minutes < 60) {
    return remainingSeconds
      ? `${minutes}m ${remainingSeconds}s`
      : `${minutes}m`;
  }
  const hours = Math.floor(minutes / 60);
  const remainingMinutes = minutes % 60;
  return remainingMinutes ? `${hours}h ${remainingMinutes}m` : `${hours}h`;
}

/**
 * A duration as `mm:ss`; `null` renders as an em dash. Minutes are padded to
 * two digits to match `formatClock`, which renders every board clock in the
 * app — the columns are tabular-nums and an unpadded minute breaks both the
 * alignment and the visual rhyme with a real chess clock.
 */
export function durationMmSs(ms: number | null) {
  if (ms == null) return "—";
  const total = Math.max(0, Math.round(ms / 1000));
  return `${String(Math.floor(total / 60)).padStart(2, "0")}:${String(total % 60).padStart(2, "0")}`;
}

/** `12 Mar` — a date compact enough for a table cell. */
export function shortDate(ms: number) {
  const date = new Date(ms);
  return `${date.getDate()} ${MONTHS[date.getMonth()]}`;
}

/**
 * `Mar 12, 2026` — for the places where the year is the point: a range that
 * spans New Year, or a first/last-seen column. Deliberately distinct from
 * `shortDate`, which drops it.
 */
export function fullDate(ms: number) {
  const date = new Date(ms);
  return `${MONTHS[date.getMonth()]} ${date.getDate()}, ${date.getFullYear()}`;
}

/** `Mar 12` — the same date the other way round, for chart ticks. */
export function monthDay(ms: number) {
  const date = new Date(ms);
  return `${MONTHS[date.getMonth()]} ${date.getDate()}`;
}

/**
 * `3 active games` / `1 active game`.
 *
 * The scheduler used to sidestep the plural with `active game(s)`, a style
 * used nowhere else in QueenUI while a dozen other call sites singularize
 * properly. One noun in, the whole phrase out.
 */
export function countText(count: number, noun: string) {
  return `${count} ${noun}${count === 1 ? "" : "s"}`;
}

const BOT_STATUS_LABELS: Record<BotStatus, string> = {
  stopped: "Stopped",
  connecting: "Connecting",
  online: "Connected",
  playing: "Playing",
  reconnecting: "Reconnecting",
  error: "Error",
};

/**
 * A bot's runtime status as one display word.
 *
 * The same value used to be spelled three ways on screen: raw and lowercase
 * in the fleet table, raw-but-CSS-capitalized in the sidebar, and — four
 * lines apart inside the challenge composer — both `online` and "Connected".
 * The table is keyed on `BotStatus`, so adding a status without giving it a
 * word is a compile error; an undeclared value is shown verbatim rather than
 * being renamed into something the backend did not say.
 */
export function botStatusLabel(status: string | undefined) {
  if (!status) return BOT_STATUS_LABELS.stopped;
  return isBotStatus(status) ? BOT_STATUS_LABELS[status] : status;
}
