import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { Sidebar } from "./Sidebar";
import { emptySnapshot, type AppSnapshot } from "../types";

const snapshot: AppSnapshot = {
  ...emptySnapshot,
  accounts: [
    {
      id: "queenbot",
      username: "QueenBot",
      engineId: "engine-1",
      rating: 2400,
      enabled: true,
    },
  ],
  runtimes: [{ accountId: "queenbot", status: "online", error: null }],
};

function renderSidebar(overrides: Partial<Parameters<typeof Sidebar>[0]> = {}) {
  return render(
    <Sidebar
      snapshot={snapshot}
      activeNav="Overview"
      liveGameCount={0}
      activeCampaigns={0}
      onNavigate={() => {}}
      onAddAccount={() => {}}
      {...overrides}
    />,
  );
}

afterEach(cleanup);

describe("nav badges", () => {
  it("puts the live-game count in the item's accessible name", () => {
    /*
     * The counts used to reach assistive technology by no path at all: the
     * badge carried an `aria-label` on an `<em>` (role `emphasis`, which
     * prohibits an author name, so it was dropped), and the enclosing button
     * carried its own `aria-label` that replaced the whole subtree anyway.
     */
    renderSidebar({ liveGameCount: 3 });
    expect(
      screen.getByRole("button", { name: "Games, 3 live games" }),
    ).toBeInTheDocument();
  });

  it("singularizes a single live game", () => {
    renderSidebar({ liveGameCount: 1 });
    expect(
      screen.getByRole("button", { name: "Games, 1 live game" }),
    ).toBeInTheDocument();
  });

  it("names the campaign badge too", () => {
    renderSidebar({ activeCampaigns: 2 });
    expect(
      screen.getByRole("button", { name: "Challenges, 2 active campaigns" }),
    ).toBeInTheDocument();
  });

  it("leaves an item with no badge named by its label alone", () => {
    renderSidebar();
    expect(screen.getByRole("button", { name: "Games" })).toBeInTheDocument();
  });
});

describe("fleet rows", () => {
  it("shows the status in the app's own vocabulary", () => {
    // `online` is written "Connected" wherever it is spelled out.
    renderSidebar();
    expect(screen.getByText("Connected")).toBeInTheDocument();
  });

  it("makes a failing bot look different from a working one", () => {
    renderSidebar({
      snapshot: {
        ...snapshot,
        runtimes: [
          {
            accountId: "queenbot",
            status: "error",
            error: "token rejected by Lichess",
          },
        ],
      },
    });

    const line = screen.getByText("Error: token rejected by Lichess");
    expect(line).toHaveClass("mini-bot-error");
  });
});
