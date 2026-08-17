import type { ReactNode } from "react";
import type { ScorebookStats } from "../../types";
import { SelectField } from "../../ui/primitives";

const PERF_OPTIONS = [
  { value: "ultraBullet", label: "UltraBullet" },
  { value: "bullet", label: "Bullet" },
  { value: "blitz", label: "Blitz" },
  { value: "rapid", label: "Rapid" },
  { value: "classical", label: "Classical" },
];

/** Page title plus the three filters every panel below is sliced by. */
export function ScorebookHero({
  accounts,
  engines,
  accountId,
  engineId,
  perf,
  onAccountChange,
  onEngineChange,
  onPerfChange,
  importButton,
}: {
  accounts: ScorebookStats["accounts"];
  engines: ScorebookStats["engines"];
  accountId: string;
  engineId: string;
  perf: string;
  onAccountChange: (value: string) => void;
  onEngineChange: (value: string) => void;
  onPerfChange: (value: string) => void;
  importButton: ReactNode;
}) {
  return (
    <section className="module-hero scorebook-hero">
      <div>
        <span className="eyebrow">Game record</span>
        <h2>Scorebook</h2>
        <p>Every finished game, scored and sliced.</p>
      </div>
      <div className="scorebook-controls">
        <SelectField
          label="Filter by account"
          value={accountId}
          onChange={onAccountChange}
        >
          <option value="">All accounts</option>
          {accounts.map((account) => (
            <option value={account.id} key={account.id}>
              {account.username}
            </option>
          ))}
        </SelectField>
        <SelectField
          label="Filter by engine"
          value={engineId}
          onChange={onEngineChange}
        >
          <option value="">All engines</option>
          {engines.map((engine) => (
            <option value={engine.id} key={engine.id}>
              {engine.name}
            </option>
          ))}
        </SelectField>
        <SelectField
          label="Filter by speed"
          value={perf}
          onChange={onPerfChange}
        >
          <option value="">All speeds</option>
          {PERF_OPTIONS.map((option) => (
            <option value={option.value} key={option.value}>
              {option.label}
            </option>
          ))}
        </SelectField>
        {importButton}
      </div>
    </section>
  );
}
