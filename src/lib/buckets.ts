import type { ActivityBucket } from "../types";

const DAY_MS = 86_400_000;

/**
 * Inclusive end (last millisecond) of the bucket that starts at `startMs`.
 *
 * Lives here rather than in `components/charts.tsx` because the scorebook page
 * needs it to decide whether a brush selection still overlaps the history, and
 * a chart module that also exports plain functions breaks Fast Refresh for
 * every component in it.
 */
export function bucketEndMs(startMs: number, bucket: ActivityBucket) {
  if (bucket === "day") return startMs + DAY_MS - 1;
  if (bucket === "week") return startMs + 7 * DAY_MS - 1;
  // Month buckets arrive as UTC calendar-month starts from the backend, so
  // the bucket end must be computed in UTC as well.
  const date = new Date(startMs);
  return (
    Date.UTC(date.getUTCFullYear(), date.getUTCMonth() + 1, 1, 0, 0, 0, 0) - 1
  );
}
