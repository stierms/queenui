import { timeControlCategory, timeControlValue } from "../lib/timeControls";
import type { TimeControl } from "../types";

export function TimeControlPresets({
  controls,
  selected,
  onSelect,
  className = "",
}: {
  controls: TimeControl[];
  selected: string;
  onSelect: (value: string) => void;
  className?: string;
}) {
  return (
    <div className={`clock-presets ${className}`.trim()}>
      {controls.map((control, index) => {
        const value = timeControlValue(control);
        return (
          <button
            type="button"
            className={selected === value ? "selected" : ""}
            aria-pressed={selected === value}
            key={`${value}-${index}`}
            onClick={() => onSelect(value)}
          >
            <strong>{value}</strong>
            <small>{timeControlCategory(control)}</small>
          </button>
        );
      })}
    </div>
  );
}
