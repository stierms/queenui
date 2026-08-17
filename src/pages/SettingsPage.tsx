import { useCallback, useEffect, useState } from "react";
import {
  CircleDot,
  Gauge,
  HardDrive,
  KeyRound,
  Palette,
  Plus,
  ScrollText,
  Server,
  ShieldAlert,
  ShieldCheck,
  Trash2,
  Volume2,
  VolumeX,
  Wifi,
  Zap,
} from "lucide-react";
import { ChessPiece, type PieceSetId } from "../ChessPiece";
import * as commands from "../api/commands";
import * as credentials from "../api/credentials";
import { onLogsUpdated } from "../api/events";
import { AppearanceControls } from "../components/appearance";
import { RunnerPairingDialog } from "../components/RunnerPairingDialog";
import {
  boardAppearanceStyle,
  boardThemes,
  pieceSets,
  type BoardThemeId,
} from "../lib/appearance";
import { canonicalEndpoint } from "../lib/endpoint";
import { errorText } from "../lib/errors";
import { formatBytes } from "../lib/format";
import {
  remoteHandoverRefusal,
  type RemoteHandoverRefusal,
  type RemoteHandoverRefusalKind,
} from "../lib/handover";
import {
  defaultTimeControls,
  timeControlCategory,
  timeControlValue,
} from "../lib/timeControls";
import {
  runnerMode,
  type LogsOverview,
  type RunnerConnectionTest,
  type RunnerMode,
  type RunnerSettingsView,
  type TimeControl,
} from "../types";
import { Button, ConfirmDialog, Switch } from "../ui/primitives";

function TimeControlEditorRow({
  control,
  index,
  canRemove,
  onChange,
  onRemove,
}: {
  control: TimeControl;
  index: number;
  canRemove: boolean;
  onChange: (control: TimeControl) => void;
  onRemove: () => void;
}) {
  const [minutes, setMinutes] = useState(String(control.limitMinutes));
  const [increment, setIncrement] = useState(String(control.increment));

  // Adjust the editable strings during render when the committed preset
  // changes (see react.dev "adjusting state when props change").
  const [prevControl, setPrevControl] = useState(control);
  if (
    prevControl.limitMinutes !== control.limitMinutes ||
    prevControl.increment !== control.increment
  ) {
    setPrevControl(control);
    setMinutes(String(control.limitMinutes));
    setIncrement(String(control.increment));
  }

  function commit() {
    const next = {
      limitMinutes: Math.min(
        180,
        Math.max(1, Math.round(Number(minutes) || control.limitMinutes)),
      ),
      increment: Math.min(60, Math.max(0, Math.round(Number(increment) || 0))),
    };
    setMinutes(String(next.limitMinutes));
    setIncrement(String(next.increment));
    onChange(next);
  }

  return (
    <div className="time-control-editor-row">
      <span className="time-control-position">{index + 1}</span>
      <input
        type="number"
        min="1"
        max="180"
        value={minutes}
        aria-label={`Preset ${index + 1} minutes`}
        onChange={(event) => setMinutes(event.target.value)}
        onBlur={commit}
        onKeyDown={(event) => {
          if (event.key === "Enter") event.currentTarget.blur();
        }}
      />
      <i>+</i>
      <input
        type="number"
        min="0"
        max="60"
        value={increment}
        aria-label={`Preset ${index + 1} increment`}
        onChange={(event) => setIncrement(event.target.value)}
        onBlur={commit}
        onKeyDown={(event) => {
          if (event.key === "Enter") event.currentTarget.blur();
        }}
      />
      <small>{timeControlCategory(control)}</small>
      <button
        type="button"
        className="remove-time-control"
        aria-label={`Remove preset ${index + 1}`}
        disabled={!canRemove}
        onClick={onRemove}
      >
        <Trash2 size={14} />
      </button>
    </div>
  );
}

const MB_PER_GB = 1024;

/**
 * How the panel names a runner in prose.
 *
 * The remote form carries the endpoint: a machine that has paired with more
 * than one runner over its lifetime has no idea which "the remote runner" means,
 * and this label is used where the sentence decides what the operator does next.
 */
function runnerTargetLabel(mode: RunnerMode, url: string): string {
  return mode === "remote" ? `the runner at ${url}` : "this computer's engine";
}

/**
 * The runner executing games right now.
 *
 * `RunnerSettingsView` reports the active *mode* and no active URL, so the
 * remote form cannot name an address here — `url` is the *configured* endpoint,
 * and in every state where this label is needed that is precisely the runner
 * which is **not** in use.
 */
function activeRunnerLabel(settings: RunnerSettingsView): string {
  return runnerMode(settings.activeMode) === "remote"
    ? "the remote runner"
    : "this computer";
}

/**
 * The receipt for a completed save.
 *
 * Saving is the switch now — `set_runner_settings_inner` drains the running
 * backend and installs the replacement before it returns — so this reports the
 * runner that is executing games rather than that a file was written.
 *
 * `restartRequired` cannot be true on this path: every `Ok` return publishes the
 * active runner from the config it just saved, which is what makes the flag
 * false. It is still the backend's own account of whether the configured runner
 * is in use, so if it ever says "not in use", this must not claim a switch.
 */
function savedRunnerNotice(
  next: RunnerSettingsView,
  switched: boolean,
): string {
  const target = runnerTargetLabel(runnerMode(next.mode), next.url ?? "");
  if (next.restartRequired) {
    return `Runner settings saved, but QueenUI is still running games on ${activeRunnerLabel(next)}.`;
  }
  return switched
    ? `Switched to ${target}.`
    : `Runner settings saved. QueenUI is already using ${target}.`;
}

