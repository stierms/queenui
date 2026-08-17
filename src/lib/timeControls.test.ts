import { afterEach, describe, expect, it } from "vitest";
import {
  defaultTimeControls,
  storedTimeControls,
  timeControlsStorageKey,
} from "./timeControls";

afterEach(() => {
  localStorage.clear();
});

describe("storedTimeControls", () => {
  it("falls back to the defaults for malformed JSON", () => {
    localStorage.setItem(timeControlsStorageKey, "{not valid json");
    expect(storedTimeControls()).toEqual(defaultTimeControls);
  });

  it("falls back to the defaults for invalid shapes", () => {
    localStorage.setItem(
      timeControlsStorageKey,
      JSON.stringify([{ limitMinutes: 0, increment: -1 }]),
    );
    expect(storedTimeControls()).toEqual(defaultTimeControls);

    localStorage.setItem(timeControlsStorageKey, JSON.stringify([]));
    expect(storedTimeControls()).toEqual(defaultTimeControls);

    localStorage.setItem(timeControlsStorageKey, JSON.stringify("3+2"));
    expect(storedTimeControls()).toEqual(defaultTimeControls);
  });

  it("returns valid stored presets", () => {
    const stored = [
      { limitMinutes: 2, increment: 1 },
      { limitMinutes: 25, increment: 10 },
    ];
    localStorage.setItem(timeControlsStorageKey, JSON.stringify(stored));
    expect(storedTimeControls()).toEqual(stored);
  });
});
