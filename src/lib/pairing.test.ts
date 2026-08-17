import { describe, expect, it } from "vitest";
import {
  canSubmitPairing,
  groupFingerprint,
  initialPairingState,
  pairingArgument,
  pairingInputProblem,
  pairingReducer,
  setupCodeProblem,
  sshAliasProblem,
  type PairingState,
} from "./pairing";
import type { PairedRunner } from "../api/pairing";

const runner: PairedRunner = {
  hostname: "runner-host",
  operatingSystem: "linux",
  architecture: "x86_64",
  logicalCpus: 32,
  url: "https://runner-host.lan:17788",
  certFingerprint:
    "3f2a9c1b4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f7081920a1b2c3d4e5f",
};

function state(overrides: Partial<PairingState> = {}): PairingState {
  return { ...initialPairingState, ...overrides };
}

describe("pairing carrier defaults", () => {
  it("starts on the carrier that never lets the code into the webview", () => {
    expect(initialPairingState.carrier).toBe("ssh");
    expect(initialPairingState.paired).toBeNull();
    expect(canSubmitPairing(initialPairingState)).toBe(false);
  });

  it("clears a stale error when the carrier or the input changes", () => {
    const failed = state({ error: "code expired", alias: "runner-host" });
    expect(
      pairingReducer(failed, { type: "carrier", carrier: "code" }).error,
    ).toBeNull();
    expect(
      pairingReducer(failed, { type: "alias", value: "newrig" }).error,
    ).toBeNull();
    // Selecting the carrier that is already selected is not a change.
    expect(pairingReducer(failed, { type: "carrier", carrier: "ssh" })).toBe(
      failed,
    );
  });
});

describe("ssh alias validation", () => {
  it("rejects exactly what ssh would misread", () => {
    expect(sshAliasProblem("")).toMatch(/Enter the ssh alias/);
    expect(sshAliasProblem("-oProxyCommand=x")).toMatch(/cannot start with/);
    expect(sshAliasProblem("old rig")).toMatch(/spaces/);
    expect(sshAliasProblem("Host=runner-host")).toMatch(/=/);
    expect(sshAliasProblem("old\u0007rig")).toMatch(/control characters/);
  });

  it("accepts an ordinary alias or host", () => {
    expect(sshAliasProblem("runner-host")).toBeNull();
    expect(sshAliasProblem("  192.168.0.36  ")).toBeNull();
    expect(sshAliasProblem("operator@runner-host.lan")).toBeNull();
  });
});

describe("setup code validation", () => {
  it("checks the shape only, and leaves the verdict to the backend", () => {
    expect(setupCodeProblem("")).toMatch(/Paste the setup code/);
    expect(setupCodeProblem("https://runner-host/pair")).toMatch(
      /not a setup code/,
    );
    expect(
      setupCodeProblem("queenui://pair?v=2&url=a\nqueenui://pair?v=2"),
    ).toMatch(/single line/);
    // A v1 code and an unknown parameter are Rust's to reject: this side
    // deliberately has no second parser to disagree with.
    expect(setupCodeProblem("queenui://pair?v=1&url=a&enroll=b")).toBeNull();
    expect(
      setupCodeProblem("  queenui://pair?v=2&url=a&fp=b&enroll=c  "),
    ).toBeNull();
  });
});

describe("submission gating", () => {
  it("submits only a valid input, and only once", () => {
    const ready = state({ alias: "runner-host" });
    expect(canSubmitPairing(ready)).toBe(true);
    const submitting = pairingReducer(ready, { type: "submit" });
    expect(submitting.pending).toBe(true);
    expect(canSubmitPairing(submitting)).toBe(false);
    // A second submit while one is in flight changes nothing.
    expect(pairingReducer(submitting, { type: "submit" })).toBe(submitting);
  });

  it("ignores a submit the carrier's input cannot satisfy", () => {
    const bad = state({ carrier: "code", code: "not-a-code" });
    expect(pairingInputProblem(bad)).toMatch(/not a setup code/);
    expect(pairingReducer(bad, { type: "submit" })).toBe(bad);
  });

  it("sends the trimmed value of the selected carrier", () => {
    expect(pairingArgument(state({ alias: " runner-host " }))).toBe(
      "runner-host",
    );
    expect(
      pairingArgument(
        state({ carrier: "code", code: " queenui://pair?v=2&enroll=x " }),
      ),
    ).toBe("queenui://pair?v=2&enroll=x");
  });
});

describe("what survives an outcome", () => {
  it("drops the spent setup code once pairing succeeds", () => {
    const submitted = pairingReducer(
      state({ carrier: "code", code: "queenui://pair?v=2&enroll=secret" }),
      { type: "submit" },
    );
    const done = pairingReducer(submitted, { type: "paired", runner });

    // One-use and already redeemed: keeping it in component state would leave
    // a live-looking secret in the tree for as long as the dialog exists.
    expect(done.code).toBe("");
    expect(done.pending).toBe(false);
    expect(done.paired).toEqual(runner);
    expect(canSubmitPairing(done)).toBe(false);
  });

  it("keeps the input and the backend's own words after a failure", () => {
    const submitted = pairingReducer(state({ alias: "runner-host" }), {
      type: "submit",
    });
    const failed = pairingReducer(submitted, {
      type: "failed",
      message: "enrollment code already redeemed",
    });

    expect(failed.error).toBe("enrollment code already redeemed");
    expect(failed.alias).toBe("runner-host");
    expect(failed.pending).toBe(false);
    expect(failed.paired).toBeNull();
    // Retrying the corrected input has to be possible.
    expect(canSubmitPairing(failed)).toBe(true);
  });

  it("returns to the initial state on reset", () => {
    const dirty = pairingReducer(
      state({ carrier: "code", code: "queenui://pair?v=2", error: "nope" }),
      { type: "paired", runner },
    );
    expect(pairingReducer(dirty, { type: "reset" })).toEqual(
      initialPairingState,
    );
  });
});

describe("fingerprint formatting", () => {
  it("groups hex in fours, uppercase, for reading against the runner", () => {
    expect(groupFingerprint("3f2a9c1b")).toBe("3F2A 9C1B");
    expect(groupFingerprint("3f:2a:9c:1b")).toBe("3F2A 9C1B");
    expect(groupFingerprint(" 3F2A9C1B ")).toBe("3F2A 9C1B");
    const long = groupFingerprint(runner.certFingerprint ?? "");
    expect(long.split(" ")).toHaveLength(16);
    expect(long.replace(/ /g, "")).toBe(
      (runner.certFingerprint ?? "").toUpperCase(),
    );
  });

  it("leaves an encoding it does not understand exactly as it was", () => {
    // Regrouping base64 would corrupt the one string the operator compares
    // character by character.
    const base64 = "PyqcG01ebwaBkqO0xdbn+AkaKzxNXm9wgZIKGyw9Tl8=";
    expect(groupFingerprint(base64)).toBe(base64);
    expect(groupFingerprint("  ")).toBe("");
  });
});