/**
 * The question the backend's acknowledgement refusal is asking.
 *
 * The two directions of a switch are not symmetric. Embedded to remote
 * *abandons* live games, so the backend refuses it outright and there is nothing
 * to confirm ("finish or resign them before switching to a runner"). Leaving a
 * remote runner abandons nothing: it is a separate process on another machine
 * playing accounts it enabled itself, and the switch only stops this desktop
 * from dispatching to it. Its games keep going, and the challenges it has sent
 * out still become its games when they are accepted — what vanishes is the view
 * of them from here, which is exactly the kind of disappearance an operator
 * reads as a crash unless something said so first. That is what is being
 * acknowledged, and the *description* is the backend's own sentence; only the
 * question in the title is the panel's, so it says which of the four refusals is
 * on screen.
 *
 * Four, not two, because the runner reports games and outgoing challenges
 * separately (`handover_inventory`) and the refusal is written from whichever it
 * found. A challenge is not a game yet — nothing is being played, and "still
 * playing" would be false — but it is work that will resolve on that machine, so
 * it is the same question about a different noun.
 */
function acknowledgementTitle(kind: RemoteHandoverRefusalKind): string {
  switch (kind) {
    case "playing":
      return "Switch runners while the remote runner is still playing?";
    case "challenges":
      return "Switch runners while the remote runner still has challenges out?";
    case "playing-and-challenges":
      return "Switch runners while the remote runner is still playing and has challenges out?";
    case "unverified":
      return "Switch runners without reaching the remote runner?";
  }
}

