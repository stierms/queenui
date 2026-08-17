import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { AccountComposer } from "./AccountComposer";
import type { EngineProfile } from "../types";

const engine: EngineProfile = {
  id: "engine",
  name: "Fake UCI",
  path: "fake",
  author: null,
  optionCount: 0,
  options: [],
  openingBook: null,
};

afterEach(cleanup);

describe("the scope hint under the token field", () => {
  it("names all three required scopes before the token is pasted", () => {
    /*
     * This line used to say "Required scope: Play games with the bot API
     * (bot:play)" — the playing scope alone. It is read while the operator is
     * ticking boxes on the Lichess token page, and it under-described the set
     * by two: a token minted from it connects, plays, and cannot run a single
     * campaign. The connect result reports what the pasted token turned out to
     * carry; this is the half of the hint that arrives before there is
     * anything to check.
     */
    render(
      <AccountComposer
        engines={[engine]}
        pending={false}
        onClose={() => {}}
        onSubmit={() => Promise.resolve()}
      />,
    );

    const hint = screen.getByText(
      /Required scopes: bot:play, challenge:read, challenge:write/,
    );
    expect(hint).toHaveTextContent(
      "a play-only token connects, but matchmaking will not work with it",
    );
    // The field points at it, so it is announced rather than merely visible.
    expect(screen.getByLabelText("Lichess API token")).toHaveAttribute(
      "aria-describedby",
      hint.id,
    );
  });
});

describe("Lichess credential storage disclosure", () => {
  it("states the exact embedded Windows storage and deletion lifecycle", () => {
    render(
      <AccountComposer
        engines={[engine]}
        pending={false}
        onClose={() => {}}
        onSubmit={() => Promise.resolve()}
      />,
    );
    expect(
      screen.getByText(
        "The token is validated, then stored in Windows Credential Manager on this PC.",
      ),
    ).toBeInTheDocument();
    expect(
      screen.getByText("Stored in Windows Credential Manager"),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        "Disconnecting the account from the Bot fleet deletes it again.",
      ),
    ).toBeInTheDocument();
  });

  it("states the exact remote runner file storage and removal lifecycle", () => {
    render(
      <AccountComposer
        engines={[engine]}
        pending={false}
        remoteRunner
        runnerUrl="http://127.0.0.1:7788"
        onClose={() => {}}
        onSubmit={() => Promise.resolve()}
      />,
    );
    expect(
      screen.getByText(
        "The token is validated, then sent to the game runner (http://127.0.0.1:7788) and stored there as a private file owned by the runner's service user.",
      ),
    ).toBeInTheDocument();
    expect(
      screen.getByText("Stored in the runner machine (http://127.0.0.1:7788)"),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        "It is not kept on this PC. Removing the account here removes it from the runner too.",
      ),
    ).toBeInTheDocument();
  });
});
