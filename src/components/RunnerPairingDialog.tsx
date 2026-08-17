import { useReducer } from "react";
import { KeyRound, ShieldCheck, TerminalSquare } from "lucide-react";
import { pairRunnerFromPayload, pairRunnerViaSsh } from "../api/pairing";
import type { PairedRunner } from "../api/pairing";
import { errorText } from "../lib/errors";
import {
  canSubmitPairing,
  groupFingerprint,
  initialPairingState,
  pairingArgument,
  pairingInputProblem,
  pairingReducer,
} from "../lib/pairing";
import { Button, Dialog } from "../ui/primitives";

/**
 * Pairing a runner (TIER B §B1).
 *
 * Two carriers for the same one-use enrollment code:
 *
 *   - **Import over ssh** (primary). QueenUI's backend runs ssh itself, reads
 *     the setup code on the runner and redeems it there. The code never enters
 *     this window, so nothing on the desktop side can leak it.
 *   - **Paste setup code** (fallback, for a machine with no ssh access from
 *     here). The operator carries the line themselves; possession of an
 *     unredeemed code wins the race, so the channel they carry it over is part
 *     of the trust model — the copy says so rather than implying the code is
 *     harmless.
 *
 * Everything after the input is Rust's: version check, pin-verified TLS,
 * redeem transaction, credential storage. This dialog submits, then shows what
 * came back or exactly why it was refused.
 */