function RunnerSettingsPanel({
  settings,
  loadError,
  onSettingsChange,
}: {
  settings: RunnerSettingsView | null;
  loadError: string | null;
  onSettingsChange: (settings: RunnerSettingsView) => void;
}) {
  const [mode, setMode] = useState<RunnerMode>("embedded");
  // Empty until pairing (or a saved config) supplies one. A prefilled loopback
  // address suggested an endpoint this machine has no credential for.
  const [url, setUrl] = useState("");
  const [pairingOpen, setPairingOpen] = useState(false);
  const [testing, setTesting] = useState(false);
  const [saving, setSaving] = useState(false);
  const [forgetting, setForgetting] = useState(false);
  const [confirmingForget, setConfirmingForget] = useState(false);
  /*
   * The backend's refusal, held while the operator answers it. Every field comes
   * from the same sentence: `kind` is which question it asked, `runner` is the
   * URL it named — which is what the acknowledgement has to be — and `message` is
   * the sentence itself, which is what the dialog shows. There is no cached count
   * here to go stale, because there is no count of this panel's own any more, and
   * the URL cannot go stale either: it is replaced wholesale by every refusal,
   * including the one a stale acknowledgement earns.
   */
  const [acknowledgement, setAcknowledgement] = useState<
    (RemoteHandoverRefusal & { message: string }) | null
  >(null);
  const [testResult, setTestResult] = useState<RunnerConnectionTest | null>(
    null,
  );
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  // The panel edits a copy of the settings App owns; adopt it whenever the
  // owner's value changes (see react.dev "adjusting state when props change").
  const [adopted, setAdopted] = useState<RunnerSettingsView | null>(null);
  if (settings && settings !== adopted) {
    const previous = adopted;
    setAdopted(settings);
    /*
     * The form follows the saved target only when that target actually changed.
     * Every failed save re-reads the settings now, and a re-read that reports
     * the same mode and URL still arrives as a new object — which used to land
     * here and reset the form, throwing away the very selection the operator had
     * just been refused for ("…finish or resign them before switching to a
     * runner") and leaving them to make it again with the refusal on screen.
     */
    // `mode` is `string` in the generated contract; narrowed once, here.
    if (previous?.mode !== settings.mode) setMode(runnerMode(settings.mode));
    if (settings.url && settings.url !== previous?.url) setUrl(settings.url);
  }

  /*
   * The bearer and the certificate pin exist only as a pairing record, and the
   * backend refuses to use that record for any endpoint other than the one it
   * was issued for. So a different address is not a setting to save — it is a
   * runner to pair with. Nothing here can submit a credential: the desktop
   * commands reject a bearer outright ("Direct bearer entry is retired").
   */
  const savedUrl = settings?.url?.trim() ?? "";
  const requestedUrl = url.trim();
  /*
   * Compared by canonical spelling, not by keystrokes. The backend stores and
   * dials the canonicalized endpoint, so `https://runner:443` and a trailing
   * slash are the *same* runner — and the raw comparison that used to be here
   * called them a different one and sent an already-paired operator back
   * through pairing. `canonicalEndpoint` mirrors the Rust rules and yields
   * `null` for anything the Rust would refuse; the raw equality stays as the
   * first clause so an endpoint that cannot be canonicalized still compares
   * equal to itself instead of being reported as changed.
   */
  const savedEndpoint = canonicalEndpoint(savedUrl);
  const sameRunner =
    requestedUrl === savedUrl ||
    (savedEndpoint !== null &&
      savedEndpoint === canonicalEndpoint(requestedUrl));
  const urlChanged = Boolean(savedUrl) && !sameRunner;
  /*
   * The endpoint and the identity are two different facts on disk, and only
   * `paired` reports the second one: a pre-pairing record leaves a URL behind
   * with no usable credential, and forgetting the credential leaves the URL in
   * the config. Deriving readiness from the URL alone therefore enabled Test
   * and Save for a runner the backend was guaranteed to refuse.
   */
  const paired = settings?.paired ?? false;
  const needsPairing = mode === "remote" && (!paired || urlChanged);
  /*
   * Whether saving will also swap the backend, derived exactly as the Rust does
   * it (`set_runner_settings_inner`):
   *
   *   switch_required = config.mode != active.mode
   *                     || (config.mode == "remote" && config.url != active.url)
   *
   * The view carries no active URL, so the remote-to-remote case is read off
   * `restartRequired` — which *is* `configured != active` — and that is sound
   * here because Save is disabled while the requested URL differs from the
   * configured one, so the URL about to be submitted is the configured one.
   * A wrong guess only picks between two true sentences ("Switching to…" vs
   * "Saving…"), never between a claim and its opposite.
   */
  const switchExpected =
    settings !== null &&
    (mode !== runnerMode(settings.activeMode) ||
      (mode === "remote" && settings.restartRequired));
  /*
   * What the pending save is actually doing. The drain-and-swap takes as long as
   * the outgoing backend needs to stop, and "Saving…" alone made a multi-second
   * runner switch look like a wedged button.
   */
  const pendingMessage = !saving
    ? null
    : switchExpected
      ? `Switching to ${runnerTargetLabel(mode, requestedUrl)}…`
      : "Saving the runner settings…";

  async function testConnection() {
    setTesting(true);
    setError(null);
    setNotice(null);
    setTestResult(null);
    try {
      setTestResult(await commands.testRunnerConnection(url));
    } catch (cause) {
      setError(
        `Could not reach the runner at ${url} — ${errorText(cause)}. Check that queen-runner is running on that machine and that this is the address QueenUI paired with.`,
      );
    } finally {
      setTesting(false);
    }
  }

  /**
   * Saves, and asks only what the backend asked.
   *
   * This panel used to gate the save itself, on the live-game count it was
   * handed from the snapshot on screen: if a remote runner appeared to be
   * playing, it raised its own confirmation *before* calling the backend. Two
   * things were wrong with that. The count was this app's copy of the runner's
   * state, which is exactly the thing a degraded link makes stale — it could
   * ask about games that had finished, or say nothing about games it had never
   * seen. And it only knew one direction: remote to *embedded*. A switch from
   * one remote runner to another passed straight through, abandoning the first
   * runner's boards with no question asked at all.
   *
   * The backend now decides, by asking the runner itself
   * (`verify_remote_handover`), and refuses the save until the answer is
   * acknowledged. So the confirmation moved to where the refusal arrives: the
   * sentence in the dialog is the backend's, the acknowledgement is a repeat of
   * the same save carrying the runner URL that sentence named, and both
   * directions are covered because the backend does not care which one it is.
   *
   * `acknowledgedRunner` is that URL and nothing else, and it is the confirm
   * handler below that supplies it — from the refusal being answered right now,
   * never from a remembered one — because the backend accepts it only for the
   * runner currently in place. That binding is deliberate: a boolean
   * acknowledged "the switch", so it outlived the runner it was about and would
   * have waved a *replaced* runner's games through unread.
   */
  async function saveSettings(acknowledgedRunner?: string) {
    setSaving(true);
    setError(null);
    setNotice(null);
    setAcknowledgement(null);
    try {
      const next = await commands.setRunnerSettings(
        mode,
        mode === "remote" ? url : undefined,
        // Sent only as the answer to a question, never as a default: an
        // unacknowledged save must be refusable, which is the whole mechanism.
        acknowledgedRunner,
      );
      onSettingsChange(next);
      /*
       * Saving used to report nothing at all, so a successful save was
       * indistinguishable from a dead button — on the one panel that decides
       * which machine runs every game. It then reported a write ("Runner
       * saved."), which is false by omission now that the same call performs the
       * switch.
       */
      setNotice(savedRunnerNotice(next, switchExpected));
    } catch (cause) {
      /*
       * The backend's own sentence, verbatim and alone.
       *
       * Any wrapper the panel could add here is a claim about what was written,
       * and the failures now land on *both* sides of the save point: the
       * live-games refusal and "QueenUI is switching runners; retry in a moment"
       * write nothing, while "Runner settings were saved, but the switch could
       * not complete…" and the embedded credential-cleanup failure both happen
       * with the new config already on disk. Only the message knows which one it
       * is — which is why the old blanket "The previously saved runner is still
       * in use." now denies the very sentence it is appended to.
       */
      const message = errorText(cause);
      setError(message);
      /*
       * Four of those refusals are questions rather than obstacles, and the
       * dialog they raise carries this same sentence. It stays in the error slot
       * underneath: cancelling is a decision not to switch, and the reason the
       * switch did not happen is the sentence the backend wrote — not a summary
       * of it this panel would have to invent.
       *
       * An acknowledged save that is refused *again* is asked again, and that is
       * not a loop with no exit — it is the acknowledgement being URL-bound doing
       * its job. The only way to reach it is for the runner in place to have
       * changed since the sentence being answered was written, and the new
       * refusal names the runner that is actually there. Suppressing it (which is
       * what this did while the acknowledgement was a boolean) would report the
       * refusal in the error slot while hiding the one question that clears it.
       * Answering it sends the new URL, because that is the refusal now held.
       */
      const refusal = remoteHandoverRefusal(message);
      if (refusal) setAcknowledgement({ ...refusal, message });
      /*
       * Same reason the panel re-reads after pairing: once a write may have
       * landed, its own copy of the settings is a guess — and after a failed
       * live switch the config *did* change, which is what flips
       * `restartRequired` and makes the banner below true. Refusals return
       * settings that are unchanged, and adoption ignores an unchanged target,
       * so this costs the operator's pending selection nothing.
       */
      try {
        onSettingsChange(await commands.getRunnerSettings());
      } catch {
        // The save failure is the more important of the two; keep it.
      }
    } finally {
      setSaving(false);
    }
  }

  /*
   * Pairing stores the endpoint, the certificate pin and the bearer on the
   * Rust side in one record, so the panel's own idea of the runner is stale the
   * moment it succeeds — re-read it rather than guessing what was saved.
   */
  async function adoptPairedRunner() {
    setError(null);
    setTestResult(null);
    /*
     * The success notice used to be set *before* the re-read was awaited, so a
     * failed re-read rendered "Paired." directly above the failure — two
     * contradictory claims at once. Nothing is announced until it is true.
     */
    try {
      const next = await commands.getRunnerSettings();
      onSettingsChange(next);
      /*
       * Pairing is a switch in exactly one case, so neither sentence may be
       * hard-coded. `pair_and_store` redeems the code, stores the identity and
       * writes the config; then, *if the runner it just paired with is already
       * the active one* — the same canonical endpoint, or an active remote whose
       * backend slot is unavailable — it publishes a new backend on the rotated
       * credential and adopts it live. Any other case (pairing a different
       * runner, or pairing while this computer is playing) writes the config and
       * stops, leaving the paired runner saved but not in use.
       *
       * `restartRequired` (configured != active) is the backend's own report of
       * which of those happened, which is why it picks the sentence rather than
       * an assumption about what pairing does. Announcing "Paired." alone left
       * the difference to a banner the operator has to find, on the reading that
       * pairing had already moved their games.
       */
      setNotice(
        next.restartRequired
          ? `Paired. The runner's certificate is pinned to this machine and the endpoint is saved, but games keep running on ${activeRunnerLabel(next)} until you save the runner.`
          : "Paired. The runner's certificate is pinned to this machine.",
      );
    } catch (cause) {
      setError(
        `Paired, but the saved runner could not be read back — ${errorText(cause)}. Reopen Settings to confirm what was stored.`,
      );
    }
  }

  /*
   * A refused pairing is not always a no-op, so the panel re-reads instead of
   * assuming its own copy survived.
   *
   * `pair_and_store` commits the identity *before* the capability probe (on
   * rotation the old bearer is already dead by then), and the adopt path writes
   * the config before it can discover that the active runner moved underneath
   * it — the failure that answers "The active runner changed while pairing;
   * save runner settings to adopt the new identity". After that one the disk
   * says remote, the runner playing games is this computer, and only a re-read
   * makes the banner below say so and Save mean what the sentence promises.
   */
  async function adoptPairingFailure() {
    try {
      onSettingsChange(await commands.getRunnerSettings());
    } catch {
      // The pairing failure is the more important of the two, and the dialog is
      // still showing it. Nothing here has claimed anything about the runner.
    }
  }

  async function forgetCredential() {
    setConfirmingForget(false);
    setForgetting(true);
    setError(null);
    setNotice(null);
    try {
      await credentials.forgetRunnerCredential();
      /*
       * The identity is gone, so `paired` in the settings this panel was handed
       * is now false — and until it is re-read, Test and Save stay enabled for a
       * runner that has no credential left. Re-read rather than guess, exactly
       * as pairing adoption does; the endpoint stays in the config, so what
       * comes back is the unpaired-URL state.
       */
      try {
        onSettingsChange(await commands.getRunnerSettings());
        /*
         * Exactly what the command does: it deletes the stored records. It does
         * not close the `RunnerClient` this session already opened, so claiming
         * a disconnection here would be a claim the process cannot keep. What
         * *does* close it is a switch away from that runner — which this panel
         * now performs without a restart, so a restart is no longer the only
         * ending this sentence can offer.
         */
        setNotice(
          "The runner's bearer token and pinned certificate were deleted from this computer. A connection already running keeps running until you switch runners or QueenUI restarts.",
        );
      } catch (readback) {
        setError(
          `The runner's bearer token and pinned certificate were deleted from this computer, but the runner settings could not be read back — ${errorText(readback)}. Reopen Settings to confirm what is stored.`,
        );
      }
    } catch (cause) {
      setError(
        `Could not delete the saved runner credential — ${errorText(cause)}. It is still stored on this computer.`,
      );
    } finally {
      setForgetting(false);
    }
  }

  return (
    <section className="panel settings-panel runner-settings-panel">
      <div className="panel-heading">
        <div>
          <span className="eyebrow">Execution</span>
          <h2>Game runner</h2>
        </div>
        <Server size={19} />
      </div>
      <p className="time-control-intro">
        Keep games on this computer, or let a headless runner own Lichess and
        the engines on another machine.
      </p>
      <div className="form-grid runner-settings-form">
        <label>
          <span>Execution target</span>
          {/* Frozen during the swap: the backend holds a change gate and answers
              a second command with "QueenUI is switching runners; retry in a
              moment", so a live control here could only collect that. */}
          <select
            value={mode}
            disabled={saving}
            onChange={(event) => {
              setMode(runnerMode(event.target.value));
              setTestResult(null);
            }}
          >
            <option value="embedded">This computer</option>
            <option value="remote">Remote runner</option>
          </select>
        </label>
      </div>
      {/* Saving this mode deletes the pairing record on the Rust side — the
          same record "Forget the paired runner" removes — so the switch says
          so before it is saved rather than after. */}
      {mode === "embedded" && Boolean(savedUrl) && (
        <p className="field-hint">
          {paired
            ? "Saving this also deletes the paired runner's bearer token and pinned certificate from this computer; returning to that runner means pairing with it again."
            : `Saving this also clears the leftover record for ${savedUrl}. It holds no credential QueenUI can use, so no working connection is lost.`}
        </p>
      )}
      {mode === "remote" && (
        <>
          {/*
           * Pairing is the only way in: the runner's certificate is pinned
           * during enrollment and a bearer is only ever minted at the other end
           * of that pin-verified channel. There is no hand-entered credential
           * to fall back to — the desktop commands reject one. Which is why a
           * saved endpoint is not a paired runner, and this heading must not
           * present one as the other: `paired` is the only fact that says
           * whether an identity this machine can actually use exists.
           */}
          <div className="settings-note runner-pairing-note">
            {savedUrl && !paired ? (
              <ShieldAlert size={15} />
            ) : (
              <ShieldCheck size={15} />
            )}
            <p>
              <strong>
                {!savedUrl
                  ? "No runner paired yet"
                  : paired
                    ? `Runner endpoint ${savedUrl}`
                    : `Not paired with ${savedUrl}`}
              </strong>
              <small>
                {savedUrl && !paired ? (
                  <>
                    This address is saved, but the pinned certificate and bearer
                    token that authorize it are not on this computer — an
                    address on its own cannot reach a runner. Run{" "}
                    <code>queen-runner pair</code> on that machine and import
                    the setup code here.
                  </>
                ) : (
                  <>
                    Run <code>queen-runner pair</code> on the runner machine,
                    then import the setup code here. QueenUI pins that
                    runner&apos;s certificate and stores the credential it is
                    issued.
                  </>
                )}
              </small>
            </p>
          </div>
          <div className="form-grid runner-settings-form">
            <label>
              <span>Runner URL</span>
              <input
                value={url}
                disabled={saving}
                placeholder="https://127.0.0.1:17788"
                // Explicit, because the hint below shares this label element
                // and would otherwise become part of the field's name.
                aria-label="Runner URL"
                aria-describedby="runner-url-hint"
                onChange={(event) => {
                  setUrl(event.target.value);
                  setTestResult(null);
                }}
              />
              {/* Each line states the backend's rule rather than this
                  machine's history: the saved endpoint can also come from a
                  pre-pairing record, which is a display hint and never a
                  usable credential. */}
              <small id="runner-url-hint">
                {urlChanged
                  ? `Changing the runner requires pairing with it again. A runner credential is only ever accepted for the address it was issued for, so nothing saved for ${savedUrl} can be used at another one.`
                  : !savedUrl
                    ? "Pairing fills this in. QueenUI cannot reach a runner it has not paired with."
                    : paired
                      ? "QueenUI only talks to this exact address, and only with the credential the runner there issued during pairing."
                      : "This address was saved without a credential QueenUI can use, so pairing with the runner at it is the only way to connect."}
              </small>
            </label>
          </div>
          <div className="time-control-actions">
            <Button disabled={saving} onClick={() => setPairingOpen(true)}>
              <ShieldCheck size={14} /> Pair runner…
            </Button>
            {/* Uses the stored pairing, which the backend accepts only for the
                endpoint it was issued for — so a changed address has nothing
                to test with until it is paired. */}
            <Button
              variant="secondary"
              disabled={testing || saving || !requestedUrl || needsPairing}
              onClick={testConnection}
            >
              <Wifi size={14} /> {testing ? "Connecting…" : "Test connection"}
            </Button>
          </div>
        </>
      )}
      {settings?.allowInsecureRemoteHttp && (
        <div className="settings-note danger-note">
          <ShieldAlert size={15} />
          <p>
            <strong>Insecure runner traffic is allowed</strong>
            <small>
              This machine may send the runner credential in plaintext to a
              non-loopback address. Anyone on the network path can read it.
            </small>
          </p>
        </div>
      )}
      {testResult && (
        <div className="settings-note" role="status">
          <Wifi size={15} />
          <p>
            <strong>{testResult.hostname} is ready</strong>
            <small>
              {testResult.operatingSystem} · {testResult.architecture} ·{" "}
              {testResult.logicalCpus} logical CPUs
            </small>
          </p>
        </div>
      )}
      {/*
       * `restartRequired` is the backend's `configured != active`, and a restart
       * is no longer what resolves it — saving performs the switch. The two ways
       * to reach this state are pairing (which saves a runner without switching
       * to it) and a live switch that failed after the save, and the flag cannot
       * tell them apart; so this states what is and is not running, and names
       * saving as the remedy with a restart behind it as the fallback. The one
       * account of a *failed* attempt is the backend's own sentence, in the error
       * slot below ("…restarting QueenUI will retry it: …").
       */}
      {settings?.restartRequired && (
        <div className="settings-note warning-note">
          <Server size={15} />
          <p>
            <strong>The saved runner is not the one in use</strong>
            <small>
              QueenUI is still running games on {activeRunnerLabel(settings)};{" "}
              {runnerTargetLabel(runnerMode(settings.mode), settings.url ?? "")}{" "}
              is saved but not in use. Saving switches runners without a restart
              — restarting QueenUI only retries a switch that could not
              complete.
            </small>
          </p>
        </div>
      )}
      {loadError && (
        <p className="game-error">
          The saved runner settings could not be read ({loadError}). QueenUI is
          showing defaults.
        </p>
      )}
      {/* Both slots are written by four different async actions whose only
          other feedback is that the panel stops looking busy. */}
      {error && (
        <p className="game-error" role="alert">
          {error}
        </p>
      )}
      {/* The same announced slot the finished actions use, for the seconds the
          swap is in flight; `notice` is cleared before the save starts, so the
          two never appear together. */}
      {pendingMessage && (
        <p className="field-hint" role="status">
          {pendingMessage}
        </p>
      )}
      {notice && (
        <p className="field-hint" role="status">
          {notice}
        </p>
      )}
      <div className="time-control-actions">
        {/* Remote mode without a matching pairing record has nothing to save:
            the backend resolves the endpoint against that record and refuses
            the save if it is missing or belongs elsewhere. */}
        <Button
          disabled={saving || !settings || needsPairing}
          onClick={() => void saveSettings()}
        >
          <Server size={14} />{" "}
          {saving ? (switchExpected ? "Switching…" : "Saving…") : "Save runner"}
        </Button>
        {/*
         * The pairing record used to survive every mode switch with no way to
         * remove it, so a machine that had once talked to a runner kept the
         * credential to it indefinitely. Frozen mid-swap like the rest: the
         * identity is what an in-flight switch dials with, and an embedded
         * switch deletes this record itself once it lands.
         */}
        <Button
          variant="ghost"
          className="text-claret"
          disabled={forgetting || saving}
          onClick={() => setConfirmingForget(true)}
        >
          <KeyRound size={14} />{" "}
          {forgetting ? "Deleting…" : "Forget the paired runner"}
        </Button>
      </div>
      <RunnerPairingDialog
        open={pairingOpen}
        onClose={() => setPairingOpen(false)}
        onPaired={() => void adoptPairedRunner()}
        onFailed={() => void adoptPairingFailure()}
      />
      {/*
       * The backend's question, asked in its own words.
       *
       * The description is the refusal verbatim — it is the only thing on screen
       * that knows the runner's address and what it reported having in flight,
       * and it was written by the check that actually asked the runner. The
       * premise cannot go stale while the operator reads it, because it is not a
       * premise this panel evaluated: it is a refusal that already happened.
       * Cancelling leaves the selection alone, exactly as every other refused
       * save does, and the sentence stays in the error slot as the reason.
       *
       * Confirming answers *this* refusal, with the URL this refusal named. Read
       * from state at click time rather than closed over at save time, so if the
       * resend is refused in turn — the runner in place having changed — the
       * second confirmation carries the second sentence's URL, not the first's.
       */}
      <ConfirmDialog
        open={acknowledgement !== null}
        title={
          acknowledgement ? acknowledgementTitle(acknowledgement.kind) : ""
        }
        description={acknowledgement?.message ?? ""}
        confirmLabel={`Switch to ${runnerTargetLabel(mode, requestedUrl)}`}
        pending={saving}
        onCancel={() => setAcknowledgement(null)}
        onConfirm={() => {
          if (acknowledgement) void saveSettings(acknowledgement.runner);
        }}
      />
      {/* Says only what the command does. It deletes the records on disk; the
          connection this session already opened is not one of them. */}
      <ConfirmDialog
        open={confirmingForget}
        title="Forget the paired runner?"
        description="QueenUI deletes the runner's bearer token and pinned certificate from this computer now. A connection already running is not cut — the remote runner keeps playing until you switch runners or QueenUI restarts. Connecting again means pairing with the runner again."
        confirmLabel="Delete the credential"
        pending={forgetting}
        onCancel={() => setConfirmingForget(false)}
        onConfirm={() => void forgetCredential()}
      />
    </section>
  );
}

