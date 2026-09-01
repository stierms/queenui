/*
 * What the generated contract cannot express.
 *
 * `models.gen.ts` (ts-rs, emitted from the Rust types) is now THE description
 * of the IPC wire. This file used to be a hand-written mirror of it; every type
 * the generator emits has been deleted from here, so there is exactly one
 * definition of each wire shape and no way for the two to drift.
 *
 * What survives is only what Rust does not export:
 *
 *   1. **Closed unions over `string` wire fields.** ts-rs emits a Rust `String`
 *      as `string`, which is the truth about the wire but useless to a `switch`.
 *      Where QueenUI itself produces the value set (`BotRuntime.status`) the
 *      union below is closed and consumers narrow with a guard from
 *      `helpers.ts`. Where the producer is Lichess or an arbitrary UCI engine
 *      (`LiveGame.status`, `UciOption.optionType`, `CampaignEvent.kind`) the
 *      union names the values the UI has copy for, and nothing pretends it is
 *      exhaustive. Rust's `CampaignStatus` enum is generated directly.
 *   2. **UI-only shapes** that never cross the IPC boundary (`Notice`,
 *      `TimeControl`).
 *   3. **Shapes derived from generated ones** with `Omit`/`Pick`, never
 *      retyped by hand.
 *
 * Import everything through `src/types/index.ts`, which re-exports the
 * generated module, this one, and `helpers.ts` together.
 */

import type { OpeningBookUpdate } from "./models.gen";

/**
 * The five control types the UCI specification defines. Engines are free to
 * emit anything, so `UciOption.optionType` is `string`; `uciControlKind`
 * maps it onto this union and folds everything unrecognised into `"string"`.
 */
export type UciOptionType = "check" | "spin" | "combo" | "button" | "string";

/**
 * Set by `RuntimeState::set_runtime` — QueenUI's own value set, so it is closed
 * and `isBotStatus` checks a generated `string` against it.
 */
export type BotStatus =
  "stopped" | "connecting" | "online" | "playing" | "reconnecting" | "error";

/**
 * Game statuses Lichess reports. Relayed verbatim, so `LiveGame.status` stays
 * `string`; this union names the ones the UI has copy for, and
 * `gameStatusLabel`'s table is keyed on it so adding a member without a label
 * is a compile error.
 */
export type GameStatus =
  | "created"
  | "started"
  | "aborted"
  | "mate"
  | "resign"
  | "stalemate"
  | "timeout"
  | "outoftime"
  | "draw"
  | "cheat"
  | "noStart"
  | "unknownFinish"
  | "variantEnd";

/** The side a bot plays. `LiveGame.color` is `string` on the wire. */
export type PlayerColor = "white" | "black";

/**
 * Every kind `campaign.rs` records. It reaches the DOM as an `event-<kind>`
 * class, so `campaignEventClass` checks membership before interpolating rather
 * than trusting the backend string.
 */
export type CampaignEventKind =
  | "start"
  | "stop"
  | "timeout"
  | "scan"
  | "request"
  | "idle"
  | "found"
  | "attempt"
  | "sent"
  | "rejected"
  | "backoff"
  | "declined"
  | "canceled"
  | "accepted"
  | "finished"
  | "aborted"
  | "error";

/** `>` sent to the engine, `<` received, `!` engine stderr, `#` QueenUI note. */
export type LogDirection = ">" | "<" | "!" | "#";

export type DiagnosticLevel = "info" | "warn" | "error";

/**
 * Bucket size the backend aggregated `ScorebookStats.activity` with. It picks
 * the bucket from the span of the history, so the value set is QueenUI's own;
 * `activityBucket` narrows the generated `string` onto it.
 */
export type ActivityBucket = "day" | "week" | "month";

/** Which runner executes games. `RunnerSettingsView.mode` is `string`. */
export type RunnerMode = "embedded" | "remote";

/**
 * The opening-book fields the configuration dialog edits. The engine id is not
 * one of them — it is supplied by the caller that knows which engine is being
 * configured — so this is the generated update minus that field, never a
 * second hand-written copy of it.
 */
export type OpeningBookRequest = Omit<OpeningBookUpdate, "engineId">;

/* ===== UI-only shapes: these never cross the IPC boundary ===== */

/**
 * Three grades, because an action can succeed and still leave the operator
 * with less than they asked for. A Lichess token that carries `bot:play` but
 * no challenge scopes connects fine and cannot run matchmaking: reporting that
 * as a success hides it, and reporting it as a failure claims the account was
 * not stored. Only `success` expires (see `useNotices`).
 */
export type Notice = {
  kind: "success" | "warning" | "error";
  message: string;
};

export type TimeControl = {
  limitMinutes: number;
  increment: number;
};
