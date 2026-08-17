import { useState } from "react";
import { KeyRound } from "lucide-react";
import { tokenStorageCopy } from "../api/credentials";
import { tokenScopeHint } from "../lib/tokenScopes";
import { Button, Dialog } from "../ui/primitives";
import type { AccountProfile } from "../types";

/**
 * Paste a new Lichess token for an account QueenUI already has.
 *
 * Until this existed, a token that had been revoked, expired, or minted without
 * the challenge scopes could only be fixed by disconnecting the account and
 * connecting it again — which deletes the stored secret, drops the account's
 * campaign, and rebuilds the profile from whatever the connect dialog's engine
 * picker happened to be showing. Operators lost settings to a bad paste.
 *
 * `update_lichess_account_token` writes the secret and stops there: no config
 * write, no restart, no bot stopped. Every sentence below is that fact stated
 * plainly, because the value of this dialog is entirely in what it does *not*
 * touch, and an operator who does not believe that will keep using the
 * destructive route.
 */
export function ReplaceTokenDialog({
  account,
  pending,
  remoteRunner = false,
  runnerUrl,
  onClose,
  onSubmit,
}: {
  account: AccountProfile;
  pending: boolean;
  /** Which machine stores the replacement, exactly as the connect dialog says. */
  remoteRunner?: boolean;
  runnerUrl?: string | null;
  onClose: () => void;
  onSubmit: (token: string) => void;
}) {
  const [token, setToken] = useState("");
  const canSubmit = Boolean(token.trim()) && !pending;
  const storage = tokenStorageCopy(remoteRunner, runnerUrl);
  return (
    <Dialog.Root
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-20 bg-black/70 backdrop-blur-[7px]" />
        <Dialog.Content className="account-modal fixed top-1/2 left-1/2 z-[21] -translate-x-1/2 -translate-y-1/2 focus:outline-none">
          <div className="modal-head">
            <div className="modal-icon">
              <KeyRound size={20} />
            </div>
            <div>
              <span className="eyebrow">Stored credential</span>
              <Dialog.Title>Replace {account.username}’s token</Dialog.Title>
              <Dialog.Description>{storage.sentence}</Dialog.Description>
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
          <form
            onSubmit={(event) => {
              event.preventDefault();
              if (canSubmit) onSubmit(token);
            }}
          >
            <div className="account-form">
              <label>
                <span>New Lichess API token</span>
                {/* Explicit, because the hint below shares this label and would
                    otherwise become part of the field's name. */}
                <input
                  type="password"
                  autoFocus
                  value={token}
                  aria-label="New Lichess API token"
                  aria-describedby="replace-token-scope"
                  onChange={(event) => setToken(event.target.value)}
                  placeholder="lip_…"
                  autoComplete="off"
                />
                {/* The same required set the connect dialog names, from the
                    same constant. */}
                <small id="replace-token-scope">{tokenScopeHint}</small>
              </label>
              <div className="credential-note">
                <KeyRound size={17} />
                <p>
                  <strong>Only the stored token changes</strong>
                  <small>
                    The engine profile, matchmaking setup and every other saved
                    setting for {account.username} stay exactly as they are.
                  </small>
                  {/*
                   * The one thing an operator replacing a token mid-session
                   * most needs to know, and the reason this is safe to do
                   * without stopping the bot: a running game holds the client
                   * it started with.
                   */}
                  <small>
                    Games and matchmaking already running are untouched and keep
                    using the old token; the new one is used from the next game
                    or campaign start.
                  </small>
                  <small>
                    The token must belong to @{account.username}. A token for
                    another Lichess account is refused, not connected in its
                    place.
                  </small>
                </p>
              </div>
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
                {pending ? "Validating…" : "Validate & replace"}
              </Button>
            </div>
          </form>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
