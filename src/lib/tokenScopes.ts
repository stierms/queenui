import type { AddAccountResult, Notice } from "../types";

/**
 * What a freshly connected Lichess token can and cannot do.
 *
 * `add_lichess_account` stores the account before it looks at scopes, so every
 * one of the three answers below is a *successful* connect — the difference is
 * what the operator can then do with it. That distinction is the whole point of
 * this module: a play-only token used to be stored with the same "connected
 * securely" receipt a full token got, and the first sign anything was wrong was
 * an opaque 403 from matchmaking, hours later.
 *
 * The required set and its order are the backend's (`lichess::MATCHMAKING_SCOPES`
 * → `missingForMatchmaking`); nothing here recomputes them, so the two cannot
 * drift. The UI's only job is to say which ones are absent and what that costs.
 */
export type TokenScopeGap = {
  /**
   * Required matchmaking scopes the connected token does not carry, in the
   * backend's stable display order. Never empty: no gap means no record.
   */
  missing: string[];
  /**
   * False when `bot:play` itself is absent — the token cannot operate bot games
   * at all, which is a different (and worse) fact than matchmaking being off.
   */
  canPlayGames: boolean;
};

/** Where a replacement token is minted. Same address the backend quotes. */
export const tokenCreateUrl = "lichess.org/account/oauth/token/create";

/**
 * The tick boxes on that page, named the way the backend's own 403 message
 * names them so the app speaks with one voice.
 *
 * All three are always listed, including ones the current token already has:
 * the remedy is a *new* token, and a new token needs the full set regardless of
 * what the old one happened to carry.
 */
const tokenCreateBoxes = "Play-bot, Read-challenges and Send-challenges";

/**
 * The required set, for the one hint that has to exist *before* any backend
 * call: the line under the token field, read while the token is being pasted.
 *
 * This is the only place the frontend names the set itself, and it is a mirror
 * of `lichess::MATCHMAKING_SCOPES`. Everything after the connect uses the
 * backend's own `missingForMatchmaking` instead, so a change to the required
 * set can only ever make this line stale — never the verdict.
 */
export const matchmakingScopes = [
  "bot:play",
  "challenge:read",
  "challenge:write",
] as const;

/**
 * That line, spelled once.
 *
 * Two dialogs now take a pasted token — the connect and the in-place
 * replacement — and both have to say the same thing about what the token needs
 * to carry. Written out twice it is one edit away from two different required
 * sets, which is the drift this module exists to prevent.
 */
export const tokenScopeHint = `Required scopes: ${matchmakingScopes.join(", ")} — a play-only token connects, but matchmaking will not work with it.`;

export const tokenScopeStorageKey = "queenui-token-scope-gaps";

/**
 * The gap this connect result records, or `null` when the token is complete.
 *
 * A `null` is meaningful — it is what clears a previous warning off an account
 * that has just been reconnected with a better token.
 */
export function tokenScopeGap(result: AddAccountResult): TokenScopeGap | null {
  if (result.missingForMatchmaking.length === 0) return null;
  return {
    missing: [...result.missingForMatchmaking],
    canPlayGames: result.canPlayGames,
  };
}

/** "scope bot:play" / "scopes challenge:read, challenge:write". */
function scopeList(gap: TokenScopeGap) {
  return `${gap.missing.length === 1 ? "scope" : "scopes"} ${gap.missing.join(", ")}`;
}

/**
 * The short line: which account, and what it lost. Kept separate from the
 * detail so the account card can set it in bold the way `runtime.error` does.
 */
export function scopeGapHeadline(gap: TokenScopeGap, username: string) {
  return gap.canPlayGames
    ? `${username} can play, but matchmaking is off`
    : `${username} cannot play with this token`;
}

/**
 * The two sentences that follow: the exact scope names, the plain consequence,
 * and the remedy. Two sentences is the budget — an operator who has just pasted
 * a token reads a line, not a page.
 *
 * The remedy used to end "then connect the account again", which was the only
 * route there was and a costly one: `add_lichess_account` on an account that
 * already exists rewrites its whole profile from the dialog, so reconnecting
 * silently reassigns the engine to whatever the composer's picker happened to
 * be showing. `update_lichess_account_token` writes the secret and nothing
 * else, so the sentence names that action instead — the account keeps its
 * engine, its campaign and its running games.
 */
export function scopeGapDetail(gap: TokenScopeGap) {
  const consequence = gap.canPlayGames
    ? "matchmaking will not work with this token"
    : "QueenUI cannot play games with this token, and matchmaking will not work either";
  return `Missing ${scopeList(gap)} — ${consequence}. Create a new token at ${tokenCreateUrl} with ${tokenCreateBoxes} ticked, then replace this account's token from its Actions menu on Overview.`;
}

/**
 * The connect-time announcement.
 *
 * A missing `bot:play` is error-grade: the account is stored, but nothing the
 * app offers to do with it will work. A missing challenge scope is a warning —
 * the bot plays, the campaign does not. Neither kind expires on its own, and
 * neither one is a success receipt, because "connected securely" is exactly the
 * sentence that hid this problem the first time.
 */
export function scopeGapNotice(gap: TokenScopeGap, username: string): Notice {
  return {
    kind: gap.canPlayGames ? "warning" : "error",
    message: `${scopeGapHeadline(gap, username)}. ${scopeGapDetail(gap)}`,
  };
}

function validGap(value: unknown): value is TokenScopeGap {
  if (!value || typeof value !== "object") return false;
  const gap = value as Partial<TokenScopeGap>;
  return (
    Array.isArray(gap.missing) &&
    gap.missing.length > 0 &&
    gap.missing.every(
      (scope) => typeof scope === "string" && scope.length > 0,
    ) &&
    typeof gap.canPlayGames === "boolean"
  );
}

/**
 * Gaps recorded by earlier connects, keyed by account id.
 *
 * This survives a restart on purpose. The snapshot carries no scope data — the
 * gap is a property of a stored token, learned once when it was validated — so
 * holding it in component state would erase the warning at the next launch
 * while the token stayed exactly as broken as it was. A false all-clear is the
 * failure this round exists to remove; the opposite risk (a token replaced
 * outside QueenUI leaving a stale warning) is visible, and the remedy the
 * notice gives — reconnect the account — is what clears it.
 */
export function storedScopeGaps(): Record<string, TokenScopeGap> {
  try {
    const stored: unknown = JSON.parse(
      localStorage.getItem(tokenScopeStorageKey) ?? "null",
    );
    if (!stored || typeof stored !== "object" || Array.isArray(stored))
      return {};
    return Object.fromEntries(
      Object.entries(stored).filter(([accountId, gap]) => {
        return accountId.length > 0 && validGap(gap);
      }),
    ) as Record<string, TokenScopeGap>;
  } catch {
    // A malformed record is not evidence of anything; start clean.
    return {};
  }
}
