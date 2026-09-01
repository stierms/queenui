import { useState } from "react";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { SettingsPage } from "./SettingsPage";
import { defaultTimeControls } from "../lib/timeControls";
import type { LogsOverview, RunnerSettingsView } from "../types";

vi.mock("../api/commands", () => ({
  getRunnerSettings: vi.fn(),
  setRunnerSettings: vi.fn(),
  testRunnerConnection: vi.fn(),
  getLogsOverview: vi.fn(),
  setLogRetention: vi.fn(),
  clearLogSessions: vi.fn(),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("../api/events", () => ({ onLogsUpdated: () => () => {} }));

const commands = await import("../api/commands");

const overview: LogsOverview = {
  sessionCount: 12,
  compressedBytes: 41_000_000,
  rawBytes: 480_000_000,
  oldestStartedAtMs: Date.now() - 86_400_000 * 30,
  liveCount: 0,
  retention: { captureEnabled: true, maxTotalMb: 40_960, maxAgeDays: 90 },
};

const embeddedSettings: RunnerSettingsView = {
  mode: "embedded",
  url: null,
  // Nothing is stored and nothing is paired: `paired` reports whether a usable
  // identity exists on disk, which is a separate fact from the saved endpoint.
  paired: false,
  activeMode: "embedded",
  source: "saved",
  restartRequired: false,
  allowInsecureRemoteHttp: false,
};

/**
 * `App` owns the runner settings now, so the harness plays that role: it holds
 * the value and applies whatever the panel reports back.
 */
function Harness({
  initialSettings = embeddedSettings,
  loadError = null,
}: {
  initialSettings?: RunnerSettingsView | null;
  loadError?: string | null;
}) {
  const [settings, setSettings] = useState<RunnerSettingsView | null>(
    initialSettings,
  );
  return (
    <SettingsPage
      boardTheme="forest"
      pieceSet="regal"
      moveSoundsEnabled={false}
      timeControls={defaultTimeControls}
      runnerSettings={settings}
      runnerSettingsError={loadError}
      onBoardThemeChange={() => {}}
      onPieceSetChange={() => {}}
      onToggleMoveSounds={() => {}}
      onTimeControlsChange={() => {}}
      onRunnerSettingsChange={setSettings}
    />
  );
}

function renderPage(props: Parameters<typeof Harness>[0] = {}) {
  return render(<Harness {...props} />);
}

beforeEach(() => {
  vi.mocked(commands.getRunnerSettings).mockResolvedValue(embeddedSettings);
  // The panel re-reads the policy after saving it, so the fake backend has to
  // remember what it was told — otherwise the field would appear to revert.
  let stored = overview;
  vi.mocked(commands.getLogsOverview).mockImplementation(() =>
    Promise.resolve(stored),
  );
  vi.mocked(commands.setLogRetention).mockImplementation((retention) => {
    stored = { ...stored, retention };
    return Promise.resolve();
  });
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("recording retention settings", () => {
  it("shows the current policy and usage", async () => {
    renderPage();
    const size = await screen.findByLabelText("Keep at most GB");
    expect(size).toHaveValue(40);
    expect(screen.getByLabelText("Delete after days")).toHaveValue(90);
    expect(screen.getByText(/12 sessions recorded/)).toBeInTheDocument();
  });

  it("never applies an intermediate cap while the size is being typed", async () => {
    const user = userEvent.setup();
    renderPage();
    const size = await screen.findByLabelText("Keep at most GB");

    await user.clear(size);
    await user.type(size, "20");
    // "2" on the way to "20" would have pruned everything above 2 GB.
    expect(commands.setLogRetention).not.toHaveBeenCalled();

    await user.tab();
    await waitFor(() =>
      expect(commands.setLogRetention).toHaveBeenCalledTimes(1),
    );
    expect(commands.setLogRetention).toHaveBeenCalledWith(
      expect.objectContaining({ maxTotalMb: 20 * 1024 }),
    );
  });

  it("commits the age cap on Enter and clamps out-of-range values", async () => {
    const user = userEvent.setup();
    renderPage();
    const age = await screen.findByLabelText("Delete after days");

    await user.clear(age);
    await user.type(age, "0{Enter}");

    await waitFor(() =>
      expect(commands.setLogRetention).toHaveBeenCalledTimes(1),
    );
    expect(commands.setLogRetention).toHaveBeenCalledWith(
      expect.objectContaining({ maxAgeDays: 1 }),
    );
    expect(age).toHaveValue(1);
  });

  it("toggles capture immediately, since disabling deletes nothing", async () => {
    const user = userEvent.setup();
    renderPage();
    const toggle = await screen.findByLabelText("Record every engine session");

    await user.click(toggle);

    await waitFor(() =>
      expect(commands.setLogRetention).toHaveBeenCalledWith(
        expect.objectContaining({ captureEnabled: false }),
      ),
    );
  });

  it("keeps the panel and names the failure when the backend is unreachable", async () => {
    // Deleting the section entirely reads as "this build has no recording
    // feature" rather than "the call failed".
    vi.mocked(commands.getLogsOverview).mockRejectedValue(new Error("no ipc"));
    renderPage();

    expect(await screen.findByText("Engine recording")).toBeInTheDocument();
    expect(
      screen.getByText(/recording service could not be reached \(no ipc\)/),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Try again" }),
    ).toBeInTheDocument();
  });

  it("rolls the retention field back when the backend rejects the change", async () => {
    const user = userEvent.setup();
    vi.mocked(commands.setLogRetention).mockRejectedValue(
      new Error("disk is read-only"),
    );
    renderPage();
    const size = await screen.findByLabelText("Keep at most GB");

    await user.clear(size);
    await user.type(size, "2{Enter}");

    expect(await screen.findByText(/disk is read-only/)).toBeInTheDocument();
    // The panel must not keep showing a cap the backend never accepted: the
    // copy promises that lowering it prunes immediately.
    await waitFor(() => expect(size).toHaveValue(40));
  });

  it("requires confirmation before deleting every recording", async () => {
    const user = userEvent.setup();
    vi.mocked(commands.clearLogSessions).mockResolvedValue(12);
    renderPage();

    await user.click(
      await screen.findByRole("button", { name: /Delete recordings/ }),
    );
    expect(commands.clearLogSessions).not.toHaveBeenCalled();

    await user.click(
      screen.getByRole("button", { name: /Delete every recording/ }),
    );
    await waitFor(() =>
      expect(commands.clearLogSessions).toHaveBeenCalledTimes(1),
    );
  });
});

describe("runner settings", () => {
  const pairedSettings: RunnerSettingsView = {
    mode: "remote",
    url: "https://runner-host:17789",
    paired: true,
    activeMode: "remote",
    source: "saved",
    restartRequired: false,
    allowInsecureRemoteHttp: false,
  };

  /*
   * What the backend reports for a record that carries an endpoint but no usable
   * identity: a pre-pairing config, or the same config once the credential has
   * been forgotten. `settings_view` returns `url: Some(_)` with `paired: false`
   * for exactly this case (see `legacy_view` in the Rust tests).
   */
  const unpairedUrlSettings: RunnerSettingsView = {
    ...pairedSettings,
    paired: false,
    source: "settings",
  };

  /*
   * A runner that is paired and saved but not running games — what
   * `pair_and_store` leaves behind whenever the runner it paired with is not
   * already the active one: it writes the config and stops. (When it *is* the
   * active one — same canonical endpoint, or an active remote whose backend slot
   * is unavailable — pairing publishes a new backend on the rotated credential
   * and adopts it live, and `restartRequired` comes back false instead.)
   * `restartRequired` is the backend's `configured != active`, which is how the
   * panel tells those two apart without assuming either.
   */
  const pairedNotSwitchedSettings: RunnerSettingsView = {
    ...pairedSettings,
    activeMode: "embedded",
    restartRequired: true,
  };

  /** A promise the test resolves by hand, to hold the save mid-flight. */
  function deferred<T>() {
    let settle: (value: T) => void = () => {};
    let fail: (cause: unknown) => void = () => {};
    const promise = new Promise<T>((resolve, reject) => {
      settle = resolve;
      fail = reject;
    });
    return { promise, settle, fail };
  }

  it("offers pairing as the only way to authorize a runner", async () => {
    const user = userEvent.setup();
    renderPage();

    await user.selectOptions(
      await screen.findByLabelText("Execution target"),
      "remote",
    );

    expect(
      screen.getByRole("button", { name: /Pair runner/ }),
    ).toBeInTheDocument();
    /*
     * The bearer and the certificate pin only exist as a pairing record, and
     * `set_runner_settings` / `test_runner_connection` reject a non-empty token
     * outright — so a field for one could only ever collect a rejection.
     */
    expect(screen.queryByLabelText("Bearer token")).not.toBeInTheDocument();
    expect(
      screen.queryByLabelText("Insecure: plaintext to a non-loopback host"),
    ).not.toBeInTheDocument();
    // Nothing is hidden behind a disclosure any more; there is one path.
    expect(
      screen.queryByRole("button", { name: /manual setup/i }),
    ).not.toBeInTheDocument();
  });

  it("tests and saves the paired runner without submitting a credential", async () => {
    const user = userEvent.setup();
    vi.mocked(commands.testRunnerConnection).mockResolvedValue({
      hostname: "runner-host",
      operatingSystem: "linux",
      architecture: "x86_64",
      logicalCpus: 32,
    });
    // A completed live switch: the backend publishes the active runner from the
    // config it just saved, so `restartRequired` comes back false.
    vi.mocked(commands.setRunnerSettings).mockResolvedValue(pairedSettings);
    renderPage({ initialSettings: pairedNotSwitchedSettings });

    await user.click(
      await screen.findByRole("button", { name: "Test connection" }),
    );
    expect(await screen.findByText("runner-host is ready")).toBeInTheDocument();
    // The endpoint and nothing else: the stored pairing supplies the rest.
    expect(commands.testRunnerConnection).toHaveBeenCalledExactlyOnceWith(
      "https://runner-host:17789",
    );

    await user.click(screen.getByRole("button", { name: "Save runner" }));
    expect(
      await screen.findByText(
        "Switched to the runner at https://runner-host:17789.",
      ),
    ).toBeInTheDocument();
    // No acknowledgement: nothing has been refused, so there is nothing to
    // acknowledge — the flag is only ever an answer.
    expect(commands.setRunnerSettings).toHaveBeenCalledExactlyOnceWith(
      "remote",
      "https://runner-host:17789",
      undefined,
    );
  });

  it("routes a changed runner URL to pairing and offers no way to submit a bearer", async () => {
    const user = userEvent.setup();
    renderPage({ initialSettings: pairedSettings });

    const url = await screen.findByLabelText("Runner URL");
    await user.clear(url);
    await user.type(url, "https://newrig:17789");

    // The credential was issued by the runner at the old address and the
    // backend refuses it anywhere else, so the only honest route is pairing.
    expect(
      screen.getByText(/Changing the runner requires pairing with it again/),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /Pair runner/ }),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save runner" })).toBeDisabled();
    // Testing would resolve the new address against the old pairing record too.
    expect(
      screen.getByRole("button", { name: "Test connection" }),
    ).toBeDisabled();
    // No field, of any name, can carry a bearer to the new endpoint.
    expect(screen.queryByLabelText("Bearer token")).not.toBeInTheDocument();
    expect(document.querySelectorAll("input[type=password]")).toHaveLength(0);
    expect(commands.setRunnerSettings).not.toHaveBeenCalled();
    expect(commands.testRunnerConnection).not.toHaveBeenCalled();
  });

  it("does not demand pairing again for a differently-spelled same endpoint", async () => {
    /*
     * The backend canonicalizes the endpoint it stores and dials, so a trailing
     * slash, the elided default port and a differently-cased host are all the
     * *same* runner. Comparing the raw strings called each of them a different
     * one and pushed an already-paired operator back through pairing.
     */
    const user = userEvent.setup();
    renderPage({ initialSettings: pairedSettings });

    const url = await screen.findByLabelText("Runner URL");
    for (const spelling of [
      "https://runner-host:17789/",
      "HTTPS://Runner-Host:17789",
      "  https://runner-host:17789  ",
    ]) {
      await user.clear(url);
      await user.type(url, spelling);

      expect(
        screen.queryByText(
          /Changing the runner requires pairing with it again/,
        ),
      ).toBeNull();
      expect(screen.getByRole("button", { name: "Save runner" })).toBeEnabled();
      expect(
        screen.getByRole("button", { name: "Test connection" }),
      ).toBeEnabled();
    }
  });

  it("still routes a genuinely different endpoint to pairing", async () => {
    // The guard rail on the normalization above: only the *spelling* is
    // forgiven. A different host, port or base path is a different runner, and
    // the pairing record the backend holds does not authorize it.
    const user = userEvent.setup();
    renderPage({ initialSettings: pairedSettings });

    const url = await screen.findByLabelText("Runner URL");
    for (const different of [
      "https://runner-host:17790",
      "https://runner-host:17789/base",
      "http://runner-host:17789",
    ]) {
      await user.clear(url);
      await user.type(url, different);

      expect(
        screen.getByText(/Changing the runner requires pairing with it again/),
      ).toBeInTheDocument();
      expect(
        screen.getByRole("button", { name: "Save runner" }),
      ).toBeDisabled();
    }
  });

  it("shows a saved endpoint that has no credential as unpaired, and refuses to use it", async () => {
    /*
     * The state this whole field exists for: `url` alone used to mean "paired",
     * so a leftover record enabled Test and Save for a runner the backend has no
     * identity for — two buttons guaranteed to fail.
     */
    renderPage({ initialSettings: unpairedUrlSettings });

    expect(
      await screen.findByText("Not paired with https://runner-host:17789"),
    ).toBeInTheDocument();
    // The address is still shown — it is a real hint about where the runner was.
    expect(screen.getByLabelText("Runner URL")).toHaveValue(
      "https://runner-host:17789",
    );
    expect(
      screen.getByText(
        /pinned certificate and bearer token that authorize it are not on this computer/,
      ),
    ).toBeInTheDocument();
    // Never "Runner endpoint …", which reads as a working connection.
    expect(
      screen.queryByText(/^Runner endpoint https:\/\/runner-host:17789$/),
    ).toBeNull();

    expect(
      screen.getByRole("button", { name: /Pair runner/ }),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save runner" })).toBeDisabled();
    expect(
      screen.getByRole("button", { name: "Test connection" }),
    ).toBeDisabled();
    expect(commands.setRunnerSettings).not.toHaveBeenCalled();
    expect(commands.testRunnerConnection).not.toHaveBeenCalled();
  });

  it("will not save remote mode before a runner is paired", async () => {
    const user = userEvent.setup();
    renderPage();

    await user.selectOptions(
      await screen.findByLabelText("Execution target"),
      "remote",
    );

    expect(screen.getByText("No runner paired yet")).toBeInTheDocument();
    // Saving would only reach the backend's "no runner identity" refusal.
    expect(screen.getByRole("button", { name: "Save runner" })).toBeDisabled();
    expect(commands.setRunnerSettings).not.toHaveBeenCalled();
  });

  it("says that switching to this computer deletes the pairing record", async () => {
    const user = userEvent.setup();
    renderPage({ initialSettings: pairedSettings });

    await user.selectOptions(
      await screen.findByLabelText("Execution target"),
      "embedded",
    );

    expect(
      screen.getByText(
        /Saving this also deletes the paired runner's bearer token and pinned certificate/,
      ),
    ).toBeInTheDocument();
  });

  it("says so loudly when the backend reports insecure http is allowed", async () => {
    renderPage({
      initialSettings: {
        mode: "remote",
        url: "http://192.168.0.36:17788",
        paired: true,
        activeMode: "remote",
        source: "saved",
        restartRequired: false,
        allowInsecureRemoteHttp: true,
      },
    });

    expect(
      await screen.findByText("Insecure runner traffic is allowed"),
    ).toBeInTheDocument();
  });

  it("names the runner it is switching to, and freezes the form, while the swap runs", async () => {
    /*
     * Saving is a drain-and-swap now: it can take as long as the outgoing
     * backend needs to stop. "Saving…" alone described a file write and made a
     * multi-second runner switch look like a wedged button.
     */
    const user = userEvent.setup();
    const save = deferred<RunnerSettingsView>();
    vi.mocked(commands.setRunnerSettings).mockReturnValue(save.promise);
    renderPage({ initialSettings: pairedNotSwitchedSettings });

    await user.click(
      await screen.findByRole("button", { name: "Save runner" }),
    );

    const switching = await screen.findByText(
      "Switching to the runner at https://runner-host:17789…",
    );
    expect(switching).toHaveAttribute("role", "status");
    /*
     * Everything that could reach the backend mid-swap — where it would only
     * collect "QueenUI is switching runners; retry in a moment" — or edit what
     * the in-flight save is about to publish.
     */
    expect(screen.getByLabelText("Execution target")).toBeDisabled();
    expect(screen.getByLabelText("Runner URL")).toBeDisabled();
    expect(screen.getByRole("button", { name: /Pair runner/ })).toBeDisabled();
    expect(
      screen.getByRole("button", { name: "Test connection" }),
    ).toBeDisabled();
    expect(screen.getByRole("button", { name: "Switching…" })).toBeDisabled();
    expect(
      screen.getByRole("button", { name: /Forget the paired runner/ }),
    ).toBeDisabled();

    save.settle(pairedSettings);

    expect(
      await screen.findByText(
        "Switched to the runner at https://runner-host:17789.",
      ),
    ).toBeInTheDocument();
    expect(screen.queryByText(/^Switching to the runner at/)).toBeNull();
    expect(screen.getByLabelText("Execution target")).toBeEnabled();
  });

  /*
   * The acknowledgement flow, which replaced a confirmation this panel used to
   * raise on its own.
   *
   * Wave 2 asked before the save, from the live-game count `App` handed it: if a
   * remote runner appeared to be playing, it raised a dialog naming that count
   * and only then called the backend. Two things were wrong. The count was this
   * app's copy of the runner's state — the one thing a degraded link makes stale
   * — so it could ask about games that had already finished, or say nothing
   * about games it had never been told about. And it only knew one direction:
   * remote to *embedded*. A switch from one remote runner to another went
   * straight through with no question asked at all, abandoning the first
   * runner's boards silently, which is the case these tests now cover.
   *
   * `verify_remote_handover` asks the runner itself and refuses the save until
   * the answer is acknowledged, so the question is the backend's now, in the
   * backend's words, and `acknowledgedRunner` — the URL that sentence names — is
   * the answer.
   *
   * Wave 4 made the acknowledgement a URL rather than a flag, and split the
   * disclosure four ways, because the runner reports live games and outgoing
   * challenges separately (`handover_inventory`). A challenge is not a game yet,
   * so "still playing" would be false of it, but it becomes that runner's game
   * when it is accepted — the same work being left behind, in its own sentence.
   */
  const RUNNER = "https://runner-host:17789";

  const stillPlaying = (count: number, runner = RUNNER) =>
    count === 1
      ? `The remote runner at ${runner} is still playing 1 game. Confirm that it will keep playing there before switching runners.`
      : `The remote runner at ${runner} is still playing ${count} games. Confirm that they will keep playing there before switching runners.`;

  const owesChallenges = (count: number, runner = RUNNER) =>
    count === 1
      ? `The remote runner at ${runner} still owns 1 outgoing challenge. Confirm that it will remain there before switching runners.`
      : `The remote runner at ${runner} still owns ${count} outgoing challenges. Confirm that they will remain there before switching runners.`;

  const playingAndOwesChallenges = (
    games: number,
    challenges: number,
    runner = RUNNER,
  ) =>
    `The remote runner at ${runner} is still playing ${games} ${
      games === 1 ? "game" : "games"
    } and owns ${challenges} outgoing ${
      challenges === 1 ? "challenge" : "challenges"
    }. Confirm that this work will remain there before switching runners.`;

  const couldNotVerify = (runner = RUNNER) =>
    `Could not verify the remote runner at ${runner}; it may still be playing games. Confirm that its games will keep running there before switching runners.`;

  // Each refusal is a complete render and confirmation flow. Keep them as
  // independently isolated tests so a slower platform does not have to run
  // seven UI scenarios inside one test's timeout budget.
  const handoverRefusals = [
    { name: "one live game", refusal: stillPlaying(1) },
    { name: "multiple live games", refusal: stillPlaying(3) },
    { name: "one outgoing challenge", refusal: owesChallenges(1) },
    { name: "multiple outgoing challenges", refusal: owesChallenges(2) },
    {
      name: "one live game and challenge",
      refusal: playingAndOwesChallenges(1, 1),
    },
    {
      name: "multiple live games and challenges",
      refusal: playingAndOwesChallenges(3, 2),
    },
    { name: "unreachable runner", refusal: couldNotVerify() },
  ] as const;

  it.each(handoverRefusals)(
    "asks with the backend's own sentence, then resends it naming that runner: $name",
    async ({ refusal }) => {
      const user = userEvent.setup();
      vi.mocked(commands.setRunnerSettings)
        .mockRejectedValueOnce(refusal)
        .mockResolvedValueOnce(embeddedSettings);
      vi.mocked(commands.getRunnerSettings).mockResolvedValue(pairedSettings);
      renderPage({ initialSettings: pairedSettings });

      await user.selectOptions(
        await screen.findByLabelText("Execution target"),
        "embedded",
      );
      await user.click(screen.getByRole("button", { name: "Save runner" }));

      const dialog = await screen.findByRole("dialog");
      // Verbatim, counts and all: the check that produced it is the only thing
      // that knows what the runner reported having in flight.
      expect(dialog).toHaveTextContent(refusal);
      // Never the refusal that cannot be acknowledged — nothing has to end.
      expect(dialog).not.toHaveTextContent(/finish or resign/);

      await user.click(
        screen.getByRole("button", {
          name: "Switch to this computer's engine",
        }),
      );

      /*
       * The runner being *left*, not the target being saved: the acknowledgement
       * clears the refusal only when it equals the live remote's own canonical
       * base URL, and the sentence is the only place on screen that URL appears.
       * The save itself is going to `undefined` — this computer.
       */
      await waitFor(() =>
        expect(commands.setRunnerSettings).toHaveBeenNthCalledWith(
          2,
          "embedded",
          undefined,
          RUNNER,
        ),
      );
      // The first attempt carried no acknowledgement, which is what got it
      // refused; the confirmation is the only thing that adds one.
      expect(commands.setRunnerSettings).toHaveBeenNthCalledWith(
        1,
        "embedded",
        undefined,
        undefined,
      );
      expect(
        await screen.findByText("Switched to this computer's engine."),
      ).toBeInTheDocument();
      expect(screen.queryByRole("dialog")).toBeNull();
    },
  );

  it("names the question the refusal asked, and never the wrong one", async () => {
    /*
     * The description is the backend's sentence; the *title* is the panel's own
     * question, so it has to be the question this refusal asked. Titling the
     * challenge-only refusal "still playing" would claim games that the runner
     * explicitly reported it does not have, on the screen that decides which
     * machine plays them.
     */
    const user = userEvent.setup();
    for (const [refusal, title, wrong] of [
      [
        stillPlaying(2),
        "Switch runners while the remote runner is still playing?",
        /challenges out/,
      ],
      [
        owesChallenges(2),
        "Switch runners while the remote runner still has challenges out?",
        /is still playing\?/,
      ],
      [
        playingAndOwesChallenges(2, 1),
        "Switch runners while the remote runner is still playing and has challenges out?",
        /without reaching/,
      ],
      [
        couldNotVerify(),
        "Switch runners without reaching the remote runner?",
        /is still playing/,
      ],
    ] as const) {
      vi.mocked(commands.setRunnerSettings).mockReset();
      vi.mocked(commands.setRunnerSettings).mockRejectedValue(refusal);
      vi.mocked(commands.getRunnerSettings).mockResolvedValue(pairedSettings);
      renderPage({ initialSettings: pairedSettings });

      await user.selectOptions(
        await screen.findByLabelText("Execution target"),
        "embedded",
      );
      await user.click(screen.getByRole("button", { name: "Save runner" }));

      const dialog = await screen.findByRole("dialog");
      expect(dialog).toHaveTextContent(title);
      expect(dialog).not.toHaveTextContent(wrong);
      cleanup();
    }
  });

  it("answers a re-refused acknowledgement with the runner the new sentence names", async () => {
    /*
     * What binding the acknowledgement to a URL is *for*.
     *
     * A boolean acknowledged "the switch", so it outlived the runner it was about:
     * the operator confirmed runner A's games, the runner in place changed before
     * the resend landed — a re-pairing that rotated the bearer, a recovery from
     * the unavailable slot, any republished backend — and the same `true` waved
     * the new runner's games through unread. The URL cannot do that. An
     * acknowledgement of A is not one of C, so the resend is refused again,
     * naming C.
     *
     * Which makes this the loop that must render: the panel used to suppress the
     * question for any save that already carried an acknowledgement, so this
     * refusal would have landed in the error slot with no way to answer it. It
     * asks again, with C's sentence, and confirming sends C's URL — not the A it
     * was holding a moment ago.
     */
    const user = userEvent.setup();
    const runnerA = "https://runner-a:17789";
    const runnerC = "https://runner-c:17789";
    // Reset rather than clear: `afterEach` clears recorded calls but leaves a
    // queued `…Once` value in place, and this test queues three.
    vi.mocked(commands.setRunnerSettings).mockReset();
    vi.mocked(commands.setRunnerSettings)
      .mockRejectedValueOnce(stillPlaying(2, runnerA))
      .mockRejectedValueOnce(couldNotVerify(runnerC))
      .mockResolvedValueOnce(embeddedSettings);
    vi.mocked(commands.getRunnerSettings).mockResolvedValue(pairedSettings);
    renderPage({ initialSettings: pairedSettings });

    await user.selectOptions(
      await screen.findByLabelText("Execution target"),
      "embedded",
    );
    await user.click(screen.getByRole("button", { name: "Save runner" }));

    expect(await screen.findByRole("dialog")).toHaveTextContent(
      stillPlaying(2, runnerA),
    );
    await user.click(
      screen.getByRole("button", { name: "Switch to this computer's engine" }),
    );

    /*
     * The second refusal is a different sentence about a different runner, and it
     * is the one on screen now — the first is not still being answered. Re-queried
     * each poll rather than held: confirming closes the dialog for the duration of
     * the resend, so the node that comes back is a new one.
     */
    const reasked = await waitFor(() => {
      const dialog = screen.getByRole("dialog");
      expect(dialog).toHaveTextContent(couldNotVerify(runnerC));
      return dialog;
    });
    expect(reasked).not.toHaveTextContent(stillPlaying(2, runnerA));
    expect(reasked).toHaveTextContent(
      "Switch runners without reaching the remote runner?",
    );

    await user.click(
      screen.getByRole("button", { name: "Switch to this computer's engine" }),
    );

    await waitFor(() =>
      expect(commands.setRunnerSettings).toHaveBeenNthCalledWith(
        3,
        "embedded",
        undefined,
        runnerC,
      ),
    );
    // And the first confirmation carried A, from the refusal that named A.
    expect(commands.setRunnerSettings).toHaveBeenNthCalledWith(
      2,
      "embedded",
      undefined,
      runnerA,
    );
    expect(
      await screen.findByText("Switched to this computer's engine."),
    ).toBeInTheDocument();
  });

  it("asks the same way when the switch is to another remote runner", async () => {
    /*
     * The case the old gate missed entirely. Both runners are remote, so nothing
     * about the *mode* changes — and the first runner's games are exactly as
     * abandoned as they would be by a switch to this computer.
     */
    const user = userEvent.setup();
    const otherRunner: RunnerSettingsView = {
      ...pairedSettings,
      url: "https://other-runner:17789",
    };
    vi.mocked(commands.setRunnerSettings)
      .mockRejectedValueOnce(stillPlaying(2))
      .mockResolvedValueOnce(otherRunner);
    vi.mocked(commands.getRunnerSettings).mockResolvedValue(otherRunner);
    renderPage({ initialSettings: otherRunner });

    await user.click(
      await screen.findByRole("button", { name: "Save runner" }),
    );

    expect(await screen.findByRole("dialog")).toHaveTextContent(
      stillPlaying(2),
    );
    await user.click(
      screen.getByRole("button", {
        name: "Switch to the runner at https://other-runner:17789",
      }),
    );

    /*
     * Two URLs, and they are not the same one: the save is going to
     * `https://other-runner:17789`, while the acknowledgement names the runner
     * being left — which is the only runner `verify_remote_handover` will accept
     * an acknowledgement for. Sending the target here would acknowledge nothing.
     */
    await waitFor(() =>
      expect(commands.setRunnerSettings).toHaveBeenNthCalledWith(
        2,
        "remote",
        "https://other-runner:17789",
        RUNNER,
      ),
    );
  });

  it("keeps the refused selection, and the refusal, when the question is cancelled", async () => {
    const user = userEvent.setup();
    vi.mocked(commands.setRunnerSettings).mockRejectedValue(stillPlaying(2));
    vi.mocked(commands.getRunnerSettings).mockResolvedValue(pairedSettings);
    renderPage({ initialSettings: pairedSettings });

    await user.selectOptions(
      await screen.findByLabelText("Execution target"),
      "embedded",
    );
    await user.click(screen.getByRole("button", { name: "Save runner" }));
    await user.click(await screen.findByRole("button", { name: "Cancel" }));

    // Cancel is not a reset: the operator's choice survives being questioned,
    // as it does when the backend refuses a save outright.
    expect(screen.getByLabelText("Execution target")).toHaveValue("embedded");
    expect(screen.queryByRole("dialog")).toBeNull();
    // Declining is a decision not to switch, and the reason it did not happen is
    // the backend's sentence — which is still the only account of it on screen.
    expect((await screen.findByRole("alert")).textContent).toBe(
      stillPlaying(2),
    );
    expect(commands.setRunnerSettings).toHaveBeenCalledExactlyOnceWith(
      "embedded",
      undefined,
      undefined,
    );
    // And nothing announces a switch that was refused.
    expect(screen.queryByText(/^Switched to/)).toBeNull();
    expect(screen.queryByText(/^Switching to/)).toBeNull();
  });

  it("asks nothing of its own when the backend does not refuse", async () => {
    /*
     * There is no frontend gate left, in either direction: a save the backend
     * accepts is a save, whatever this app believes is being played and wherever
     * it believes it is being played.
     */
    const user = userEvent.setup();
    // The direction wave 2 gated, and the direction it deliberately did not.
    for (const [settings, target, url] of [
      [pairedSettings, "embedded", undefined],
      [embeddedSettings, "embedded", undefined],
    ] as const) {
      vi.mocked(commands.setRunnerSettings).mockReset();
      vi.mocked(commands.setRunnerSettings).mockResolvedValue(embeddedSettings);
      renderPage({ initialSettings: settings });

      await user.selectOptions(
        await screen.findByLabelText("Execution target"),
        target,
      );
      await user.click(screen.getByRole("button", { name: "Save runner" }));

      await waitFor(() =>
        expect(commands.setRunnerSettings).toHaveBeenCalledExactlyOnceWith(
          target,
          url,
          undefined,
        ),
      );
      expect(screen.queryByRole("dialog")).toBeNull();
      expect(screen.queryByText(/is still playing/)).toBeNull();
      cleanup();
    }
  });

  it("does not offer a confirmation for a refusal a confirmation cannot clear", async () => {
    /*
     * The pre-switch verification refusals: local live games, an unresolved
     * outgoing challenge, and the authoritative Lichess checks. Every one of them
     * names something that has to be resolved — finished, resigned, cancelled, or
     * waited out — and an acknowledgement does not clear any of it. Offering one
     * would be a button whose only effect is to collect the same refusal again.
     */
    const user = userEvent.setup();
    for (const refusal of [
      // Live games on this computer, which the switch would abandon.
      "1 game is still being played from this computer; finish or resign them before switching to a runner.",
      "3 games are still being played from this computer; finish or resign them before switching to a runner.",
      // Outgoing challenges QueenUI knows about locally, both numbers.
      "An outgoing challenge to Opponent is still unresolved; cancel it or let it resolve before switching to a runner.",
      "2 outgoing challenges are still unresolved (Opponent, Other); cancel them or let them resolve before switching to a runner.",
      "A campaign challenge is still unresolved; cancel it or let it resolve before switching to a runner.",
      // The authoritative Lichess checks, including the game ids they list.
      "Lichess account QueenBot still has 1 live game (abc123); finish or resign them before switching to a runner.",
      "Lichess account QueenBot still has 2 live games (abc123, def456); finish or resign them before switching to a runner.",
      "Lichess account QueenBot still has 1 outgoing challenge (Opponent); cancel them or let them resolve before switching to a runner.",
      "Could not verify Lichess account QueenBot before switching runners; live games or outgoing challenges may still exist.",
    ]) {
      vi.mocked(commands.setRunnerSettings).mockReset();
      vi.mocked(commands.setRunnerSettings).mockRejectedValue(refusal);
      vi.mocked(commands.getRunnerSettings).mockResolvedValue(
        pairedNotSwitchedSettings,
      );
      renderPage({ initialSettings: pairedNotSwitchedSettings });

      await user.click(
        await screen.findByRole("button", { name: "Save runner" }),
      );

      // Rendered whole, in the ordinary error slot, with no dialog over it.
      expect((await screen.findByRole("alert")).textContent).toBe(refusal);
      expect(screen.queryByRole("dialog")).toBeNull();
      expect(commands.setRunnerSettings).toHaveBeenCalledOnce();
      cleanup();
    }
  });

  it("says it is switching to this computer, then that it did", async () => {
    const user = userEvent.setup();
    const save = deferred<RunnerSettingsView>();
    vi.mocked(commands.setRunnerSettings).mockReturnValue(save.promise);
    renderPage({ initialSettings: pairedSettings });

    await user.selectOptions(
      await screen.findByLabelText("Execution target"),
      "embedded",
    );
    await user.click(screen.getByRole("button", { name: "Save runner" }));

    expect(
      await screen.findByText("Switching to this computer's engine…"),
    ).toBeInTheDocument();
    expect(screen.getByLabelText("Execution target")).toBeDisabled();

    save.settle(embeddedSettings);

    expect(
      await screen.findByText("Switched to this computer's engine."),
    ).toBeInTheDocument();
  });

  it("does not announce a switch when the saved runner is the one already running", async () => {
    /*
     * `set_runner_settings_inner` only swaps when the requested target differs
     * from the active one, so this save really is nothing but a write — and
     * "Switched to…" would be a switch that never happened.
     */
    const user = userEvent.setup();
    const save = deferred<RunnerSettingsView>();
    vi.mocked(commands.setRunnerSettings).mockReturnValue(save.promise);
    renderPage();

    await user.click(
      await screen.findByRole("button", { name: /Save runner/ }),
    );

    expect(
      await screen.findByText("Saving the runner settings…"),
    ).toBeInTheDocument();
    expect(screen.queryByText(/^Switching to/)).toBeNull();

    save.settle(embeddedSettings);

    expect(
      await screen.findByText(
        "Runner settings saved. QueenUI is already using this computer's engine.",
      ),
    ).toBeInTheDocument();
    // The old receipt, which described a write and said nothing about runners.
    expect(screen.queryByText("Runner saved.")).toBeNull();
    expect(screen.queryByText(/^Switched to/)).toBeNull();
  });

  it("renders the live-games refusal exactly as the backend wrote it, count and all", async () => {
    // The refusal happens before anything is saved, and it is the one sentence
    // that tells the operator what to do about it. No wrapper may reword it.
    const user = userEvent.setup();
    for (const refusal of [
      "1 game is still being played from this computer; finish or resign them before switching to a runner.",
      "3 games are still being played from this computer; finish or resign them before switching to a runner.",
    ]) {
      vi.mocked(commands.setRunnerSettings).mockRejectedValue(refusal);
      vi.mocked(commands.getRunnerSettings).mockResolvedValue(
        pairedNotSwitchedSettings,
      );
      renderPage({ initialSettings: pairedNotSwitchedSettings });

      await user.click(
        await screen.findByRole("button", { name: "Save runner" }),
      );

      const alert = await screen.findByRole("alert");
      expect(alert.textContent).toBe(refusal);
      cleanup();
    }
  });

  it("keeps the backend's account of a switch that failed after the save, and offers the restart as the fallback", async () => {
    /*
     * The state this whole rework exists for: the config is on disk, the old
     * backend is still running, and the backend says so itself. The blanket
     * wrapper — "The previously saved runner is still in use." — denied the
     * sentence it was appended to, and "Could not save the runner settings"
     * denies the half that did land.
     */
    const user = userEvent.setup();
    const savedButFailed =
      "Runner settings were saved, but the switch could not complete; restarting QueenUI will retry it: QueenUI automation is already owned";
    vi.mocked(commands.setRunnerSettings).mockRejectedValue(savedButFailed);
    // The fallback state: embedded configured, the remote backend restored.
    vi.mocked(commands.getRunnerSettings).mockResolvedValue({
      mode: "embedded",
      url: null,
      paired: true,
      activeMode: "remote",
      source: "saved",
      restartRequired: true,
      allowInsecureRemoteHttp: false,
    });
    renderPage({ initialSettings: pairedSettings });

    await user.selectOptions(
      await screen.findByLabelText("Execution target"),
      "embedded",
    );
    await user.click(screen.getByRole("button", { name: /Save runner/ }));

    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toBe(savedButFailed);
    expect(alert).not.toHaveTextContent(
      "previously saved runner is still in use",
    );
    expect(alert).not.toHaveTextContent("Could not save the runner settings");
    expect(screen.queryByText(/^Switched to/)).toBeNull();
    expect(screen.queryByText("Runner saved.")).toBeNull();

    /*
     * The re-read is what makes the banner truthful: a failed switch changes
     * `restartRequired`, and the panel's own copy of the settings cannot know.
     */
    expect(
      await screen.findByText("The saved runner is not the one in use"),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        /QueenUI is still running games on the remote runner; this computer's engine is saved but not in use/,
      ),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        /restarting QueenUI only retries a switch that could not complete/,
      ),
    ).toBeInTheDocument();
    expect(commands.getRunnerSettings).toHaveBeenCalledTimes(1);
    // A restart is the fallback now, never the way to switch runners.
    expect(screen.queryByText("Restart QueenUI to switch runners")).toBeNull();
  });

  it("renders the mid-swap refusal verbatim and keeps the selection it refused", async () => {
    const user = userEvent.setup();
    const busy = "QueenUI is switching runners; retry in a moment";
    vi.mocked(commands.setRunnerSettings).mockRejectedValue(busy);
    // Nothing was written: the change gate is taken before anything else.
    vi.mocked(commands.getRunnerSettings).mockResolvedValue(pairedSettings);
    renderPage({ initialSettings: pairedSettings });

    await user.selectOptions(
      await screen.findByLabelText("Execution target"),
      "embedded",
    );
    await user.click(screen.getByRole("button", { name: /Save runner/ }));

    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toBe(busy);
    /*
     * The re-read reports the same saved runner, and adopting an unchanged
     * target must not throw away the selection the operator is being told to
     * retry with.
     */
    await waitFor(() =>
      expect(commands.getRunnerSettings).toHaveBeenCalledTimes(1),
    );
    expect(screen.getByLabelText("Execution target")).toHaveValue("embedded");
    // Nothing was saved, so nothing may claim the saved runner changed.
    expect(
      screen.queryByText("The saved runner is not the one in use"),
    ).toBeNull();
  });

  it("renders the interrupted-switch error verbatim, and agrees that saving is the recovery", async () => {
    /*
     * The state an abandoned switch leaves behind: the backend slot holds no
     * runner at all, so every command fails with this one sentence — and the
     * sentence names its own remedy, which is this panel. Anything appended
     * that offered a different one (a restart, "nothing was saved") would send
     * the operator away from the only control that can fix it.
     */
    const user = userEvent.setup();
    const interrupted =
      "The runner switch was interrupted; save runner settings again to recover the backend";
    vi.mocked(commands.setRunnerSettings).mockRejectedValue(interrupted);
    vi.mocked(commands.getRunnerSettings).mockResolvedValue({
      ...embeddedSettings,
      paired: true,
      activeMode: "remote",
      restartRequired: true,
    });
    renderPage({ initialSettings: pairedSettings });

    await user.selectOptions(
      await screen.findByLabelText("Execution target"),
      "embedded",
    );
    await user.click(screen.getByRole("button", { name: /Save runner/ }));

    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toBe(interrupted);
    expect(alert).not.toHaveTextContent("Could not save the runner settings");
    expect(alert).not.toHaveTextContent(
      "previously saved runner is still in use",
    );
    expect(screen.queryByText(/^Switched to/)).toBeNull();
    // The panel's own banner points at the same remedy the sentence names.
    expect(
      await screen.findByText("The saved runner is not the one in use"),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/Saving switches runners without a restart/),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save runner" })).toBeEnabled();
  });

  it("does not contradict the backend when the embedded switch landed but the record stayed", async () => {
    /*
     * The backend switches to embedded first and deletes the pairing record
     * second, so this failure means the switch *did* happen — games are already
     * running on this computer, and nothing here may ask for a restart.
     */
    const user = userEvent.setup();
    const halfDone =
      "Runner mode is embedded, but the stored pairing record could not be removed: keyring is locked. Use ‘Forget the paired runner’ to retry the deletion.";
    vi.mocked(commands.setRunnerSettings).mockRejectedValue(halfDone);
    vi.mocked(commands.getRunnerSettings).mockResolvedValue({
      ...embeddedSettings,
      paired: true,
    });
    renderPage({ initialSettings: pairedSettings });

    await user.selectOptions(
      await screen.findByLabelText("Execution target"),
      "embedded",
    );
    await user.click(screen.getByRole("button", { name: /Save runner/ }));

    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toBe(halfDone);
    expect(alert).not.toHaveTextContent(
      "previously saved runner is still in use",
    );
    expect(alert).not.toHaveTextContent("Could not save the runner settings");

    await waitFor(() =>
      expect(commands.getRunnerSettings).toHaveBeenCalledTimes(1),
    );
    // Configured and active agree, so there is no fallback to offer.
    expect(
      screen.queryByText("The saved runner is not the one in use"),
    ).toBeNull();
    expect(screen.queryByText(/Restart QueenUI/)).toBeNull();
  });

  it("says that pairing saved a runner without moving games to it", async () => {
    /*
     * Pairing from *this computer*, which is this test's starting state: the
     * runner being paired is not the active one, so `pair_and_store` redeems the
     * code, stores the identity, writes the config — and stops. It does not call
     * `begin_switch`, so the runner it just saved is not the one playing games,
     * and only saving (or a restart) makes it so.
     *
     * It is not the only case any more, which is exactly why the sentence is
     * chosen from `restartRequired` rather than from the fact that pairing
     * happened: re-pairing the runner that is already active adopts the new
     * credential live, and then this sentence would be false.
     */
    const user = userEvent.setup();
    vi.mocked(invoke).mockResolvedValue({
      hostname: "runner-host",
      operatingSystem: "linux",
      architecture: "x86_64",
      logicalCpus: 32,
      url: "https://runner-host:17789",
      certFingerprint: "ab".repeat(32),
    });
    vi.mocked(commands.getRunnerSettings).mockResolvedValue(
      pairedNotSwitchedSettings,
    );
    renderPage();

    await user.selectOptions(
      await screen.findByLabelText("Execution target"),
      "remote",
    );
    await user.click(screen.getByRole("button", { name: /Pair runner/ }));
    await user.type(screen.getByLabelText("ssh alias or host"), "runner-host");
    await user.click(screen.getByRole("button", { name: "Pair runner" }));

    expect(
      await screen.findByText(
        "Paired. The runner's certificate is pinned to this machine and the endpoint is saved, but games keep running on this computer until you save the runner.",
      ),
    ).toBeInTheDocument();
    expect(
      screen.getByText("The saved runner is not the one in use"),
    ).toBeInTheDocument();
    expect(screen.queryByText("Restart QueenUI to switch runners")).toBeNull();
  });

  it("does not claim games stayed put when re-pairing adopted the runner live", async () => {
    /*
     * The case the sentence above must not be hard-coded for. Re-pairing the
     * runner that is *already* active — a rotated bearer for the same canonical
     * endpoint — publishes a new backend on the new credential and adopts it
     * immediately, so configured and active agree and there is no "until you
     * save the runner" left to promise. The backend's report of that is
     * `restartRequired: false`, which is what picks the shorter sentence.
     */
    const user = userEvent.setup();
    vi.mocked(invoke).mockResolvedValue({
      hostname: "runner-host",
      operatingSystem: "linux",
      architecture: "x86_64",
      logicalCpus: 32,
      url: "https://runner-host:17789",
      certFingerprint: "ab".repeat(32),
    });
    vi.mocked(commands.getRunnerSettings).mockResolvedValue(pairedSettings);
    renderPage({ initialSettings: pairedSettings });

    await user.click(
      await screen.findByRole("button", { name: /Pair runner…/ }),
    );
    await user.type(screen.getByLabelText("ssh alias or host"), "runner-host");
    await user.click(screen.getByRole("button", { name: "Pair runner" }));

    expect(
      await screen.findByText(
        "Paired. The runner's certificate is pinned to this machine.",
      ),
    ).toBeInTheDocument();
    // Neither half of the not-in-use claim: the runner it paired with is in use.
    expect(screen.queryByText(/games keep running on/)).toBeNull();
    expect(
      screen.queryByText("The saved runner is not the one in use"),
    ).toBeNull();
  });

  it("shows the adopt-the-new-identity refusal verbatim and re-reads what pairing stored", async () => {
    /*
     * `pair_and_store` commits the identity, then writes the config, then finds
     * the active runner has moved underneath it — and refuses to adopt. The
     * endpoint and credential on disk are the new runner's; the runner playing
     * games is this computer. Both facts have to reach the panel, or the
     * sentence's instruction ("save runner settings…") has nothing true behind
     * it and "Paired." would be the only thing on screen.
     */
    const user = userEvent.setup();
    const changed =
      "The active runner changed while pairing; save runner settings to adopt the new identity";
    vi.mocked(invoke).mockRejectedValue(changed);
    vi.mocked(commands.getRunnerSettings).mockResolvedValue(
      pairedNotSwitchedSettings,
    );
    renderPage({ initialSettings: pairedSettings });

    await user.click(
      await screen.findByRole("button", { name: /Pair runner…/ }),
    );
    await user.type(screen.getByLabelText("ssh alias or host"), "runner-host");
    await user.click(screen.getByRole("button", { name: "Pair runner" }));

    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toBe(changed);
    // Nothing may report a pairing that the backend refused to adopt.
    expect(screen.queryByText(/^Paired\./)).toBeNull();

    await waitFor(() =>
      expect(commands.getRunnerSettings).toHaveBeenCalledTimes(1),
    );
    expect(
      await screen.findByText("The saved runner is not the one in use"),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        /QueenUI is still running games on this computer; the runner at https:\/\/runner-host:17789 is saved but not in use/,
      ),
    ).toBeInTheDocument();

    // Saving is what adopts it, exactly as the sentence says.
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.getByRole("button", { name: "Save runner" })).toBeEnabled();
    expect(screen.getByLabelText("Runner URL")).toHaveValue(
      "https://runner-host:17789",
    );
  });

  it("deletes the pairing record only after confirmation, and says the live connection survives", async () => {
    const user = userEvent.setup();
    vi.mocked(invoke).mockResolvedValue(undefined);
    // Forgetting deletes the identity and leaves the endpoint in the config.
    vi.mocked(commands.getRunnerSettings).mockResolvedValue(
      unpairedUrlSettings,
    );
    renderPage({ initialSettings: pairedSettings });

    await user.click(
      await screen.findByRole("button", { name: /Forget the paired runner/ }),
    );
    expect(invoke).not.toHaveBeenCalledWith("forget_runner_credential");

    /*
     * `forget_runner_credential` deletes the stored records and nothing else —
     * the `RunnerClient` this session already opened keeps forwarding until the
     * process exits. Both the prompt and the receipt have to say so.
     */
    expect(
      screen.getByText(/A connection already running is not cut/),
    ).toBeInTheDocument();

    await user.click(
      screen.getByRole("button", { name: "Delete the credential" }),
    );
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("forget_runner_credential"),
    );
    const receipt = await screen.findByText(
      /bearer token and pinned certificate were deleted from this computer/,
    );
    // A live switch away from that runner closes the connection too, so the
    // restart is no longer the only ending this sentence can offer.
    expect(receipt).toHaveTextContent(
      "until you switch runners or QueenUI restarts",
    );
    expect(receipt).toHaveAttribute("role", "status");
  });

  it("re-reads the settings after a forget, so Test and Save stop offering a deleted credential", async () => {
    const user = userEvent.setup();
    vi.mocked(invoke).mockResolvedValue(undefined);
    vi.mocked(commands.getRunnerSettings).mockResolvedValue(
      unpairedUrlSettings,
    );
    renderPage({ initialSettings: pairedSettings });

    // Paired to begin with: both actions are legitimately available.
    expect(
      await screen.findByRole("button", { name: "Save runner" }),
    ).toBeEnabled();
    expect(
      screen.getByRole("button", { name: "Test connection" }),
    ).toBeEnabled();

    await user.click(
      screen.getByRole("button", { name: /Forget the paired runner/ }),
    );
    await user.click(
      screen.getByRole("button", { name: "Delete the credential" }),
    );

    /*
     * Without the re-read the panel keeps the stale `paired: true` it was handed
     * and both buttons stay live for a credential that no longer exists.
     */
    await waitFor(() =>
      expect(commands.getRunnerSettings).toHaveBeenCalledTimes(1),
    );
    expect(
      await screen.findByText("Not paired with https://runner-host:17789"),
    ).toBeInTheDocument();
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Save runner" }),
      ).toBeDisabled(),
    );
    expect(
      screen.getByRole("button", { name: "Test connection" }),
    ).toBeDisabled();
  });

  it("does not claim the credential survived when only the read-back failed", async () => {
    const user = userEvent.setup();
    vi.mocked(invoke).mockResolvedValue(undefined);
    vi.mocked(commands.getRunnerSettings).mockRejectedValue(
      new Error("no ipc"),
    );
    renderPage({ initialSettings: pairedSettings });

    await user.click(
      await screen.findByRole("button", { name: /Forget the paired runner/ }),
    );
    await user.click(
      screen.getByRole("button", { name: "Delete the credential" }),
    );

    // The deletion succeeded; only the refresh did not. Reporting this as a
    // failed deletion would send the operator looking for a credential that is
    // already gone.
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(
      "were deleted from this computer, but the runner settings could not be read back",
    );
    expect(alert).toHaveTextContent("no ipc");
    expect(alert).not.toHaveTextContent("still stored on this computer");
  });

  it("reports a rejected forget without claiming anything was deleted", async () => {
    const user = userEvent.setup();
    vi.mocked(invoke).mockRejectedValue(new Error("keyring is locked"));
    renderPage({ initialSettings: pairedSettings });

    await user.click(
      await screen.findByRole("button", { name: /Forget the paired runner/ }),
    );
    await user.click(
      screen.getByRole("button", { name: "Delete the credential" }),
    );

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(
      "Could not delete the saved runner credential",
    );
    expect(alert).toHaveTextContent("keyring is locked");
    expect(alert).toHaveTextContent("still stored on this computer");
    // The panel used to mock this path to success; a refusal must never leave
    // the deletion receipt on screen next to it.
    expect(screen.queryByText(/were deleted from this computer/)).toBeNull();
  });
});
