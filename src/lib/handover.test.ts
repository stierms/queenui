import { describe, expect, it } from "vitest";
import { remoteHandoverRefusal } from "./handover";

/*
 * Both lists are copied from the Rust that produces them — `verify_remote_handover`
 * and `live_games_switch_error` in `src-tauri/src/lib.rs`,
 * `locally_unverifiable_outgoing_challenge_error` and `verify_authoritative_handover`
 * in `crates/queen-core/src/runtime.rs` — with the same interpolations their own
 * tests use. The point of the second list is the one this classification could
 * get dangerously wrong: offering to "confirm" a refusal that no acknowledgement
 * clears would let an operator press a button that does nothing but re-refuse,
 * on the screen that decides which machine plays their games.
 */
const RUNNER = "https://runner-host:17789";

const ACKNOWLEDGEABLE = [
  `The remote runner at ${RUNNER} is still playing 1 game. Confirm that it will keep playing there before switching runners.`,
  `The remote runner at ${RUNNER} is still playing 2 games. Confirm that they will keep playing there before switching runners.`,
  `The remote runner at ${RUNNER} still owns 1 outgoing challenge. Confirm that it will remain there before switching runners.`,
  `The remote runner at ${RUNNER} still owns 2 outgoing challenges. Confirm that they will remain there before switching runners.`,
  `The remote runner at ${RUNNER} is still playing 1 game and owns 1 outgoing challenge. Confirm that this work will remain there before switching runners.`,
  `The remote runner at ${RUNNER} is still playing 3 games and owns 2 outgoing challenges. Confirm that this work will remain there before switching runners.`,
  `Could not verify the remote runner at ${RUNNER}; it may still be playing games. Confirm that its games will keep running there before switching runners.`,
];

const NOT_ACKNOWLEDGEABLE = [
  // Live games on *this* computer: these have to end, not be acknowledged.
  "1 game is still being played from this computer; finish or resign them before switching to a runner.",
  "3 games are still being played from this computer; finish or resign them before switching to a runner.",
  // Outgoing challenges, locally known and campaign-driven.
  "An outgoing challenge to Opponent is still unresolved; cancel it or let it resolve before switching to a runner.",
  "2 outgoing challenges are still unresolved (One, Two); cancel them or let them resolve before switching to a runner.",
  "An outgoing challenge creation for bot against OtherOpponent is still uncertain; let QueenUI reconcile it before switching to a runner.",
  "A campaign challenge is still unresolved; cancel it or let it resolve before switching to a runner.",
  "2 campaign challenges are still unresolved; cancel them or let them resolve before switching to a runner.",
  // The authoritative Lichess checks.
  "Lichess account Bot still has 1 live game (game-1); finish or resign them before switching to a runner.",
  "Lichess account Bot still has 2 live games (game-1, game-2); finish or resign them before switching to a runner.",
  "Lichess account Bot still has 1 outgoing challenge (Opponent); cancel them or let them resolve before switching to a runner.",
  "Could not verify Lichess account Bot before switching runners; live games or outgoing challenges may still exist.",
  // Switch-machinery failures.
  "QueenUI is switching runners; retry in a moment",
  "The runner switch was interrupted; save runner settings again to recover the backend",
  "Runner settings were saved, but the switch could not complete; restarting QueenUI will retry it: QueenUI automation is already owned",
  "The active runner changed while pairing; save runner settings to adopt the new identity",
];

