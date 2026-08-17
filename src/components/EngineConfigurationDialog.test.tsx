import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { EngineConfigurationDialog } from "./EngineConfigurationDialog";
import type { EngineProfile } from "../types";

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

const engine: EngineProfile = {
  id: "engine-1",
  name: "Queen",
  path: "C:\\queen.exe",
  author: null,
  optionCount: 3,
  options: [
    {
      name: "Hash",
      optionType: "spin",
      defaultValue: "16",
      value: "16",
      min: 1,
      max: 4096,
      choices: [],
    },
    {
      name: "Personality",
      // Not a UCI type. It used to fall into a silent `else`; now it maps to a
      // text field by an explicit rule.
      optionType: "wildcard",
      defaultValue: "balanced",
      value: "balanced",
      min: null,
      max: null,
      choices: [],
    },
  ],
  openingBook: {
    enabled: true,
    path: "C:\\books\\perf.bin",
    name: "perf.bin",
    format: "polyglot",
    maxPlies: 20,
    topMovePercent: 10,
    entryCount: 4200,
  },
};

function props() {
  return {
    engine,
    busy: new Set<string>(),
    onClose: vi.fn(),
    onSaveOptions: vi.fn(() => Promise.resolve(true)),
    onRefreshOptions: vi.fn(() => Promise.resolve(true)),
    onSaveBook: vi.fn(() => Promise.resolve(true)),
    onClearBook: vi.fn(() => Promise.resolve(true)),
    showNotice: vi.fn(),
  };
}

function renderDialog(overrides: Partial<ReturnType<typeof props>> = {}) {
  const merged = { ...props(), ...overrides };
  render(<EngineConfigurationDialog {...merged} />);
  return merged;
}

afterEach(cleanup);

