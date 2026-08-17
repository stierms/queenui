import type { PairedRunner } from "../api/pairing";

/**
 * Pairing dialog state, as a reducer.
 *
 * The rules worth stating are all about what happens to a *secret* and to an
 * *error*:
 *
 *   - The setup code is one-use and short-lived. It is cleared from state the
 *     moment a pairing succeeds, so it does not sit in a React tree (or a
 *     component's props in a devtools snapshot) after it has been spent.
 *   - A failure keeps the operator's input, because the usual cause is a typo
 *     or an expired code and retyping the whole line helps nobody.
 *   - The backend's error text is stored verbatim and never rewritten. The
 *     runner is the authority on why an enrollment was refused — "expired",
 *     "already redeemed", "certificate does not match the pin" are all
 *     different problems with different fixes, and a friendly generic message
 *     would erase the difference.
 *   - Switching carrier or editing a field clears the previous error: an error
 *     that no longer describes what is in the form is worse than none.
 *
 * The validation below mirrors the payload/alias rules in TIER B §B1 so the
 * operator gets an immediate answer, and mirrors them *only* — Rust re-checks
 * everything, and its verdict is the one that counts.
 */

export type PairingCarrier = "ssh" | "code";

export type PairingState = {
  carrier: PairingCarrier;
  alias: string;
  code: string;
  pending: boolean;
  /** Set once, terminal: the dialog then shows the paired runner. */
  paired: PairedRunner | null;
  /** The backend's message, exactly as it was rejected. */
  error: string | null;
};

export const initialPairingState: PairingState = {
  // ssh is the primary carrier: the payload never enters the webview.
  carrier: "ssh",
  alias: "",
  code: "",
  pending: false,
  paired: null,
  error: null,
};

export type PairingAction =
  | { type: "carrier"; carrier: PairingCarrier }
  | { type: "alias"; value: string }
  | { type: "code"; value: string }
  | { type: "submit" }
  | { type: "paired"; runner: PairedRunner }
  | { type: "failed"; message: string }
  | { type: "reset" };

export function pairingReducer(
  state: PairingState,
  action: PairingAction,
): PairingState {
  switch (action.type) {
    case "carrier":
      return state.carrier === action.carrier
        ? state
        : { ...state, carrier: action.carrier, error: null };
    case "alias":
      return { ...state, alias: action.value, error: null };
    case "code":
      return { ...state, code: action.value, error: null };
    case "submit":
      return canSubmitPairing(state)
        ? { ...state, pending: true, error: null }
        : state;
    case "paired":
      return {
        ...state,
        pending: false,
        paired: action.runner,
        error: null,
        // The code is spent; do not keep it around.
        code: "",
      };
    case "failed":
      return { ...state, pending: false, error: action.message };
    case "reset":
      return initialPairingState;
  }
}

/**
 * Why this ssh alias cannot be used, or `null` when it can.
 *
 * The alias is passed to ssh as data, never through a shell, and the same
 * three rejections are enforced in Rust: a leading `-` would be read as an
 * option, whitespace would split it into two arguments, and `=` is how ssh
 * config options are written.
 */
export function sshAliasProblem(alias: string): string | null {
  const value = alias.trim();
  if (!value) return "Enter the ssh alias or host of the runner machine.";
  if (value.startsWith("-")) {
    return "An alias cannot start with “-”; ssh would read it as an option.";
  }
  if (/\s/.test(value)) return "An alias cannot contain spaces.";
  if (value.includes("=")) return "An alias cannot contain “=”.";
  // Checked by code point rather than by regex: a control character in a
  // pattern is itself a lint error, and rejecting them is the whole point.
  if (hasControlCharacter(value)) {
    return "An alias cannot contain control characters.";
  }
  return null;
}

function hasControlCharacter(value: string): boolean {
  for (const character of value) {
    const code = character.codePointAt(0) ?? 0;
    if (code < 0x20 || code === 0x7f) return true;
  }
  return false;
}

/** The scheme and path a v2 setup code starts with. */
const SETUP_CODE_PREFIX = "queenui://pair?";

/**
 * Why this setup code cannot be submitted, or `null` when it can.
 *
 * Shape only. The version, the parameters and the enrollment value itself are
 * parsed and judged by Rust — a second parser here would be a second opinion
 * about what is valid, and the two would drift.
 */
export function setupCodeProblem(code: string): string | null {
  const value = code.trim();
  if (!value) return "Paste the setup code printed by “queen-runner pair”.";
  if (/[\r\n]/.test(value)) {
    return "A setup code is a single line. Paste only the line that starts with queenui://pair.";
  }
  if (!value.startsWith(SETUP_CODE_PREFIX)) {
    return "That is not a setup code — it starts with queenui://pair.";
  }
  return null;
}

/** The problem with whichever carrier is selected, or `null`. */
export function pairingInputProblem(state: PairingState): string | null {
  return state.carrier === "ssh"
    ? sshAliasProblem(state.alias)
    : setupCodeProblem(state.code);
}

export function canSubmitPairing(state: PairingState): boolean {
  if (state.pending || state.paired) return false;
  return pairingInputProblem(state) === null;
}

/** The value to send for the selected carrier, trimmed as Rust expects it. */
export function pairingArgument(state: PairingState): string {
  return (state.carrier === "ssh" ? state.alias : state.code).trim();
}

const HEX_ONLY = /^[0-9a-f]+$/i;

/**
 * A certificate fingerprint, formatted for reading aloud against the runner's
 * terminal output: uppercase hex in groups of four.
 *
 * Separators the backend may already have used (`:`, spaces, `-`) are removed
 * first so one canonical grouping is shown. Anything that is not plain hex —
 * base64, say — is returned trimmed and untouched, because regrouping an
 * encoding this function does not understand would corrupt the one string the
 * operator is supposed to compare character by character.
 */
export function groupFingerprint(fingerprint: string): string {
  const raw = fingerprint.trim();
  const compact = raw.replace(/[\s:-]/g, "");
  if (!compact || !HEX_ONLY.test(compact)) return raw;
  return (compact.toUpperCase().match(/.{1,4}/g) ?? []).join(" ");
}
