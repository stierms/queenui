import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  addLichessAccount,
  dismissGameError,
  getSnapshot,
  setRunnerSettings,
  testRunnerConnection,
  updateLichessAccountToken,
} from "./commands";
import { emptySnapshot, type AddAccountResult } from "../types";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

/**
 * The runner IPC boundary, asserted on the payload itself.
 *
 * `set_runner_settings` and `test_runner_connection` fail any request carrying a
 * non-empty bearer ("Direct bearer entry is retired; use the one-time runner
 * pairing flow"), and cleartext remote HTTP is refused outright. A `token` or
 * `allowInsecureRemoteHttp` key in either payload is therefore never a feature —
 * it is a request that cannot succeed. `toStrictEqual` is the point: unlike
 * `toHaveBeenCalledWith` it does not treat an explicit `undefined` key as
 * absent, so a reintroduced argument fails here even when it is unset — and so
 * does an acknowledgement that starts arriving without being asked for.
 */
describe("runner IPC payloads", () => {
  beforeEach(() => vi.mocked(invoke).mockReset());

  it("saves the runner with only a mode and an endpoint", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);

    await setRunnerSettings("remote", "https://runner-host:17789");

    expect(invoke).toHaveBeenCalledOnce();
    const [command, payload] = vi.mocked(invoke).mock.calls[0];
    expect(command).toBe("set_runner_settings");
    expect(payload).toStrictEqual({
      mode: "remote",
      url: "https://runner-host:17789",
      // Unset: an acknowledgement is only ever sent as the answer to a refusal,
      // and a save that has not been refused anything must stay refusable.
      acknowledgedRunner: undefined,
    });
  });

  it("omits the endpoint entirely when switching to this computer", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);

    await setRunnerSettings("embedded");

    expect(vi.mocked(invoke).mock.calls[0][1]).toStrictEqual({
      mode: "embedded",
      url: undefined,
      acknowledgedRunner: undefined,
    });
  });

  it("acknowledges by naming the runner, not by asserting a boolean", async () => {
    /*
     * The one argument that turns a refusal into a save. `verify_remote_handover`
     * refuses a switch away from a remote runner that still has games or outgoing
     * challenges — or one it could not reach to ask — and clears the refusal only
     * when this equals that runner's own canonical base URL. So the wire name and
     * the value both matter: a `true` under any name acknowledges nothing, and the
     * URL is what stops an acknowledgement of one runner from authorizing another.
     *
     * Note the two URLs below are deliberately different. `url` is where the save
     * is going; `acknowledgedRunner` is the runner being *left*, which is the one
     * the refusal named.
     */
    vi.mocked(invoke).mockResolvedValue(undefined);

    await setRunnerSettings(
      "remote",
      "https://other-runner:17789",
      "https://runner-host:17789",
    );

    expect(vi.mocked(invoke).mock.calls[0][1]).toStrictEqual({
      mode: "remote",
      url: "https://other-runner:17789",
      acknowledgedRunner: "https://runner-host:17789",
    });
  });

  it("tests a connection with only the endpoint", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);

    await testRunnerConnection("https://runner-host:17789");

    expect(invoke).toHaveBeenCalledOnce();
    const [command, payload] = vi.mocked(invoke).mock.calls[0];
    expect(command).toBe("test_runner_connection");
    expect(payload).toStrictEqual({ url: "https://runner-host:17789" });
  });
});

