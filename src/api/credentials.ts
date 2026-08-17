import { invoke } from "@tauri-apps/api/core";

/**
 * Credential lifecycle — every IPC call that *removes* a stored secret.
 *
 * Isolated in its own module so a backend command rename touches these two
 * constants and nothing else in the frontend.
 */

/** Deletes the account and its Lichess token from the OS credential store. */
const REMOVE_LICHESS_ACCOUNT = "remove_lichess_account";

/** Forgets the saved bearer token for the remote runner endpoint. */
const FORGET_RUNNER_CREDENTIAL = "forget_runner_credential";

export function removeLichessAccount(accountId: string): Promise<void> {
  return invoke<void>(REMOVE_LICHESS_ACCOUNT, { accountId });
}

export function forgetRunnerCredential(): Promise<void> {
  return invoke<void>(FORGET_RUNNER_CREDENTIAL);
}

/**
 * Where a Lichess token physically lives, which depends on which runner is
 * *active* — not on which one is saved. In remote mode the token is handed to
 * the runner and stored by it, on that machine; the local credential store is
 * not involved at all. The account dialog said "Windows Credential Manager"
 * unconditionally, which was wrong in exactly the case where it mattered.
 */
export function tokenStorageCopy(remote: boolean, runnerUrl?: string | null) {
  if (!remote) {
    return {
      where: "Windows Credential Manager",
      sentence:
        "The token is validated, then stored in Windows Credential Manager on this PC.",
      note: "QueenUI never writes this token to its settings file or frontend storage.",
    };
  }
  const host = runnerUrl?.trim() ? ` (${runnerUrl.trim()})` : "";
  return {
    where: `the runner machine${host}`,
    sentence: `The token is validated, then sent to the game runner${host} and stored there as a private file owned by the runner's service user.`,
    note: "It is not kept on this PC. Removing the account here removes it from the runner too.",
  };
}
