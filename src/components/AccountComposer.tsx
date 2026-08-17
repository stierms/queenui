import { useState } from "react";
import { ChevronDown, FileKey } from "lucide-react";
import { tokenStorageCopy } from "../api/credentials";
import { tokenScopeHint } from "../lib/tokenScopes";
import { Button, Dialog } from "../ui/primitives";
import type { EngineProfile } from "../types";

export function AccountComposer({
  engines,
  pending,
  remoteRunner = false,
  runnerUrl,
  onClose,
  onSubmit,
}: {
  engines: EngineProfile[];
  pending: boolean;
  /**
   * Which machine will hold the token. In remote mode it is handed to the
   * runner and stored there; the dialog used to claim Windows Credential
   * Manager unconditionally, which was wrong in exactly the case that matters.
   */
  remoteRunner?: boolean;
  runnerUrl?: string | null;
  onClose: () => void;
  onSubmit: (token: string, engineId: string) => Promise<void>;
}) {
  const [token, setToken] = useState("");
  const [engineId, setEngineId] = useState(engines[0]?.id ?? "");
  const canSubmit = Boolean(token.trim()) && Boolean(engineId) && !pending;
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
        <Dialog.Content className="account-modal fixed left-1/2 top-1/2 z-[21] -translate-x-1/2 -translate-y-1/2 focus:outline-none">
          <div className="modal-head">
            <div className="modal-icon">
              <FileKey size={20} />
            </div>
            <div>
              <span className="eyebrow">Secure connection</span>
              <Dialog.Title>Connect Lichess BOT</Dialog.Title>
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
              if (canSubmit) void onSubmit(token, engineId);
            }}
          >
            <div className="account-form">
              <label>
                <span>Lichess API token</span>
                {/* Explicit, because the hint below shares this label and
                    would otherwise become part of the field's name — the same
                    fix the runner panel's bearer field carries. */}
                <input
                  type="password"
                  autoFocus
                  value={token}
                  aria-label="Lichess API token"
                  aria-describedby="lichess-token-scope"
                  onChange={(event) => setToken(event.target.value)}
                  placeholder="lip_…"
                  autoComplete="off"
                />
                {/*
                 * All three, not just `bot:play`. This line used to name the
                 * playing scope alone, which is true of a bot that only ever
                 * answers challenges and wrong about every other thing the app
                 * offers — and it is the sentence an operator reads while
                 * ticking boxes on the Lichess page. The connect answers with
                 * what the pasted token actually carries; this is the half of
                 * the hint that arrives before there is anything to check.
                 *
                 * From `tokenScopes`, so the replace-token dialog's copy of
                 * this line cannot come to require a different set.
                 */}
                <small id="lichess-token-scope">{tokenScopeHint}</small>
              </label>
              <label>
                <span>Engine profile</span>
                <div className="select-wrap">
                  <select
                    value={engineId}
                    onChange={(event) => setEngineId(event.target.value)}
                  >
                    {engines.map((engine) => (
                      <option value={engine.id} key={engine.id}>
                        {engine.name}
                      </option>
                    ))}
                  </select>
                  <ChevronDown size={15} />
                </div>
              </label>
              <div className="credential-note">
                <FileKey size={17} />
                <p>
                  <strong>Stored in {storage.where}</strong>
                  <small>{storage.note}</small>
                  <small>
                    Disconnecting the account from the Bot fleet deletes it
                    again.
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
                disabled={!token.trim() || !engineId || pending}
              >
                {pending ? "Validating…" : "Validate & connect"}
              </Button>
            </div>
          </form>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