describe("the account connect boundary", () => {
  beforeEach(() => vi.mocked(invoke).mockReset());

  it("hands the scope verdict back whole instead of discarding it", async () => {
    /*
     * `add_lichess_account` was typed `Promise<void>` while the command already
     * answered with the token's OAuth scopes. Lichess reports those on the
     * validation call and nowhere else — no snapshot field carries them — so a
     * `void` here is not a small inaccuracy: it throws away the only evidence
     * the app will ever have that a token cannot run matchmaking, which is how
     * a play-only token reached a live campaign and answered 403.
     *
     * `toStrictEqual` on the whole result is the point. Returning only
     * `result.account`, or only the boolean, type-checks and would leave the
     * three-way verdict unrecoverable at the call site.
     */
    const result: AddAccountResult = {
      account: {
        id: "queenbot",
        username: "QueenBot",
        engineId: "engine-1",
        rating: 2400,
        enabled: false,
      },
      scopes: ["bot:play", "preference:read"],
      missingForMatchmaking: ["challenge:read", "challenge:write"],
      canPlayGames: true,
    };
    vi.mocked(invoke).mockResolvedValue(result);

    expect(await addLichessAccount("lip_secret", "engine-1")).toStrictEqual(
      result,
    );
    expect(invoke).toHaveBeenCalledExactlyOnceWith("add_lichess_account", {
      request: { token: "lip_secret", engineId: "engine-1" },
    });
  });
});

describe("the token replacement boundary", () => {
  beforeEach(() => vi.mocked(invoke).mockReset());

  it("sends the account id beside the token, and returns the whole verdict", async () => {
    /*
     * Two things this payload has to get right, and both are load-bearing.
     *
     * The account id: the backend validates the pasted token against Lichess
     * and refuses it when it names a different account, instead of repointing
     * the profile at whoever the token belongs to. A payload of just a token
     * would leave nothing to compare against — the wrong-account refusal only
     * exists because the caller says which account it means.
     *
     * The answer: the same `AddAccountResult` the connect returns, kept whole.
     * A replacement is the only moment other than a connect when QueenUI ever
     * sees a token's OAuth scopes, so typing this `void` would let an operator
     * fix a revoked token and silently lose matchmaking with no way to find
     * out — the exact failure the scope verdict was added for.
     */
    const result: AddAccountResult = {
      account: {
        id: "queenbot",
        username: "QueenBot",
        engineId: "engine-1",
        rating: 2400,
        enabled: true,
      },
      scopes: ["bot:play"],
      missingForMatchmaking: ["challenge:read", "challenge:write"],
      canPlayGames: true,
    };
    vi.mocked(invoke).mockResolvedValue(result);

    expect(
      await updateLichessAccountToken("queenbot", "lip_replacement"),
    ).toStrictEqual(result);
    expect(invoke).toHaveBeenCalledExactlyOnceWith(
      "update_lichess_account_token",
      { accountId: "queenbot", token: "lip_replacement" },
    );
  });
});

describe("the game-error dismissal boundary", () => {
  beforeEach(() => vi.mocked(invoke).mockReset());

  it("names the game whose retained error is being forgotten", async () => {
    // Per game, never a "clear all": each retained error is a separate fact an
    // operator has or has not read, and the backend refuses an id it is not
    // holding ("No retained game error was found for …") rather than silently
    // succeeding.
    vi.mocked(invoke).mockResolvedValue(undefined);

    await dismissGameError("F5tD1jRk");

    expect(invoke).toHaveBeenCalledExactlyOnceWith("dismiss_game_error", {
      gameId: "F5tD1jRk",
    });
  });
});

describe("the snapshot fetch", () => {
  beforeEach(() => vi.mocked(invoke).mockReset());

  it("hands the stamp through instead of unwrapping the payload", async () => {
    /*
     * `get_snapshot` answers with the same envelope the snapshot event carries,
     * and the tempting simplification here — returning `payload` so callers keep
     * their old shape — would throw the stamp away at the one boundary that can
     * still see it, leaving a response from a runner this app has already left
     * indistinguishable from the current one's state. That is the bug the stamp
     * exists to stop, so the envelope crosses this boundary whole.
     */
    const envelope = {
      backendGeneration: 4,
      payload: emptySnapshot,
    };
    vi.mocked(invoke).mockResolvedValue(envelope);

    expect(await getSnapshot()).toStrictEqual(envelope);
    expect(invoke).toHaveBeenCalledExactlyOnceWith("get_snapshot");
  });
});