/**
 * A retention cap. Editing is local until blur or Enter, because committing
 * per keystroke would prune on every intermediate value: typing "20" passes
 * through 2, and a 2 GB cap deletes everything a 20 GB one would have kept.
 * Same pattern as the time-control rows above.
 */
function RetentionField({
  label,
  unit,
  value,
  min,
  max,
  step,
  format,
  disabled,
  onCommit,
}: {
  label: string;
  unit: string;
  value: number;
  min: number;
  max: number;
  step?: number;
  format: (value: number) => string;
  disabled: boolean;
  onCommit: (value: number) => void;
}) {
  const [draft, setDraft] = useState(() => format(value));
  const [committed, setCommitted] = useState(value);
  if (committed !== value) {
    setCommitted(value);
    setDraft(format(value));
  }

  function commit() {
    const parsed = Number(draft);
    if (!Number.isFinite(parsed)) {
      setDraft(format(value));
      return;
    }
    const next = Math.min(max, Math.max(min, parsed));
    setDraft(format(next));
    if (next !== value) onCommit(next);
  }

  return (
    <label>
      <span>{label}</span>
      <div className="input-wrap">
        <input
          type="number"
          min={min}
          max={max}
          step={step}
          disabled={disabled}
          value={draft}
          aria-label={`${label} ${unit}`}
          onChange={(event) => setDraft(event.target.value)}
          onBlur={commit}
          onKeyDown={(event) => {
            if (event.key === "Enter") event.currentTarget.blur();
          }}
        />
        <small>{unit}</small>
      </div>
    </label>
  );
}