describe("engine configuration dialog", () => {
  it("closes straight away when nothing has been edited", async () => {
    const user = userEvent.setup();
    const { onClose } = renderDialog();

    await user.keyboard("{Escape}");

    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
  });

  it("asks before discarding an in-progress edit session", async () => {
    // Escape or a stray overlay click used to throw away sixty edited options
    // with no prompt.
    const user = userEvent.setup();
    const { onClose } = renderDialog();

    await user.click(screen.getByRole("tab", { name: /UCI options/ }));
    const hash = screen.getByRole("spinbutton", { name: "Hash" });
    await user.clear(hash);
    await user.type(hash, "512");
    await user.keyboard("{Escape}");

    expect(
      await screen.findByRole("heading", { name: "Discard your changes?" }),
    ).toBeInTheDocument();
    expect(onClose).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "Discard and close" }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("closes straight away when × is clicked with nothing edited", async () => {
    const user = userEvent.setup();
    const { onClose } = renderDialog();

    await user.click(screen.getByRole("button", { name: "Close" }));

    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
    expect(screen.queryByText("Discard your changes?")).toBeNull();
  });

  it("asks before × discards an in-progress edit session", async () => {
    /*
     * × was a `Dialog.Close`, which reaches `onOpenChange` directly and so
     * never ran the guard Escape and overlay clicks go through. It was the one
     * dismissal that threw the edits away in silence — and the obvious one to
     * reach for.
     */
    const user = userEvent.setup();
    const { onClose } = renderDialog();

    await user.click(screen.getByRole("tab", { name: /UCI options/ }));
    const hash = screen.getByRole("spinbutton", { name: "Hash" });
    await user.clear(hash);
    await user.type(hash, "512");
    await user.click(screen.getByRole("button", { name: "Close" }));

    expect(
      await screen.findByRole("heading", { name: "Discard your changes?" }),
    ).toBeInTheDocument();
    expect(onClose).not.toHaveBeenCalled();

    // The edit survives the question, so cancelling leaves the session intact
    // rather than returning to a dialog that has already been reset.
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    await waitFor(() =>
      expect(screen.getByRole("spinbutton", { name: "Hash" })).toHaveValue(512),
    );
    expect(onClose).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "Close" }));
    await user.click(
      await screen.findByRole("button", { name: "Discard and close" }),
    );
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("confirms before removing a saved opening book", async () => {
    const user = userEvent.setup();
    const { onClearBook } = renderDialog();

    await user.click(screen.getByRole("button", { name: /Remove book/ }));
    expect(onClearBook).not.toHaveBeenCalled();

    // The modal marks the rest of the page aria-hidden, so this resolves to
    // the confirmation's own button.
    await user.click(screen.getByRole("button", { name: "Remove book" }));
    await waitFor(() => expect(onClearBook).toHaveBeenCalledTimes(1));
  });

  it("renders an unrecognised UCI option type as a text field", async () => {
    const user = userEvent.setup();
    renderDialog();

    await user.click(screen.getByRole("tab", { name: /UCI options/ }));
    const control = screen.getByRole("textbox", { name: "Personality" });
    expect(control).toHaveValue("balanced");
  });

  it("asks before overwriting every UCI option with its default", async () => {
    // The identically-labelled reset in Settings confirms, and this dialog
    // already confirms before discarding the same edits on close.
    const user = userEvent.setup();
    renderDialog();

    await user.click(screen.getByRole("tab", { name: /UCI options/ }));
    await user.type(screen.getByRole("spinbutton", { name: "Hash" }), "0");
    expect(screen.getByRole("spinbutton", { name: "Hash" })).toHaveValue(160);

    await user.click(screen.getByRole("button", { name: "Reset defaults" }));
    expect(
      await screen.findByRole("heading", { name: "Reset every UCI option?" }),
    ).toBeInTheDocument();
    /*
     * Exactly one question on screen. Opening a confirmation moves focus out
     * of the engine dialog's own content, which Radix reports as an outside
     * interaction — that used to run the dirty guard and stack "Discard your
     * changes?" on top of the question just asked.
     */
    expect(screen.queryByText("Discard your changes?")).toBeNull();

    await user.click(screen.getByRole("button", { name: "Reset to defaults" }));
    await waitFor(() =>
      expect(screen.getByRole("spinbutton", { name: "Hash" })).toHaveValue(16),
    );
  });

  it("keeps the reset when it is cancelled", async () => {
    const user = userEvent.setup();
    renderDialog();

    await user.click(screen.getByRole("tab", { name: /UCI options/ }));
    await user.type(screen.getByRole("spinbutton", { name: "Hash" }), "0");
    await user.click(screen.getByRole("button", { name: "Reset defaults" }));
    await user.click(await screen.findByRole("button", { name: "Cancel" }));

    await waitFor(() =>
      expect(screen.getByRole("spinbutton", { name: "Hash" })).toHaveValue(160),
    );
  });

  it("asks one question at a time when the book is removed with edits pending", async () => {
    // Same stacking bug on the pre-existing path: the book-removal question
    // used to arrive with "Discard your changes?" already on top of it.
    const user = userEvent.setup();
    renderDialog();

    await user.click(screen.getByRole("tab", { name: /UCI options/ }));
    await user.type(screen.getByRole("spinbutton", { name: "Hash" }), "0");
    await user.click(screen.getByRole("tab", { name: /Opening book/ }));
    await user.click(screen.getByRole("button", { name: /Remove book/ }));

    expect(
      await screen.findByRole("heading", {
        name: "Remove the opening book?",
      }),
    ).toBeInTheDocument();
    expect(screen.queryByText("Discard your changes?")).toBeNull();
  });

  it("says Removing on the remove button, not Validating on the save button", async () => {
    /*
     * Saving and clearing the book shared one busy key, so a removal in
     * flight made the *Save* button read "Validating…" — the wrong control
     * narrating the wrong operation.
     */
    renderDialog({ busy: new Set([`book-clear-${engine.id}`]) });

    expect(screen.getByRole("button", { name: /Removing…/ })).toBeDisabled();
    expect(screen.queryByRole("button", { name: /Validating…/ })).toBeNull();
    expect(
      screen.getByRole("button", { name: "Save book policy" }),
    ).toBeDisabled();
  });

  it("does not describe a cleared depth field as ply 0", async () => {
    const user = userEvent.setup();
    renderDialog();

    const plies = screen.getByRole("spinbutton", {
      name: "Maximum book plies",
    });
    await user.clear(plies);

    expect(
      screen.getByText("Enter how deep the book may be used"),
    ).toBeInTheDocument();
    expect(screen.queryByText(/ply 0/)).not.toBeInTheDocument();
  });
});
