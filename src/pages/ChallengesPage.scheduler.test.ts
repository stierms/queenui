import { describe, expect, it } from "vitest";
import { schedulerHealthDetail, schedulerHealthTitle } from "./schedulerHealth";
import type { CampaignRuntime, CampaignStatus } from "../types";
import { campaignEventClass } from "../types";

const ALL_STATUSES: CampaignStatus[] = [
  "starting",
  "discovering",
  "challenging",
  "running",
  "waiting",
  "backoff",
  "stopping",
  "stopped",
  "error",
  "unknown",
];

function runtime(status: string): CampaignRuntime {
  return {
    accountId: "queenbot",
    // Keep this helper able to simulate an unknown runtime wire value even
    // though the generated Rust contract now exposes a closed status union.
    status: status as CampaignStatus,
    activeGames: 2,
    pendingChallenges: 1,
    eligibleBots: 4,
    onlineBotsScanned: 312,
    challengesSent: 7,
    gamesStarted: 3,
    gamesCompleted: 2,
    lastOpponent: null,
    activity: "Scanning",
    error: null,
    nextScanAt: null,
    stopAt: null,
    events: [],
  };
}

describe("scheduler health copy", () => {
  it("has distinct copy for every campaign status", () => {
    // The old `default:` arm reported any unrecognised status as
    // "Matchmaking is stopped" — a running campaign displayed as stopped.
    const titles = ALL_STATUSES.map((status) =>
      schedulerHealthTitle(runtime(status)),
    );
    expect(new Set(titles).size).toBe(ALL_STATUSES.length);
    for (const status of ALL_STATUSES) {
      expect(schedulerHealthDetail(runtime(status))).not.toBe("");
    }
  });

  it("reports 'stopped' only for the stopped status and for no runtime", () => {
    expect(schedulerHealthTitle(runtime("stopped"))).toBe(
      "Matchmaking is stopped",
    );
    expect(schedulerHealthTitle(undefined)).toBe("Matchmaking is stopped");
    expect(schedulerHealthTitle(runtime("running"))).not.toMatch(/stopped/i);
  });

  it("does not report an unrecognised backend status as stopped", () => {
    // Runtime input can still violate a static contract, so a status this
    // build has never heard of must read as "cannot say", never as stopped.
    const title = schedulerHealthTitle(runtime("teleporting"));
    expect(title).not.toMatch(/stopped/i);
    expect(title).toBe(schedulerHealthTitle(runtime("unknown")));
    expect(schedulerHealthDetail(runtime("teleporting"))).not.toBe("");
  });

  it("names the next scan when one is scheduled", () => {
    expect(schedulerHealthDetail(runtime("waiting"), 42)).toContain("42s");
    expect(schedulerHealthDetail(runtime("waiting"))).toContain(
      "automatically",
    );
    const now = new Date(2026, 8, 1, 10, 27, 45).getTime();
    const limited = runtime("backoff");
    limited.nextScanAt = now + 73_025_000;
    const detail = schedulerHealthDetail(limited, 73_025, now);
    expect(detail).toContain("tomorrow at 06:44:50");
    expect(detail).toContain("in 20h 17m");
  });
});

describe("campaign event classes", () => {
  it("maps every kind campaign.rs records", () => {
    for (const kind of [
      "start",
      "stop",
      "timeout",
      "scan",
      "request",
      "idle",
      "found",
      "attempt",
      "sent",
      "rejected",
      "backoff",
      "declined",
      "canceled",
      "accepted",
      "finished",
      "aborted",
      "error",
    ]) {
      expect(campaignEventClass(kind)).toBe(`event-${kind}`);
    }
  });

  it("refuses to interpolate an unknown backend string into a class", () => {
    expect(campaignEventClass("something-new")).toBe("event-unknown");
    expect(campaignEventClass('" onload="alert(1)')).toBe("event-unknown");
  });
});
