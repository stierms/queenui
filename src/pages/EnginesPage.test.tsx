import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AppSnapshot } from "../types";
import { EnginesPage } from "./EnginesPage";

const snapshot: AppSnapshot = {
  engines: [
    {
      id: "stockfish",
      name: "Stockfish 17.1",
      path: "/home/operator/.local/lib/queenui/stockfish",
      author: null,
      optionCount: 60,
      options: [
        {
          name: "SyzygyPath",
          optionType: "string",
          defaultValue: "<empty>",
          value: "",
          min: null,
          max: null,
          choices: [],
        },
        ...Array.from({ length: 59 }, (_, index) => ({
          name: `Engine option ${index + 1}`,
          optionType: "spin",
          defaultValue: "1",
          value: "1",
          min: 1,
          max: 128,
          choices: [],
        })),
      ],
      openingBook: null,
    },
  ],
  accounts: [],
  runtimes: [],
  games: [],
  campaigns: [],
  campaignRuntimes: [],
};

afterEach(cleanup);

describe("remote engine management", () => {
  it("opens the scoped trusted-engine browser without exposing path or upload controls", async () => {
    const user = userEvent.setup();
    const onAdd = vi.fn();
    const onRegister = vi.fn().mockResolvedValue(true);
    render(
      <EnginesPage
        snapshot={snapshot}
        busy={new Set()}
        remoteRunner
        showNotice={() => {}}
        onAdd={onAdd}
        onRegister={onRegister}
        onRemove={() => {}}
        onSaveOptions={() => Promise.resolve(true)}
        onRefreshOptions={() => Promise.resolve(true)}
        onSaveBook={() => Promise.resolve(true)}
        onClearBook={() => Promise.resolve(true)}
      />,
    );

    await user.click(
      screen.getByRole("button", { name: "Browse trusted engines" }),
    );
    expect(
      screen.getByRole("heading", { name: "Browse trusted engines" }),
    ).toBeInTheDocument();
    expect(screen.queryByLabelText("Executable path on runner")).toBeNull();
    expect(screen.queryByRole("button", { name: /Upload/ })).toBeNull();
  });
});

describe("engine probe truth", () => {
  function renderEngines(engines: AppSnapshot["engines"]) {
    render(
      <EnginesPage
        snapshot={{ ...snapshot, engines }}
        busy={new Set()}
        showNotice={() => {}}
        onAdd={() => {}}
        onRegister={() => Promise.resolve(true)}
        onRemove={() => {}}
        onSaveOptions={() => Promise.resolve(true)}
        onRefreshOptions={() => Promise.resolve(true)}
        onSaveBook={() => Promise.resolve(true)}
        onClearBook={() => Promise.resolve(true)}
      />,
    );
  }

  const base = snapshot.engines[0];

  it("dates a successful probe instead of claiming standing readiness", () => {
    renderEngines([
      { ...base, probeOk: true, lastProbedAtMs: Date.now() - 4 * 60_000 },
    ]);

    const badge = screen.getByText("UCI verified 4m ago");
    expect(badge.className).toContain("engine-probe-ok");
    // Config load does not re-probe, so the age is the claim's freshness
    // bound, not decoration.
    expect(badge.title).toMatch(/does not re-probe/);
  });

  it("says the last probe failed for a retained profile", () => {
    renderEngines([
      {
        ...base,
        probeOk: false,
        lastProbedAtMs: Date.now() - 2 * 86_400_000,
      },
    ]);

    const badge = screen.getByText("Probe failed 2d ago");
    expect(badge.className).toContain("engine-probe-failed");
    expect(badge.className).not.toContain("engine-probe-ok");
    expect(screen.queryByText(/UCI verified/)).toBeNull();
    // The profile is still listed — the removal is the operator's call.
    expect(
      screen.getByRole("heading", { name: "Stockfish 17.1" }),
    ).toBeInTheDocument();
  });

  it("stays neutral for a profile that has never been probed", () => {
    // An old config: no probe was recorded when the profile was saved, which
    // is not the same as a probe that succeeded.
    renderEngines([base]);

    const badge = screen.getByText("Not probed yet");
    expect(badge.className).toContain("engine-probe-unknown");
    expect(badge.className).not.toContain("engine-probe-ok");
    expect(screen.queryByText(/UCI verified/)).toBeNull();
    expect(screen.queryByText(/UCI ready/)).toBeNull();
  });

  it("keeps the green claim off a profile with no probe result", () => {
    // Every state renders exactly one badge per card, and only the verified
    // one may be green.
    renderEngines([
      {
        ...base,
        id: "a",
        name: "A",
        probeOk: true,
        lastProbedAtMs: Date.now(),
      },
      {
        ...base,
        id: "b",
        name: "B",
        probeOk: false,
        lastProbedAtMs: Date.now(),
      },
      { ...base, id: "c", name: "C" },
    ]);

    const badges = document.querySelectorAll(".engine-probe");
    expect(badges.length).toBe(3);
    expect(document.querySelectorAll(".engine-probe-ok").length).toBe(1);
  });
});

