import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ChallengesPage } from "./ChallengesPage";
import { defaultTimeControls } from "../lib/timeControls";
import {
  emptySnapshot,
  type AppSnapshot,
  type CampaignRuntime,
  type LiveGame,
} from "../types";

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
};

function renderPage(
  overrides: Partial<Parameters<typeof ChallengesPage>[0]> = {},
) {
  const onStart = vi.fn(() => Promise.resolve(true));
  render(
    <ChallengesPage
      snapshot={snapshot}
      timeControls={defaultTimeControls}
      busy={new Set()}
      onDirectChallenge={() => {}}
      onStart={onStart}
      onStop={() => Promise.resolve(true)}
      {...overrides}
    />,
  );
  return { onStart };
}

afterEach(cleanup);

describe("campaign setup form", () => {
  it("does not start matchmaking from an Enter press in the form", async () => {
    /*
     * The confirmation line tells the operator to "start from the Live
     * controller". The form has no submit button and two number fields, so
     * the browser suppresses implicit submission — this test pins that,
     * because the copy would be a lie if Enter armed a campaign.
     */
    const user = userEvent.setup();
    const { onStart } = renderPage();

    const minRating = screen.getByRole("spinbutton", {
      name: /Minimum rating/,
    });
    await user.click(minRating);
    await user.keyboard("{Enter}");

    expect(onStart).not.toHaveBeenCalled();
  });

  it("frames a scheduler error rather than dropping the backend string in bare", () => {
    renderPage({
      snapshot: {
        ...snapshot,
        campaignRuntimes: [
          {
            accountId: "queenbot",
            status: "error",
            activeGames: 0,
            pendingChallenges: 0,
            eligibleBots: 0,
            onlineBotsScanned: 0,
            challengesSent: 0,
            gamesStarted: 0,
            lastOpponent: null,
            activity: "",
            error: "429 from Lichess",
            nextScanAt: null,
            stopAt: null,
            events: [],
          },
        ],
      },
    });

    const alert = screen.getByRole("alert");
    expect(alert).toHaveTextContent("Matchmaking reported a problem");
    expect(alert).toHaveTextContent("429 from Lichess");
  });

  it("sends incoming acceptance and a time limit as campaign settings", async () => {
    const user = userEvent.setup();
    const { onStart } = renderPage();

    await user.click(
      screen.getByRole("switch", {
        name: "Accept matching incoming challenges",
      }),
    );
    await user.click(screen.getByRole("button", { name: "Time limit" }));
    const duration = screen.getByRole("spinbutton", { name: "Run for" });
    await user.clear(duration);
    await user.type(duration, "2");
    await user.click(screen.getByRole("button", { name: "Start matchmaking" }));

    expect(onStart).toHaveBeenCalledWith(
      expect.objectContaining({
        acceptIncomingChallenges: true,
        stopAfterMinutes: 120,
        stopAfterGames: null,
      }),
    );
  });

  it("sends a game limit without a time limit", async () => {
    const user = userEvent.setup();
    const { onStart } = renderPage();

    await user.click(screen.getByRole("button", { name: "Game limit" }));
    const games = screen.getByRole("spinbutton", {
      name: "Stop after games started",
    });
    await user.clear(games);
    await user.type(games, "24");
    await user.click(screen.getByRole("button", { name: "Start matchmaking" }));

    expect(onStart).toHaveBeenCalledWith(
      expect.objectContaining({
        acceptIncomingChallenges: false,
        stopAfterMinutes: null,
        stopAfterGames: 24,
      }),
    );
  });
});

