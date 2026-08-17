import type { TimeControl } from "../types";

export const defaultTimeControls: TimeControl[] = [
  { limitMinutes: 1, increment: 1 },
  { limitMinutes: 3, increment: 2 },
  { limitMinutes: 5, increment: 3 },
  { limitMinutes: 10, increment: 0 },
  { limitMinutes: 15, increment: 10 },
];

export const timeControlsStorageKey = "queenui-time-controls";

export function validTimeControl(value: unknown): value is TimeControl {
  if (!value || typeof value !== "object") return false;
  const control = value as Partial<TimeControl>;
  return (
    Number.isInteger(control.limitMinutes) &&
    control.limitMinutes! >= 1 &&
    control.limitMinutes! <= 180 &&
    Number.isInteger(control.increment) &&
    control.increment! >= 0 &&
    control.increment! <= 60
  );
}

export function storedTimeControls(): TimeControl[] {
  try {
    const stored = JSON.parse(
      localStorage.getItem(timeControlsStorageKey) ?? "null",
    );
    if (
      Array.isArray(stored) &&
      stored.length >= 1 &&
      stored.length <= 8 &&
      stored.every(validTimeControl)
    )
      return stored;
  } catch {
    // Ignore malformed preferences and restore the defaults.
  }
  return defaultTimeControls;
}

export function timeControlValue(control: TimeControl) {
  return `${control.limitMinutes}+${control.increment}`;
}

export function timeControlCategory(control: TimeControl) {
  const estimatedSeconds = control.limitMinutes * 60 + control.increment * 40;
  return estimatedSeconds < 180
    ? "Bullet"
    : estimatedSeconds < 480
      ? "Blitz"
      : estimatedSeconds < 1500
        ? "Rapid"
        : "Classical";
}

export function defaultSelectedTimeControl(controls: TimeControl[]) {
  return (
    controls.find(
      (control) => control.limitMinutes === 3 && control.increment === 2,
    ) ?? controls[0]
  );
}
