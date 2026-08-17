import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { RunnerPairingDialog } from "./RunnerPairingDialog";
import type { PairedRunner } from "../api/pairing";

vi.mock("../api/pairing", () => ({
  pairRunnerFromPayload: vi.fn(),
  pairRunnerViaSsh: vi.fn(),
}));

const pairing = await import("../api/pairing");

const paired: PairedRunner = {
  hostname: "runner-host",
  operatingSystem: "linux",
  architecture: "x86_64",
  logicalCpus: 32,
  url: "https://runner-host.lan:17788",
  certFingerprint:
    "3f2a9c1b4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f7081920a1b2c3d4e5f",
};

const SETUP_CODE =
  "queenui://pair?v=2&url=https://runner-host.lan&fp=3f2a&enroll=s";

function renderDialog(onPaired = vi.fn()) {
  render(<RunnerPairingDialog open onClose={vi.fn()} onPaired={onPaired} />);
  return onPaired;
}

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("runner pairing dialog", () => {
  it("offers ssh first and will not submit an alias ssh would misread", async () => {
    const user = userEvent.setup();
    renderDialog();

    expect(
      screen.getByRole("radio", { name: /Import over ssh/ }),
    ).toBeChecked();
    const submit = screen.getByRole("button", { name: "Pair runner" });
    expect(submit).toBeDisabled();

    await user.type(screen.getByLabelText("ssh alias or host"), "-oProxy=x");
    expect(submit).toBeDisabled();
    expect(screen.getByText(/cannot start with/)).toBeInTheDocument();
    expect(pairing.pairRunnerViaSsh).not.toHaveBeenCalled();
  });

  it("pairs over ssh and reports the machine, endpoint and pinned fingerprint", async () => {
    const user = userEvent.setup();
    vi.mocked(pairing.pairRunnerViaSsh).mockResolvedValue(paired);
    const onPaired = renderDialog();

    await user.type(
      screen.getByLabelText("ssh alias or host"),
      " runner-host ",
    );
    await user.click(screen.getByRole("button", { name: "Pair runner" }));

    await waitFor(() =>
      expect(pairing.pairRunnerViaSsh).toHaveBeenCalledWith("runner-host"),
    );
    expect(await screen.findByText("runner-host answered")).toBeInTheDocument();
    expect(
      screen.getByText(/https:\/\/runner-host.lan:17788/),
    ).toBeInTheDocument();
    // Grouped for reading against what `queen-runner pair` printed.
    expect(screen.getByLabelText("Pinned certificate fingerprint")).toHaveValue(
      "3F2A 9C1B 4D5E 6F70 8192 A3B4 C5D6 E7F8 091A 2B3C 4D5E 6F70 8192 0A1B 2C3D 4E5F",
    );
    expect(onPaired).toHaveBeenCalledWith(paired);
  });

  it("says the pin was stored when the backend does not report it", async () => {
    const user = userEvent.setup();
    // Today's agreed contract returns only `RunnerConnectionTest`, so this is
    // the shape the dialog actually has to survive.
    vi.mocked(pairing.pairRunnerViaSsh).mockResolvedValue({
      hostname: paired.hostname,
      operatingSystem: paired.operatingSystem,
      architecture: paired.architecture,
      logicalCpus: paired.logicalCpus,
    });
    renderDialog();

    await user.type(screen.getByLabelText("ssh alias or host"), "runner-host");
    await user.click(screen.getByRole("button", { name: "Pair runner" }));

    expect(
      await screen.findByText("The certificate pin was stored"),
    ).toBeInTheDocument();
    expect(
      screen.queryByLabelText("Pinned certificate fingerprint"),
    ).not.toBeInTheDocument();
  });

  it("hands a pasted setup code straight to the backend", async () => {
    const user = userEvent.setup();
    vi.mocked(pairing.pairRunnerFromPayload).mockResolvedValue(paired);
    renderDialog();

    await user.click(screen.getByRole("radio", { name: /Paste setup code/ }));
    const field = screen.getByLabelText("Setup code");
    await user.type(field, "not a setup code");
    expect(screen.getByRole("button", { name: "Pair runner" })).toBeDisabled();

    await user.clear(field);
    await user.type(field, SETUP_CODE);
    await user.click(screen.getByRole("button", { name: "Pair runner" }));

    await waitFor(() =>
      expect(pairing.pairRunnerFromPayload).toHaveBeenCalledWith(SETUP_CODE),
    );
    // The code is one-use and now spent: it must not still be on screen.
    expect(await screen.findByText("runner-host answered")).toBeInTheDocument();
    expect(screen.queryByLabelText("Setup code")).not.toBeInTheDocument();
    expect(document.body.textContent).not.toContain("enroll=s");
  });

  it("renders the backend's refusal verbatim and keeps the input", async () => {
    const user = userEvent.setup();
    vi.mocked(pairing.pairRunnerFromPayload).mockRejectedValue(
      "enrollment code expired at 2026-08-16T20:14:03Z",
    );
    renderDialog();

    await user.click(screen.getByRole("radio", { name: /Paste setup code/ }));
    await user.type(screen.getByLabelText("Setup code"), SETUP_CODE);
    await user.click(screen.getByRole("button", { name: "Pair runner" }));

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(
      "enrollment code expired at 2026-08-16T20:14:03Z",
    );
    // Still editable, still the operator's text: the usual fix is a new code.
    expect(screen.getByLabelText("Setup code")).toHaveValue(SETUP_CODE);
    expect(pairing.pairRunnerViaSsh).not.toHaveBeenCalled();
  });

  it("clears a failure when the operator switches carrier", async () => {
    const user = userEvent.setup();
    vi.mocked(pairing.pairRunnerViaSsh).mockRejectedValue(
      new Error("no route"),
    );
    renderDialog();

    await user.type(screen.getByLabelText("ssh alias or host"), "runner-host");
    await user.click(screen.getByRole("button", { name: "Pair runner" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("no route");

    await user.click(screen.getByRole("radio", { name: /Paste setup code/ }));
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });
});
