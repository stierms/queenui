import { invoke } from "@tauri-apps/api/core";
import type { RunnerConnectionTest } from "../types";

/**
 * Runner pairing — the two enrollment carriers, isolated in one module.
 *
 * Both commands use the backend pairing contract. Everything that matters
 * happens on the Rust side: minting nothing, redeeming the one-use enrollment
 * code over pin-verified TLS, and storing `RunnerIdentity { url, cert_fp,
 * bearer }` in the platform credential store.
 *
 * **No bearer, and no enrollment secret produced by the runner, ever reaches
 * TypeScript.** The paste carrier hands Rust the one line the operator pasted
 * and gets back only a description of the machine that answered; the ssh
 * carrier never lets the payload into the webview at all. Neither the payload
 * nor any part of it is logged here.
 *
 * Backend command names are kept in the two constants below.
 */

/** Paste carrier: the operator pastes the `queenui://pair?v=2…` line. */
const PAIR_RUNNER_FROM_PAYLOAD = "pair_runner_from_payload";

/** Zero-copy carrier: Rust runs ssh itself and parses the payload there. */
const PAIR_RUNNER_VIA_SSH = "pair_runner_via_ssh";

/**
 * What a successful pairing reports back.
 *
 * `RunnerConnectionTest` proves *which machine* answered on the pinned channel.
 * `url` and `certFingerprint` are read defensively so the panel can show the
 * endpoint it is now bound to and the fingerprint it pinned. If either is
 * absent, the dialog says the pin was stored rather than inventing a value to
 * display.
 */
export type PairedRunner = RunnerConnectionTest & {
  url?: string | null;
  /** SHA-256 of the runner's DER certificate, as the backend pinned it. */
  certFingerprint?: string | null;
};

/** Redeems a pasted setup code. The string is passed straight through. */
export function pairRunnerFromPayload(payload: string): Promise<PairedRunner> {
  return invoke<PairedRunner>(PAIR_RUNNER_FROM_PAYLOAD, { payload });
}

/** Fetches and redeems a setup code over ssh, by alias. */
export function pairRunnerViaSsh(alias: string): Promise<PairedRunner> {
  return invoke<PairedRunner>(PAIR_RUNNER_VIA_SSH, { alias });
}
