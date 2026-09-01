import {
  countText,
  durationShortSeconds,
  shortDate,
  timeOfDay,
} from "../lib/format";
import { assertNever, campaignStatus, type CampaignRuntime } from "../types";

/*
 * What the matchmaking scheduler is doing, in one line and in one paragraph.
 *
 * Lives beside `ChallengesPage` rather than inside it: the page exports
 * components, these are pure functions over a runtime, and a module that
 * exports both breaks React Fast Refresh for the whole file.
 *
 * Exhaustive over `CampaignStatus`. The `default` arm used to swallow every
 * unrecognised value into "Matchmaking is stopped", so a backend rename would
 * have reported a *running* campaign as stopped — a wrong-state display rather
 * than a visible failure. `CampaignRuntime.status` arrives as a bare `string`
 * from the generated contract, so `campaignStatus` narrows it first and an
 * unknown value lands on the "cannot say" arm, never on "stopped".
 */

export function schedulerHealthTitle(runtime?: CampaignRuntime) {
  if (!runtime) return "Matchmaking is stopped";
  const status = campaignStatus(runtime.status);
  switch (status) {
    case "discovering":
      return "Discovery request in progress";
    case "challenging":
      return "Candidates found — sending challenges";
    case "running":
      return "Target capacity is filled";
    case "waiting":
      return "Scheduler healthy — no match yet";
    case "backoff":
      return "Scheduler paused by Lichess rate limiting";
    case "error":
      return "Campaign controller needs attention";
    case "starting":
      return "Starting campaign controller";
    case "stopping":
      return "Stopping safely";
    case "stopped":
      return "Matchmaking is stopped";
    case "unknown":
      return "Campaign state is being reconciled";
    default:
      return assertNever(status);
  }
}

export function schedulerHealthDetail(
  runtime?: CampaignRuntime,
  nextScanSeconds: number | null = null,
  now = Date.now(),
) {
  const idle = "Start a campaign to begin discovering online opponents.";
  if (!runtime) return idle;
  const status = campaignStatus(runtime.status);
  switch (status) {
    case "discovering":
      return "The API queue and discovery response share a 12-second deadline; every phase appears below.";
    case "challenging":
      return "Opponent attempts and Lichess responses are recorded in real time.";
    case "running":
      return `${countText(runtime.activeGames, "active game")} and ${countText(runtime.pendingChallenges, "unanswered challenge")}.`;
    case "waiting":
      return nextScanSeconds === null
        ? "Another scan is scheduled automatically."
        : `No eligible opponent accepted yet. Scanning again in ${nextScanSeconds}s.`;
    case "backoff":
      if (nextScanSeconds === null || runtime.nextScanAt === null) {
        return "QueenUI will retry automatically.";
      }
      return `Respecting the API limit. Retrying ${scheduledTime(runtime.nextScanAt, now)} (in ${durationShortSeconds(nextScanSeconds)}).`;
    case "error":
      return (
        runtime.error ||
        "The controller stopped unexpectedly. Stop it, then start again."
      );
    case "starting":
      return "Connecting the bot event stream before discovering opponents.";
    case "stopping":
      return "Outstanding challenges are being canceled; active games are untouched.";
    case "stopped":
      return idle;
    /*
     * COORDINATION.md: a challenge is being created or cancelled and the
     * controller cannot yet say which side won. A failed cancellation stays in
     * `pendingChallenges` with the reason in `error`, so both are reported
     * rather than presenting the campaign as stopped.
     */
    case "unknown":
      return (
        runtime.error ||
        `Waiting for Lichess to confirm the last request. ${countText(runtime.activeGames, "active game")}, ${countText(runtime.pendingChallenges, "unanswered challenge")}.`
      );
    default:
      return assertNever(status);
  }
}

function scheduledTime(timestamp: number, now: number) {
  const at = new Date(timestamp);
  const current = new Date(now);
  const sameDay =
    at.getFullYear() === current.getFullYear() &&
    at.getMonth() === current.getMonth() &&
    at.getDate() === current.getDate();
  if (sameDay) return `today at ${timeOfDay(timestamp)}`;

  const tomorrow = new Date(current);
  tomorrow.setDate(tomorrow.getDate() + 1);
  const nextDay =
    at.getFullYear() === tomorrow.getFullYear() &&
    at.getMonth() === tomorrow.getMonth() &&
    at.getDate() === tomorrow.getDate();
  return nextDay
    ? `tomorrow at ${timeOfDay(timestamp)}`
    : `on ${shortDate(timestamp)} at ${timeOfDay(timestamp)}`;
}