describe("destructive engine actions", () => {
  const assignedSnapshot: AppSnapshot = {
    ...snapshot,
    accounts: [
      {
        id: "queenbot",
        username: "QueenBot",
        engineId: "stockfish",
        rating: null,
        enabled: true,
      },
    ],
  };

  function renderPage(onRemove: () => void, source = assignedSnapshot) {
    render(
      <EnginesPage
        snapshot={source}
        busy={new Set()}
        showNotice={() => {}}
        onAdd={() => {}}
        onRegister={() => Promise.resolve(true)}
        onRemove={onRemove}
        onSaveOptions={() => Promise.resolve(true)}
        onRefreshOptions={() => Promise.resolve(true)}
        onSaveBook={() => Promise.resolve(true)}
        onClearBook={() => Promise.resolve(true)}
      />,
    );
  }

  it("confirms before removing a profile and names the bots that lose it", async () => {
    const user = userEvent.setup();
    const onRemove = vi.fn();
    renderPage(onRemove);

    await user.click(screen.getByRole("button", { name: /Remove/ }));
    expect(onRemove).not.toHaveBeenCalled();
    expect(
      screen.getByRole("heading", { name: "Remove Stockfish 17.1?" }),
    ).toBeInTheDocument();
    expect(screen.getByText(/QueenBot is assigned to it/)).toBeInTheDocument();

    await user.click(
      screen.getByRole("button", { name: "Remove engine profile" }),
    );
    expect(onRemove).toHaveBeenCalledTimes(1);
  });

  it("keeps the profile when the confirmation is dismissed", async () => {
    const user = userEvent.setup();
    const onRemove = vi.fn();
    renderPage(onRemove, snapshot);

    await user.click(screen.getByRole("button", { name: /Remove/ }));
    await user.click(screen.getByRole("button", { name: "Cancel" }));

    expect(onRemove).not.toHaveBeenCalled();
  });

  /*
   * Stacking. Both dialogs on this page are modal, so a card button behind one
   * of them cannot be clicked by an operator — `pointer-events: none` on the
   * body, a focus trap, and an aria-hidden background. `fireEvent` dispatches
   * straight at the element and so reaches it anyway, which is how the stack
   * was found. The page has to refuse the second open: the removal
   * confirmation renders before the configuration dialog, so it stacked
   * underneath at the same z-index and only became visible — unrequested —
   * when the dialog above it closed.
   */
  function openDialogCount() {
    return document.querySelectorAll('[role="dialog"]').length;
  }

  it("refuses a removal confirmation while the configuration dialog is open", async () => {
    const user = userEvent.setup();
    const onRemove = vi.fn();
    renderPage(onRemove, snapshot);

    const remove = screen.getByRole("button", { name: /Remove/ });
    await user.click(screen.getByRole("button", { name: "Configure" }));
    expect(openDialogCount()).toBe(1);

    fireEvent.click(remove);

    expect(openDialogCount()).toBe(1);
    expect(screen.queryByText("Remove Stockfish 17.1?")).toBeNull();
    expect(onRemove).not.toHaveBeenCalled();
  });

  it("refuses the configuration dialog while a removal confirmation is open", async () => {
    const user = userEvent.setup();
    const onRemove = vi.fn();
    renderPage(onRemove, snapshot);

    const configure = screen.getByRole("button", { name: "Configure" });
    await user.click(screen.getByRole("button", { name: /Remove/ }));
    expect(openDialogCount()).toBe(1);

    fireEvent.click(configure);

    expect(openDialogCount()).toBe(1);
    expect(screen.queryByText("Configure Stockfish 17.1")).toBeNull();
  });

  it("refuses the trusted-engine browser while a removal confirmation is open", async () => {
    const user = userEvent.setup();
    render(
      <EnginesPage
        snapshot={snapshot}
        busy={new Set()}
        remoteRunner
        showNotice={() => {}}
        onAdd={() => {}}
        onRegister={() => Promise.resolve(true)}
        onRemove={() => {}}
        onSaveOptions={() => Promise.resolve(true)}
        onRefreshOptions={() => Promise.resolve(true)}
        onSaveBook={() => Promise.resolve(true)}
        onClearBook={() => Promise.resolve(true)}
      />,
    );

    const browse = screen.getByRole("button", {
      name: "Browse trusted engines",
    });
    await user.click(screen.getByRole("button", { name: /Remove/ }));
    expect(openDialogCount()).toBe(1);

    fireEvent.click(browse);

    expect(openDialogCount()).toBe(1);
  });
});
