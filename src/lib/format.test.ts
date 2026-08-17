import { describe, expect, it } from "vitest";
import {
  durationMmSs,
  formatBytes,
  relativeDay,
  relativeSince,
  timeOfDay,
  timeOfDayShort,
  titleCase,
} from "./format";

describe("format helpers", () => {
  it("prints one decimal below 100 of a unit and whole numbers above", () => {
    // The Logs page and Settings disagreed here, so the same recording size
    // printed differently on two screens.
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(1536)).toBe("1.5 KB");
    expect(formatBytes(47_500_000)).toBe("45.3 MB");
    expect(formatBytes(250_000_000)).toBe("238 MB");
    expect(formatBytes(41_000_000_000)).toBe("38.2 GB");
  });

  it("separates the two clock helpers that used to share a name", () => {
    const at = new Date(2026, 7, 16, 9, 4, 5).getTime();
    expect(timeOfDay(at)).toBe("09:04:05");
    expect(timeOfDayShort(at)).toBe("09:04");
    // ...and this one is a duration, not a time of day — padded like the
    // board clocks it sits beside.
    expect(durationMmSs(65_400)).toBe("01:05");
    expect(durationMmSs(null)).toBe("—");
    expect(durationMmSs(-10)).toBe("00:00");
  });

  it("describes ages in the coarsest useful unit", () => {
    const now = new Date(2026, 7, 16).getTime();
    const days = (count: number) => now - count * 86_400_000;
    expect(relativeDay(now, now)).toBe("today");
    expect(relativeDay(days(1), now)).toBe("yesterday");
    expect(relativeDay(days(9), now)).toBe("9d ago");
    expect(relativeDay(days(45), now)).toBe("1mo ago");
    expect(relativeDay(days(800), now)).toBe("2y ago");
  });

  it("resolves minutes and hours before falling back to days", () => {
    // A probe badge's whole job is to say how stale its evidence is, and
    // `relativeDay` calls everything inside 24 h "today".
    const now = new Date(2026, 7, 16, 12, 0, 0).getTime();
    expect(relativeSince(now, now)).toBe("just now");
    expect(relativeSince(now - 45_000, now)).toBe("just now");
    expect(relativeSince(now - 60_000, now)).toBe("1m ago");
    expect(relativeSince(now - 59 * 60_000, now)).toBe("59m ago");
    expect(relativeSince(now - 3_600_000, now)).toBe("1h ago");
    expect(relativeSince(now - 23 * 3_600_000, now)).toBe("23h ago");
    expect(relativeSince(now - 86_400_000, now)).toBe("yesterday");
    expect(relativeSince(now - 9 * 86_400_000, now)).toBe("9d ago");
    // A backend clock ahead of this one must not print a negative age.
    expect(relativeSince(now + 30_000, now)).toBe("just now");
  });

  it("title-cases without choking on an empty string", () => {
    expect(titleCase("resign")).toBe("Resign");
    expect(titleCase("")).toBe("");
  });
});
