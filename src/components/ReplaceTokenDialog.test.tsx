import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ReplaceTokenDialog } from "./ReplaceTokenDialog";
import type { AccountProfile } from "../types";

const account: AccountProfile = {
  id: "queenbot",
  username: "QueenBot",
  engineId: "engine-1",
  rating: 2400,
  enabled: true,
};

function renderDialog(
  overrides: Partial<Parameters<typeof ReplaceTokenDialog>[0]> = {},
) {
  const onSubmit = vi.fn();
  const onClose = vi.fn();
  render(
    <ReplaceTokenDialog
      account={account}
      pending={false}
      onClose={onClose}
      onSubmit={onSubmit}
      {...overrides}
    />,
  );
  return { onSubmit, onClose };
}

afterEach(cleanup);

describe("what the replace-token dialog promises", () => {
  it("states that settings survive and that running games keep the old token", () => {
    /*
     * The reason this dialog exists. Replacing a token used to mean
     * disconnecting the account and connecting it again, which deletes the
     * secret, drops the campaign, and rebuilds the profile from the connect
     * dialog's engine picker — operators lost settings while fixing a token.
     *
     * `update_lichess_account_token` writes the secret and stops: no config
     * write, no restart, no bot stopped. Both halves of that are asserted here
     * because both are promises the operator acts on — one says it is safe to
     * do at all, the other says it is safe to do mid-game.
     */
    renderDialog();

    expect(screen.getByText("Only the stored token changes")).toBeVisible();
    expect(
      screen.getByText(/every other saved setting for QueenBot stay/),
    ).toBeVisible();
    expect(
      screen.getByText(
        /Games and matchmaking already running are untouched and keep using the old token; the new one is used from the next game or campaign start\./,
      ),
    ).toBeVisible();
  });

  it("says the token must belong to this account, before the backend has to", () => {
    // The backend refuses a token minted for another account. Saying so up
    // front turns a refusal into something the operator was warned about.
    renderDialog();
    expect(
      screen.getByText(/The token must belong to @QueenBot/),
    ).toBeVisible();
  });

  it("carries the same three-scope hint as the connect dialog", () => {
    /*
     * From `tokenScopes.matchmakingScopes`, which is a mirror of
     * `lichess::MATCHMAKING_SCOPES` — the same constant the connect dialog
     * reads. A token pasted here is minted from the same Lichess page and
     * needs the same boxes ticked; two hand-written copies of this line is one
     * edit away from two different required sets.
     */
    renderDialog();

    const hint = screen.getByText(
      /Required scopes: bot:play, challenge:read, challenge:write/,
    );
    expect(hint).toHaveTextContent(
      "a play-only token connects, but matchmaking will not work with it",
    );
    expect(screen.getByLabelText("New Lichess API token")).toHaveAttribute(
      "aria-describedby",
      hint.id,
    );
  });

  it("names the machine that will store the replacement", () => {
    // The same disclosure the connect dialog makes, from the same helper: in
    // remote mode the token is handed to the runner and stored there.
    renderDialog({ remoteRunner: true, runnerUrl: "https://runner:17789" });
    expect(
      screen.getByText(
        "The token is validated, then sent to the game runner (https://runner:17789) and stored there as a private file owned by the runner's service user.",
      ),
    ).toBeVisible();
  });
});

describe("the replace-token submission", () => {
  it("refuses to submit an empty field rather than calling the backend", async () => {
    // The backend answers "Enter a Lichess API token." to an empty token. The
    // dialog does not need a round trip to know that.
    const user = userEvent.setup();
    const { onSubmit } = renderDialog();

    const submit = screen.getByRole("button", { name: "Validate & replace" });
    expect(submit).toBeDisabled();
    await user.click(submit);
    expect(onSubmit).not.toHaveBeenCalled();

    await user.type(
      screen.getByLabelText("New Lichess API token"),
      "lip_replacement",
    );
    await user.click(submit);
    expect(onSubmit).toHaveBeenCalledExactlyOnceWith("lip_replacement");
  });

  it("keeps its verb while the token is being validated", () => {
    // Not a bare "…": a disabled control announced as "horizontal ellipsis" is
    // the one moment it most needs a name.
    renderDialog({ pending: true });
    expect(screen.getByRole("button", { name: "Validating…" })).toBeDisabled();
  });
});
