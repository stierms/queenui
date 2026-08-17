import { useState } from "react";
import { X } from "lucide-react";
import { ActivityBars, type TimeSelection } from "../charts";
import type { ActivityBucket, DayLine } from "../../types";
import { Button } from "../../ui/primitives";
import {
  BUCKET_CADENCE,
  DAY_MS,
  dateInputValue,
  parseDateInput,
  rangeLabel,
} from "./format";

/**
 * Games per bucket, brushable into a time range, with a date-input filter
 * as the keyboard-reachable equivalent of the brush. The committed range is
 * the page's (it drives the stats query); the in-progress text of the two
 * date inputs is local.
 */
export function ActivityPanel({
  days,
  bucket,
  selection,
  onSelect,
}: {
  days: DayLine[];
  bucket: ActivityBucket;
  selection: TimeSelection | null;
  onSelect: (selection: TimeSelection | null) => void;
}) {
  const [showDateFilter, setShowDateFilter] = useState(false);
  // In-progress date-input edits (may be a partial range); discarded in
  // favor of values derived from `selection` whenever it moves, which keeps
  // the inputs and the brush in sync both ways.
  const [dateDraft, setDateDraft] = useState<{
    from: string;
    to: string;
  } | null>(null);
  const [draftRange, setDraftRange] = useState(selection);
  if (draftRange !== selection) {
    setDraftRange(selection);
    setDateDraft(null);
  }
  const fromInput =
    dateDraft?.from ?? (selection ? dateInputValue(selection.fromMs) : "");
  const toInput =
    dateDraft?.to ?? (selection ? dateInputValue(selection.toMs) : "");

  function applyDateInputs(fromValue: string, toValue: string) {
    setDateDraft({ from: fromValue, to: toValue });
    if (!fromValue && !toValue) {
      onSelect(null);
      return;
    }
    const fromMs = fromValue ? parseDateInput(fromValue) : null;
    const toMs = toValue ? parseDateInput(toValue) : null;
    if (fromMs == null || toMs == null) return;
    const toEndMs = toMs + DAY_MS - 1;
    if (fromMs <= toEndMs) onSelect({ fromMs, toMs: toEndMs });
  }

  const selectedGames = selection
    ? days
        .filter(
          (day) =>
            day.dayStartMs >= selection.fromMs &&
            day.dayStartMs <= selection.toMs,
        )
        .reduce((sum, day) => sum + day.games, 0)
    : 0;

  return (
    <section className="panel scorebook-panel">
      <div className="panel-heading activity-heading">
        <div>
          <span className="eyebrow">{BUCKET_CADENCE[bucket]}</span>
          <h2>Activity</h2>
          <p className="activity-subtitle">
            {selection ? rangeLabel(selection) : "All time"}
          </p>
        </div>
        <div className="activity-tools">
          {selection && (
            <span className="activity-range-chip">
              {rangeLabel(selection)} · {selectedGames} game
              {selectedGames === 1 ? "" : "s"}
              <button
                type="button"
                aria-label="Clear time selection"
                onClick={() => onSelect(null)}
              >
                <X size={12} />
              </button>
            </span>
          )}
          <Button
            variant="secondary"
            aria-expanded={showDateFilter}
            onClick={() => setShowDateFilter((shown) => !shown)}
          >
            Filter by date
          </Button>
        </div>
      </div>
      <div className="scorebook-panel-body">
        {showDateFilter && (
          <div className="activity-date-filter">
            <label>
              From
              <input
                type="date"
                value={fromInput}
                onChange={(event) =>
                  applyDateInputs(event.target.value, toInput)
                }
              />
            </label>
            <label>
              To
              <input
                type="date"
                value={toInput}
                onChange={(event) =>
                  applyDateInputs(fromInput, event.target.value)
                }
              />
            </label>
          </div>
        )}
        {days.length > 0 ? (
          <ActivityBars
            days={days}
            bucket={bucket}
            selection={selection}
            onSelect={onSelect}
          />
        ) : (
          <p className="chart-empty">No games in this period.</p>
        )}
      </div>
    </section>
  );
}