/**
 * Recording policy for the engine flight recorder. Both caps apply — whichever
 * bites first removes the oldest sessions — so both are editable here.
 */
function RecordingSettings() {
  const [overview, setOverview] = useState<LogsOverview | null>(null);
  const [unavailable, setUnavailable] = useState<string | null>(null);
  const [confirmingClear, setConfirmingClear] = useState(false);
  const [clearing, setClearing] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(() => {
    commands
      .getLogsOverview()
      .then((next) => {
        setOverview(next);
        setUnavailable(null);
      })
      .catch((cause: unknown) => setUnavailable(errorText(cause)));
  }, []);

  useEffect(() => {
    refresh();
    return onLogsUpdated(refresh);
  }, [refresh]);

  function apply(patch: Partial<LogsOverview["retention"]>) {
    if (!overview) return;
    const previous = overview;
    const retention = { ...overview.retention, ...patch };
    setOverview({ ...overview, retention });
    setError(null);
    commands
      .setLogRetention(retention)
      .then(refresh)
      .catch((cause: unknown) => {
        /*
         * Roll the optimistic write back. Leaving it applied made the panel
         * read "Keep at most 0.5 GB" while the backend was still at 2 GB —
         * and since the copy says lowering a limit prunes immediately, the
         * operator had every reason to believe recordings had just been
         * deleted.
         */
        setOverview(previous);
        setError(
          `Could not save the recording limits — ${errorText(cause)}. The limits shown are the ones still in force.`,
        );
      });
  }

  /*
   * A failed read used to `return null`, deleting the whole section — which
   * reads as "this build doesn't have that feature" rather than "the call
   * failed". Same lesson the Logs page learned about its stats strip.
   */
  if (unavailable) {
    return (
      <section className="panel settings-panel">
        <div className="panel-heading">
          <div>
            <span className="eyebrow">Diagnostics</span>
            <h2>Engine recording</h2>
          </div>
          <ScrollText size={19} />
        </div>
        <p className="game-error">
          The recording service could not be reached ({unavailable}).
        </p>
        <div className="time-control-actions">
          <Button variant="secondary" onClick={refresh}>
            Try again
          </Button>
        </div>
      </section>
    );
  }

  const retention = overview?.retention;
  return (
    <section className="panel settings-panel">
      <div className="panel-heading">
        <div>
          <span className="eyebrow">Diagnostics</span>
          <h2>Engine recording</h2>
        </div>
        <ScrollText size={19} />
      </div>
      <div className="setting-toggle-row">
        <span className="settings-option-icon">
          <ScrollText size={20} />
        </span>
        <div>
          <strong>Record every engine session</strong>
          <small>
            Keeps the complete UCI conversation of each game so it can be read
            and exported from Logs.
          </small>
        </div>
        <Switch
          checked={retention?.captureEnabled ?? false}
          disabled={!retention}
          aria-label="Record every engine session"
          onCheckedChange={(checked) => apply({ captureEnabled: checked })}
        />
      </div>
      <div className="form-grid">
        <RetentionField
          label="Keep at most"
          unit="GB"
          min={0.5}
          max={200}
          step={0.5}
          disabled={!retention}
          value={(retention?.maxTotalMb ?? 0) / MB_PER_GB}
          format={(value) => value.toFixed(1)}
          onCommit={(gigabytes) =>
            apply({ maxTotalMb: Math.round(gigabytes * MB_PER_GB) })
          }
        />
        <RetentionField
          label="Delete after"
          unit="days"
          min={1}
          max={3650}
          disabled={!retention}
          value={retention?.maxAgeDays ?? 0}
          format={(value) => String(Math.round(value))}
          onCommit={(days) => apply({ maxAgeDays: Math.round(days) })}
        />
      </div>
      <div className="settings-note">
        <HardDrive size={15} />
        <p>
          <strong>
            {overview
              ? `${formatBytes(overview.compressedBytes)} on disk`
              : "Reading usage…"}
          </strong>
          <small>
            {overview
              ? `${overview.sessionCount} session${
                  overview.sessionCount === 1 ? "" : "s"
                } recorded${
                  overview.rawBytes > 0
                    ? ` · ${formatBytes(overview.rawBytes)} uncompressed`
                    : ""
                } · lowering either limit deletes the oldest recordings immediately`
              : "Compressed roughly ten times smaller than the raw output."}
          </small>
        </p>
      </div>
      {error && (
        <p className="game-error" role="alert">
          {error}
        </p>
      )}
      <div className="time-control-actions">
        <Button
          variant="secondary"
          disabled={!overview || overview.sessionCount === 0}
          onClick={() => setConfirmingClear(true)}
        >
          <Trash2 size={14} /> Delete recordings
        </Button>
      </div>
      <ConfirmDialog
        open={confirmingClear}
        title="Delete every recording?"
        description={`All ${overview?.sessionCount ?? 0} recorded engine sessions are removed from disk. This cannot be undone.`}
        confirmLabel="Delete every recording"
        pending={clearing}
        onCancel={() => setConfirmingClear(false)}
        onConfirm={() => {
          setConfirmingClear(false);
          setClearing(true);
          setError(null);
          commands
            .clearLogSessions()
            .then(refresh)
            .catch((cause: unknown) =>
              setError(
                `Could not delete the recordings — ${errorText(cause)}. Nothing was removed.`,
              ),
            )
            .finally(() => setClearing(false));
        }}
      />
    </section>
  );
}

