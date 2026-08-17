import type {
  ActivityBucket,
  AppSnapshot,
  BotRuntime,
  BotStatus,
  CampaignEventKind,
  CampaignStatus,
  DiagnosticLevel,
  LiveGame,
  LogDirection,
  PlayerColor,
  RunnerMode,
  UciOptionType,
} from "./index";

/**
 * Compile-time exhaustiveness. Reaching this call means a discriminated union
 * grew an arm that the caller's `switch` does not handle, and the compiler
 * says so at the call site rather than at runtime.
 *
 * The runtime throw is the honest fallback for a value that only a
 * contract violation (a backend that emits an undeclared variant) can
 * produce; callers that must survive such a value narrow with the
 * `is*` guards below instead of calling this.
 */
export function assertNever(value: never): never {
  throw new Error(`Unhandled variant: ${JSON.stringify(value)}`);
}

/**
 * The UCI control an option should render as. Anything the specification does
 * not define — and engines do print oddities — falls back to a text field,
 * which is what the dialog already did silently; here it is a stated rule.
 */
export function uciControlKind(optionType: string): UciOptionType {
  switch (optionType.toLowerCase()) {
    case "check":
      return "check";
    case "spin":
      return "spin";
    case "combo":
      return "combo";
    case "button":
      return "button";
    default:
      return "string";
  }
}

const CAMPAIGN_EVENT_KINDS: ReadonlySet<string> = new Set<CampaignEventKind>([
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
  "error",
]);

/**
 * `event-<kind>` marker class for a campaign event. An unrecognised kind gets
 * the neutral class instead of injecting a backend string into the DOM.
 */
export function campaignEventClass(kind: string) {
  return CAMPAIGN_EVENT_KINDS.has(kind) ? `event-${kind}` : "event-unknown";
}

/*
 * ===== Narrowing the generated `string`s =====
 *
 * ts-rs emits a Rust `String` as `string`, which is exactly what the wire
 * carries. The guards below are the one place that decides what QueenUI does
 * with a value outside the set it knows. Each one either coerces to the member
 * that is honest for that field, or — where no member would be — reports
 * membership and leaves the choice to the caller. Nothing lets an unrecognised
 * string reach a `switch` that would fall through to `assertNever` and throw
 * inside a render.
 */

const BOT_STATUSES: ReadonlySet<string> = new Set<BotStatus>([
  "stopped",
  "connecting",
  "online",
  "playing",
  "reconnecting",
  "error",
]);

/**
 * Whether a `BotRuntime.status` is one QueenUI knows.
 *
 * A predicate rather than a coercion on purpose: there is no safe default for
 * a bot's status. Reading an unrecognised one as "stopped" would claim a bot
 * is not playing, which is the one thing the fleet view must never get wrong —
 * callers show it as unknown instead.
 */
export function isBotStatus(value: string): value is BotStatus {
  return BOT_STATUSES.has(value);
}

const CAMPAIGN_STATUSES: ReadonlySet<string> = new Set<CampaignStatus>([
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
]);

/**
 * A `CampaignRuntime.status`.
 *
 * An unrecognised value maps to `"unknown"` — "QueenUI cannot say" — and
 * deliberately NOT to `"stopped"`: reporting a running campaign as stopped is
 * the exact dishonesty the closed union exists to prevent.
 */
export function campaignStatus(value: string): CampaignStatus {
  return CAMPAIGN_STATUSES.has(value) ? (value as CampaignStatus) : "unknown";
}

/** A `LiveGame.color`; only Lichess produces it and only ever these two. */
export function playerColor(value: string): PlayerColor {
  return value === "black" ? "black" : "white";
}

const LOG_DIRECTIONS: ReadonlySet<string> = new Set<LogDirection>([
  ">",
  "<",
  "!",
  "#",
]);

/** A `LogLine.direction`; an unknown marker renders as a QueenUI note. */
export function logDirection(value: string): LogDirection {
  return LOG_DIRECTIONS.has(value) ? (value as LogDirection) : "#";
}

const DIAGNOSTIC_LEVELS: ReadonlySet<string> = new Set<DiagnosticLevel>([
  "info",
  "warn",
  "error",
]);

/** A `DiagnosticEntry.level`; an unknown level is shown, not hidden. */
export function diagnosticLevel(value: string): DiagnosticLevel {
  return DIAGNOSTIC_LEVELS.has(value) ? (value as DiagnosticLevel) : "info";
}

/**
 * A `ScorebookStats.activityBucket`. An unrecognised bucket is read as a day,
 * which is what the chart assumed before the field existed.
 */
export function activityBucket(value: string): ActivityBucket {
  return value === "week" || value === "month" ? value : "day";
}

/**
 * A `RunnerSettingsView.mode` / `activeMode`. Unknown means embedded: the
 * conservative answer, because it never points the UI at a remote machine the
 * backend did not actually name.
 */
export function runnerMode(value: string): RunnerMode {
  return value === "remote" ? "remote" : "embedded";
}

export const emptySnapshot: AppSnapshot = {
  engines: [],
  accounts: [],
  runtimes: [],
  games: [],
  campaigns: [],
  campaignRuntimes: [],
};

const STOPPED_RUNTIME: Omit<BotRuntime, "accountId"> = {
  status: "stopped",
  error: null,
};

export function runtimeFor(
  snapshot: AppSnapshot,
  accountId: string,
): BotRuntime {
  return (
    snapshot.runtimes.find((runtime) => runtime.accountId === accountId) ?? {
      accountId,
      ...STOPPED_RUNTIME,
    }
  );
}

export function engineNameForGame(snapshot: AppSnapshot, game: LiveGame) {
  const account = snapshot.accounts.find((item) => item.id === game.accountId);
  return (
    snapshot.engines.find((item) => item.id === account?.engineId)?.name ??
    "UCI engine"
  );
}
