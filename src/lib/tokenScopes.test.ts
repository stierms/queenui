import { afterEach, describe, expect, it } from "vitest";
import {
  scopeGapDetail,
  scopeGapHeadline,
  scopeGapNotice,
  storedScopeGaps,
  tokenScopeGap,
  tokenScopeStorageKey,
} from "./tokenScopes";
import type { AddAccountResult } from "../types";

const account = {
  id: "queenbot",
  username: "QueenBot",
  engineId: "engine-1",
  rating: 2400,
  enabled: false,
};

function result(overrides: Partial<AddAccountResult>): AddAccountResult {
  return {
    account,
    scopes: [],
    missingForMatchmaking: [],
    canPlayGames: true,
    ...overrides,
  };
}

afterEach(() => localStorage.clear());

describe("the scope verdict", () => {
  it("records nothing when the token carries the whole required set", () => {
    // No gap, no entry — this `null` is also what clears a previous warning
    // off an account that has just been reconnected with a better token.
    expect(
      tokenScopeGap(
        result({
          scopes: ["bot:play", "challenge:read", "challenge:write"],
        }),
      ),
    ).toBeNull();
  });

  it("keeps the backend's scope list and order verbatim", () => {
    /*
     * `missingForMatchmaking` is computed from `lichess::MATCHMAKING_SCOPES`,
     * whose order is fixed. Re-deriving or re-sorting it here would let the UI
     * name a scope the backend does not require, or omit one it does.
     */
    expect(
      tokenScopeGap(
        result({
          scopes: ["challenge:write"],
          missingForMatchmaking: ["bot:play", "challenge:read"],
          canPlayGames: false,
        }),
      ),
    ).toStrictEqual({
      missing: ["bot:play", "challenge:read"],
      canPlayGames: false,
    });
  });
});

describe("the hint's wording", () => {
  const matchmakingGap = {
    missing: ["challenge:read", "challenge:write"],
    canPlayGames: true,
  };
  const blockingGap = { missing: ["bot:play"], canPlayGames: false };

  it("names every missing challenge scope and the exact consequence", () => {
    expect(scopeGapHeadline(matchmakingGap, "QueenBot")).toBe(
      "QueenBot can play, but matchmaking is off",
    );
    /*
     * The remedy names the in-place replacement, not a reconnect. Connecting an
     * account that already exists rewrites its profile from the connect dialog
     * — including reassigning its engine to whatever the picker was showing —
     * so the advice for a token problem used to cost the operator settings.
     * `update_lichess_account_token` writes the secret and nothing else.
     */
    expect(scopeGapDetail(matchmakingGap)).toBe(
      "Missing scopes challenge:read, challenge:write — matchmaking will not " +
        "work with this token. Create a new token at " +
        "lichess.org/account/oauth/token/create with Play-bot, Read-challenges " +
        "and Send-challenges ticked, then replace this account's token from " +
        "its Actions menu on Overview.",
    );
  });

  it("says a bot:play-less token cannot play, and does not soften it", () => {
    expect(scopeGapHeadline(blockingGap, "QueenBot")).toBe(
      "QueenBot cannot play with this token",
    );
    // Singular "scope", because one is missing. The remedy still names all
    // three boxes: a replacement token needs the full set whatever the old
    // one carried.
    expect(scopeGapDetail(blockingGap)).toBe(
      "Missing scope bot:play — QueenUI cannot play games with this token, " +
        "and matchmaking will not work either. Create a new token at " +
        "lichess.org/account/oauth/token/create with Play-bot, Read-challenges " +
        "and Send-challenges ticked, then replace this account's token from " +
        "its Actions menu on Overview.",
    );
  });

  it("grades a playable token as a warning and an unplayable one as an error", () => {
    /*
     * The grade is the difference between "your bot works, your campaign does
     * not" and "nothing here will work". Collapsing them into one kind is the
     * mistake that would put a stored, unusable account behind a success tick.
     */
    expect(scopeGapNotice(matchmakingGap, "QueenBot").kind).toBe("warning");
    expect(scopeGapNotice(blockingGap, "QueenBot").kind).toBe("error");
    expect(scopeGapNotice(blockingGap, "QueenBot").message).toContain(
      "bot:play",
    );
  });
});

describe("the recorded gaps", () => {
  it("survives a restart, because the token does", () => {
    localStorage.setItem(
      tokenScopeStorageKey,
      JSON.stringify({
        queenbot: { missing: ["challenge:write"], canPlayGames: true },
      }),
    );
    expect(storedScopeGaps()).toStrictEqual({
      queenbot: { missing: ["challenge:write"], canPlayGames: true },
    });
  });

  it("drops entries it cannot trust rather than inventing a verdict", () => {
    // An empty `missing` is not a gap, a missing boolean is not a grade, and a
    // malformed record is not evidence that any token is broken.
    localStorage.setItem(
      tokenScopeStorageKey,
      JSON.stringify({
        good: { missing: ["bot:play"], canPlayGames: false },
        emptyList: { missing: [], canPlayGames: true },
        noGrade: { missing: ["bot:play"] },
        notAnObject: "bot:play",
      }),
    );
    expect(storedScopeGaps()).toStrictEqual({
      good: { missing: ["bot:play"], canPlayGames: false },
    });
  });

  it("starts clean on unparseable storage", () => {
    localStorage.setItem(tokenScopeStorageKey, "{not json");
    expect(storedScopeGaps()).toStrictEqual({});
  });
});