export function SettingsPage({
  boardTheme,
  pieceSet,
  moveSoundsEnabled,
  timeControls,
  runnerSettings = null,
  runnerSettingsError = null,
  onBoardThemeChange,
  onPieceSetChange,
  onToggleMoveSounds,
  onTimeControlsChange,
  onRunnerSettingsChange,
}: {
  boardTheme: BoardThemeId;
  pieceSet: PieceSetId;
  moveSoundsEnabled: boolean;
  timeControls: TimeControl[];
  /** Owned by `App`, so the shell and this panel cannot disagree about mode. */
  runnerSettings?: RunnerSettingsView | null;
  runnerSettingsError?: string | null;
  /*
   * No live-game count. The panel used to take one — `countLiveGames` over the
   * snapshot on screen — to decide whether switching away from a remote runner
   * needed confirming. That decision is the backend's now, because the backend
   * can ask the runner and this app can only quote a snapshot that a degraded
   * link may have frozen minutes ago.
   */
  onBoardThemeChange: (theme: BoardThemeId) => void;
  onPieceSetChange: (set: PieceSetId) => void;
  onToggleMoveSounds: () => void;
  onTimeControlsChange: (controls: TimeControl[]) => void;
  onRunnerSettingsChange?: (settings: RunnerSettingsView) => void;
}) {
  const [confirmingReset, setConfirmingReset] = useState(false);
  const currentTheme =
    boardThemes.find((theme) => theme.id === boardTheme) ?? boardThemes[0];
  const currentPieces =
    pieceSets.find((set) => set.id === pieceSet) ?? pieceSets[0];

  return (
    <div className="module-content settings-page">
      <header className="module-hero settings-hero">
        <div>
          <span className="eyebrow">Application preferences</span>
          <h2>Make QueenUI feel like your board</h2>
          <p>
            Presentation preferences are saved automatically on this computer.
          </p>
        </div>
        <span className="settings-saved">
          <CircleDot size={13} /> Saved locally
        </span>
      </header>

      <div className="settings-layout">
        <div className="settings-main-column">
          <RunnerSettingsPanel
            settings={runnerSettings}
            loadError={runnerSettingsError}
            onSettingsChange={onRunnerSettingsChange ?? (() => {})}
          />
          <section className="panel settings-panel appearance-settings-panel">
            <div className="panel-heading">
              <div>
                <span className="eyebrow">Appearance</span>
                <h2>Board and pieces</h2>
              </div>
              <Palette size={19} />
            </div>
            <div className="settings-board-summary">
              {/*
               * Decorative. The `aria-label` that used to sit here was on a
               * bare div, where it is dropped anyway, and the same fact is
               * already stated as visible text in the sibling block below —
               * so hide the swatch rather than announcing four loose piece
               * names with nothing to attach them to.
               */}
              <div
                className="settings-board-preview"
                style={boardAppearanceStyle(boardTheme)}
                aria-hidden="true"
              >
                {Array.from({ length: 16 }, (_, index) => (
                  <span key={index}>
                    {index === 3 && (
                      <ChessPiece type="k" color="b" pieceSet={pieceSet} />
                    )}
                    {index === 6 && (
                      <ChessPiece type="n" color="b" pieceSet={pieceSet} />
                    )}
                    {index === 9 && (
                      <ChessPiece type="q" color="w" pieceSet={pieceSet} />
                    )}
                    {index === 12 && (
                      <ChessPiece type="k" color="w" pieceSet={pieceSet} />
                    )}
                  </span>
                ))}
              </div>
              <div>
                <span>Current presentation</span>
                <strong>
                  {currentTheme.name} · {currentPieces.name}
                </strong>
                <small>
                  This selection applies immediately to every live and completed
                  game.
                </small>
              </div>
            </div>
            <div className="settings-appearance-controls">
              <AppearanceControls
                boardTheme={boardTheme}
                pieceSet={pieceSet}
                onBoardThemeChange={onBoardThemeChange}
                onPieceSetChange={onPieceSetChange}
              />
            </div>
          </section>

          <section className="panel settings-panel audio-settings-panel">
            <div className="panel-heading">
              <div>
                <span className="eyebrow">Audio</span>
                <h2>Game sounds</h2>
              </div>
              <Volume2 size={19} />
            </div>
            <div className="setting-toggle-row">
              <span className="settings-option-icon">
                {moveSoundsEnabled ? (
                  <Volume2 size={20} />
                ) : (
                  <VolumeX size={20} />
                )}
              </span>
              <div>
                <strong>Move and capture sounds</strong>
                <small>
                  Play immediate audio feedback when a move arrives from
                  Lichess.
                </small>
              </div>
              <Switch
                checked={moveSoundsEnabled}
                aria-label="Move and capture sounds"
                onCheckedChange={onToggleMoveSounds}
              />
            </div>
            <div className="settings-note">
              <Zap size={15} />
              <p>
                <strong>Instant synchronization</strong>
                <small>
                  The sound button beside a live board controls this same
                  preference.
                </small>
              </p>
            </div>
          </section>
        </div>

        <aside className="settings-side-column">
          <RecordingSettings />
          <section className="panel settings-panel time-control-settings-panel">
            <div className="panel-heading">
              <div>
                <span className="eyebrow">Challenges</span>
                <h2>Time control presets</h2>
              </div>
              <Gauge size={19} />
            </div>
            <p className="time-control-intro">
              Edit the quick choices used for direct challenges and automatic
              matchmaking.
            </p>
            <div className="time-control-editor">
              <div className="time-control-columns" aria-hidden="true">
                <span />
                <span>Minutes</span>
                <i />
                <span>Increment</span>
                <span />
                <span />
              </div>
              {timeControls.map((control, index) => (
                <TimeControlEditorRow
                  control={control}
                  index={index}
                  canRemove={timeControls.length > 1}
                  key={index}
                  onChange={(next) =>
                    onTimeControlsChange(
                      timeControls.map((item, itemIndex) =>
                        itemIndex === index ? next : item,
                      ),
                    )
                  }
                  onRemove={() =>
                    onTimeControlsChange(
                      timeControls.filter(
                        (_, itemIndex) => itemIndex !== index,
                      ),
                    )
                  }
                />
              ))}
            </div>
            <div className="time-control-actions">
              <Button
                variant="secondary"
                disabled={timeControls.length >= 8}
                onClick={() => {
                  const candidates = [
                    ...defaultTimeControls,
                    { limitMinutes: 2, increment: 1 },
                    { limitMinutes: 30, increment: 20 },
                  ];
                  const next = candidates.find(
                    (candidate) =>
                      !timeControls.some(
                        (control) =>
                          timeControlValue(control) ===
                          timeControlValue(candidate),
                      ),
                  ) ?? { limitMinutes: 3, increment: 0 };
                  onTimeControlsChange([...timeControls, next]);
                }}
              >
                <Plus size={14} /> Add preset
              </Button>
              <button
                type="button"
                className="reset-time-controls"
                onClick={() => setConfirmingReset(true)}
              >
                Reset defaults
              </button>
            </div>
            {/* Resetting discards custom presets, so it asks like the
                other destructive actions do. */}
            <ConfirmDialog
              open={confirmingReset}
              title="Reset the time-control presets?"
              description="Your custom presets are replaced by QueenUI's defaults."
              confirmLabel="Reset to defaults"
              pending={false}
              onCancel={() => setConfirmingReset(false)}
              onConfirm={() => {
                onTimeControlsChange(
                  defaultTimeControls.map((control) => ({ ...control })),
                );
                setConfirmingReset(false);
              }}
            />
          </section>
        </aside>
      </div>
    </div>
  );
}