describe("remote-handover refusal", () => {
  it("recognises the game refusal in both its numbers", () => {
    expect(remoteHandoverRefusal(ACKNOWLEDGEABLE[0])?.kind).toBe("playing");
    expect(remoteHandoverRefusal(ACKNOWLEDGEABLE[1])?.kind).toBe("playing");
  });

  it("recognises the outgoing-challenge refusal in both its numbers", () => {
    /*
     * A challenge is not a game — nothing is being played, so "still playing"
     * would be false — but it will become a game on that machine when it is
     * accepted, which is the same work to leave behind and its own sentence.
     */
    expect(remoteHandoverRefusal(ACKNOWLEDGEABLE[2])?.kind).toBe("challenges");
    expect(remoteHandoverRefusal(ACKNOWLEDGEABLE[3])?.kind).toBe("challenges");
  });

  it("recognises the combined refusal as neither of its halves", () => {
    // One sentence about both, with a single closing clause ("this work")
    // because no pronoun covers games and challenges together. Classifying it
    // as the game-only refusal would title a dialog that under-reports what is
    // being left behind.
    expect(remoteHandoverRefusal(ACKNOWLEDGEABLE[4])?.kind).toBe(
      "playing-and-challenges",
    );
    expect(remoteHandoverRefusal(ACKNOWLEDGEABLE[5])?.kind).toBe(
      "playing-and-challenges",
    );
  });

  it("recognises the could-not-verify refusal as its own question", () => {
    // A different sentence with a different meaning — the counts are unknown,
    // not zero — so the dialog it raises has to ask a different question.
    expect(remoteHandoverRefusal(ACKNOWLEDGEABLE[6])?.kind).toBe("unverified");
  });

  it("extracts the runner URL every one of them names", () => {
    /*
     * The sentence is the only place that URL appears, and it is what the
     * acknowledgement has to carry: the backend clears the refusal only when
     * `acknowledgedRunner` equals the live remote's own canonical base URL. A
     * classification that recognised the question without recovering the address
     * would leave the panel with nothing to answer it with.
     */
    for (const refusal of ACKNOWLEDGEABLE) {
      expect(remoteHandoverRefusal(refusal)?.runner).toBe(RUNNER);
    }
  });

  it("extracts the URL as spelled, whatever shape the canonical form takes", () => {
    // Not parsed, not normalized, not compared against anything this panel
    // knows: the backend canonicalizes the endpoint it dials and compares
    // byte-for-byte, so the only correct answer is the spelling it used.
    for (const runner of [
      "http://127.0.0.1:17788",
      "https://runner.internal",
      "https://runner-host:17789/queen",
    ]) {
      expect(
        remoteHandoverRefusal(
          `The remote runner at ${runner} is still playing 1 game. Confirm that it will keep playing there before switching runners.`,
        )?.runner,
      ).toBe(runner);
      // The one sentence whose URL is not followed by a space.
      expect(
        remoteHandoverRefusal(
          `Could not verify the remote runner at ${runner}; it may still be playing games. Confirm that its games will keep running there before switching runners.`,
        )?.runner,
      ).toBe(runner);
    }
  });

  it("offers no acknowledgement for a refusal an acknowledgement cannot clear", () => {
    for (const refusal of NOT_ACKNOWLEDGEABLE) {
      expect(remoteHandoverRefusal(refusal)).toBeNull();
    }
  });

  it("is anchored, so a sentence that merely contains one does not qualify", () => {
    for (const message of ACKNOWLEDGEABLE) {
      expect(
        remoteHandoverRefusal(`Runner settings were saved, but: ${message}`),
      ).toBeNull();
      expect(remoteHandoverRefusal(`${message} And then some.`)).toBeNull();
    }
  });

  it("tolerates surrounding whitespace and nothing else", () => {
    expect(remoteHandoverRefusal(`\n${ACKNOWLEDGEABLE[0]}  `)?.kind).toBe(
      "playing",
    );
    expect(
      remoteHandoverRefusal(
        `The remote runner at ${RUNNER} is still playing some games. Confirm that they will keep playing there before switching runners.`,
      ),
    ).toBeNull();
    expect(
      remoteHandoverRefusal(
        `The remote runner at ${RUNNER} still owns several outgoing challenges. Confirm that they will remain there before switching runners.`,
      ),
    ).toBeNull();
  });
});