export function RunnerPairingDialog({
  open,
  onClose,
  onPaired,
  onFailed,
}: {
  open: boolean;
  onClose: () => void;
  /** A runner was paired; the panel re-reads its settings. */
  onPaired: (runner: PairedRunner) => void;
  /**
   * Pairing was refused. Not a no-op: the identity is committed before the
   * capability probe, and the adopt path writes the config before it can
   * discover that the active runner moved — so the panel re-reads rather than
   * trusting the copy it held. The message stays here; only the fact that it
   * failed leaves this dialog.
   */
  onFailed?: () => void;
}) {
  const [state, dispatch] = useReducer(pairingReducer, initialPairingState);
  const problem = pairingInputProblem(state);
  const canSubmit = canSubmitPairing(state);

  function close() {
    dispatch({ type: "reset" });
    onClose();
  }

  async function submit() {
    if (!canSubmit) return;
    const argument = pairingArgument(state);
    dispatch({ type: "submit" });
    try {
      const runner =
        state.carrier === "ssh"
          ? await pairRunnerViaSsh(argument)
          : await pairRunnerFromPayload(argument);
      dispatch({ type: "paired", runner });
      onPaired(runner);
    } catch (cause) {
      // Verbatim: the runner knows why it refused, and that reason is the
      // operator's only clue about which of half a dozen states they are in.
      dispatch({ type: "failed", message: errorText(cause) });
      onFailed?.();
    }
  }

  return (
    <Dialog.Root
      open={open}
      onOpenChange={(next) => {
        if (!next) close();
      }}
    >
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-20 bg-black/70 backdrop-blur-[7px]" />
        <Dialog.Content className="account-modal fixed top-1/2 left-1/2 z-[21] -translate-x-1/2 -translate-y-1/2 focus:outline-none">
          <div className="modal-head">
            <div className="modal-icon">
              <ShieldCheck size={20} />
            </div>
            <div>
              <span className="eyebrow">Pinned connection</span>
              <Dialog.Title>Pair a runner</Dialog.Title>
              <Dialog.Description>
                {state.paired
                  ? "QueenUI pinned this runner's certificate and stored its credential."
                  : "Run “queen-runner pair” on the runner machine, then bring the setup code here."}
              </Dialog.Description>
            </div>
            <Dialog.Close asChild>
              <Button
                variant="icon"
                className="text-lg leading-none"
                aria-label="Close"
              >
                ×
              </Button>
            </Dialog.Close>
          </div>

          {state.paired ? (
            <PairedSummary runner={state.paired} onDone={close} />
          ) : (
            <form
              onSubmit={(event) => {
                event.preventDefault();
                void submit();
              }}
            >
              <div className="account-form">
                <fieldset className="pairing-carriers">
                  <legend>How should the setup code get here?</legend>
                  <label>
                    <input
                      type="radio"
                      name="pairing-carrier"
                      value="ssh"
                      checked={state.carrier === "ssh"}
                      onChange={() =>
                        dispatch({ type: "carrier", carrier: "ssh" })
                      }
                    />
                    <span>
                      <strong>Import over ssh</strong>
                      <small>
                        QueenUI fetches and redeems the code on the runner. It
                        never passes through this window.
                      </small>
                    </span>
                  </label>
                  <label>
                    <input
                      type="radio"
                      name="pairing-carrier"
                      value="code"
                      checked={state.carrier === "code"}
                      onChange={() =>
                        dispatch({ type: "carrier", carrier: "code" })
                      }
                    />
                    <span>
                      <strong>Paste setup code</strong>
                      <small>
                        For a runner you cannot reach over ssh from this
                        machine.
                      </small>
                    </span>
                  </label>
                </fieldset>

                {state.carrier === "ssh" ? (
                  <label>
                    <span>ssh alias or host</span>
                    <input
                      autoFocus
                      autoComplete="off"
                      spellCheck={false}
                      value={state.alias}
                      placeholder="runner-host"
                      aria-label="ssh alias or host"
                      onChange={(event) =>
                        dispatch({ type: "alias", value: event.target.value })
                      }
                    />
                    <small>
                      QueenUI runs ssh directly, with a strict host-key policy —
                      an unknown or changed host key fails the pairing instead
                      of asking.
                    </small>
                  </label>
                ) : (
                  <label>
                    <span>Setup code</span>
                    <input
                      autoFocus
                      autoComplete="off"
                      spellCheck={false}
                      value={state.code}
                      placeholder="queenui://pair?v=2&url=…&fp=…&enroll=…"
                      aria-label="Setup code"
                      onChange={(event) =>
                        dispatch({ type: "code", value: event.target.value })
                      }
                    />
                    <small>
                      One line, valid for ten minutes and usable once. Anyone
                      who sees it before you use it can pair instead of you, so
                      carry it the way you would a password.
                    </small>
                  </label>
                )}

                {state.error && (
                  <p className="game-error pairing-error" role="alert">
                    {state.error}
                  </p>
                )}
                {!state.error && problem && state.pending === false && (
                  <p className="field-hint">{problem}</p>
                )}
              </div>
              <div className="modal-actions">
                <Dialog.Close asChild>
                  <Button variant="secondary">Cancel</Button>
                </Dialog.Close>
                <Button
                  type="submit"
                  variant="primary"
                  className="min-w-[130px]"
                  disabled={!canSubmit}
                >
                  {state.pending ? "Pairing…" : "Pair runner"}
                </Button>
              </div>
            </form>
          )}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/** What was actually paired: which machine answered, where, and on what pin. */
function PairedSummary({
  runner,
  onDone,
}: {
  runner: PairedRunner;
  onDone: () => void;
}) {
  const fingerprint = runner.certFingerprint?.trim();
  return (
    <>
      <div className="account-form">
        <div className="credential-note">
          <TerminalSquare size={17} />
          <p>
            <strong>{runner.hostname} answered</strong>
            <small>
              {runner.operatingSystem} · {runner.architecture} ·{" "}
              {runner.logicalCpus} logical CPUs
            </small>
            {runner.url && <small>Bound to {runner.url}</small>}
          </p>
        </div>
        {fingerprint ? (
          <label>
            <span>Pinned certificate fingerprint (SHA-256)</span>
            <input
              readOnly
              className="pairing-fingerprint"
              value={groupFingerprint(fingerprint)}
              aria-label="Pinned certificate fingerprint"
              onFocus={(event) => event.currentTarget.select()}
            />
            <small>
              Compare it with the fingerprint “queen-runner pair” printed. A
              certificate that does not match this pin is refused from now on —
              there is no path that accepts a different one.
            </small>
          </label>
        ) : (
          <div className="credential-note">
            <KeyRound size={17} />
            <p>
              <strong>The certificate pin was stored</strong>
              <small>
                This build cannot display it for comparison yet. Only a runner
                presenting that exact certificate will be talked to.
              </small>
            </p>
          </div>
        )}
      </div>
      <div className="modal-actions">
        <Button variant="primary" onClick={onDone}>
          Done
        </Button>
      </div>
    </>
  );
}