describe("the rated default", () => {
  it("preselects Rated for an account with no saved campaign", async () => {
    /*
     * The backend has defaulted a campaign to rated for some time
     * (`default_campaign_rated`), while this form opened on Casual — so "the
     * default" meant two different things depending on who was asked, and an
     * operator who armed matchmaking without touching Mode got games that moved
     * no rating.
     */
    const user = userEvent.setup();
    const { onStart } = renderPage();

    expect(screen.getByRole("button", { name: "Rated" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    expect(screen.getByRole("button", { name: "Casual" })).toHaveAttribute(
      "aria-pressed",
      "false",
    );

    // And it is what actually gets sent, not just what is highlighted.
    await user.click(screen.getByRole("button", { name: "Start matchmaking" }));
    expect(onStart).toHaveBeenCalledWith(
      expect.objectContaining({
        rated: true,
        acceptIncomingChallenges: false,
        stopAfterMinutes: null,
        stopAfterGames: null,
      }),
    );
  });

  it("lets a saved casual campaign stay casual", () => {
    /*
     * The default may only ever decide what an *unconfigured* account shows.
     * A saved campaign is the operator's own answer to this question, and a
     * default that overwrote it would silently make a deliberately casual
     * campaign rated the next time the page was opened.
     */
    renderPage({
      snapshot: {
        ...snapshot,
        campaigns: [
          {
            accountId: "queenbot",
            minRating: 1800,
            maxRating: 2600,
            concurrency: 1,
            clockLimit: 180,
            clockIncrement: 2,
            rated: false,
            color: "random",
            acceptIncomingChallenges: false,
            stopAfterMinutes: null,
            stopAfterGames: null,
          },
        ],
      },
    });

    expect(screen.getByRole("button", { name: "Casual" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    expect(screen.getByRole("button", { name: "Rated" })).toHaveAttribute(
      "aria-pressed",
      "false",
    );
  });

  it("still takes a saved rated campaign from the snapshot, not from the default", () => {
    // The mirror image, so the test above cannot pass merely because the
    // saved value and the default happen to disagree.
    renderPage({
      snapshot: {
        ...snapshot,
        campaigns: [
          {
            accountId: "queenbot",
            minRating: 2000,
            maxRating: 2400,
            concurrency: 2,
            clockLimit: 300,
            clockIncrement: 3,
            rated: true,
            color: "white",
            acceptIncomingChallenges: false,
            stopAfterMinutes: null,
            stopAfterGames: null,
          },
        ],
      },
    });

    expect(screen.getByRole("button", { name: "Rated" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    expect(
      screen.getByRole("spinbutton", { name: /Minimum rating/ }),
    ).toHaveValue(2000);
  });
});

describe("live game count", () => {
  function game(id: string, accountId: string, status: string): LiveGame {
    return {
      id,
      accountId,
      botUsername: "QueenBot",
      opponent: `Opponent-${id}`,
      botRating: null,
      opponentRating: null,
      color: "white",
      initialFen: "startpos",
      moves: "e2e4",
      status,
      whiteTime: 60_000,
      blackTime: 60_000,
      whiteIncrement: 0,
      blackIncrement: 0,
      clockUpdatedAt: 0,
      result: null,
      engineLine: null,
      engineInfo: null,
      engineThinking: false,
      error: null,
    };
  }

  function stoppedRuntime(activeGames: number): CampaignRuntime {
    return {
      accountId: "queenbot",
      status: "stopped",
      activeGames,
      pendingChallenges: 0,
      eligibleBots: 0,
      onlineBotsScanned: 0,
      challengesSent: 0,
      gamesStarted: 0,
      lastOpponent: null,
      activity: "Ready",
      error: null,
      nextScanAt: null,
      stopAt: null,
      events: [],
    };
  }

  it("counts this account's live games, not the scheduler's last total", () => {
    /*
     * Stopping a campaign clears `pendingChallenges` but leaves `activeGames`
     * at whatever the scheduler last saw, and the loop that refreshed it has
     * exited — so this page claimed "Active games 2" for the rest of the
     * session while the sidebar badge, the Overview status strip and the Games
     * page all counted the snapshot down to 0. One definition now: live games
     * in the snapshot, belonging to this account.
     */
    renderPage({
      snapshot: {
        ...snapshot,
        games: [
          game("live", "queenbot", "started"),
          game("done", "queenbot", "mate"),
        ],
        campaignRuntimes: [stoppedRuntime(2)],
      },
    });

    expect(screen.getByText("Active games").parentElement).toHaveTextContent(
      "1",
    );
    expect(document.querySelector(".capacity-ring strong")?.textContent).toBe(
      "1",
    );
  });

  it("does not count another account's games", () => {
    renderPage({
      snapshot: {
        ...snapshot,
        games: [game("elsewhere", "other-bot", "started")],
        campaignRuntimes: [stoppedRuntime(1)],
      },
    });

    expect(screen.getByText("Active games").parentElement).toHaveTextContent(
      "0",
    );
    expect(document.querySelector(".capacity-ring strong")?.textContent).toBe(
      "0",
    );
  });
});
