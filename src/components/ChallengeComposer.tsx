import { useMemo, useState } from "react";
import { ChevronDown, CircleDot, Search, Swords } from "lucide-react";
import { botStatusLabel } from "../lib/format";
import {
  defaultSelectedTimeControl,
  timeControlValue,
} from "../lib/timeControls";
import type {
  AccountProfile,
  BotRuntime,
  ChallengeRequest,
  TimeControl,
} from "../types";
import { Button, Dialog } from "../ui/primitives";
import { StatusDot } from "./StatusDot";
import { TimeControlPresets } from "./TimeControlPresets";

export function ChallengeComposer({
  accounts,
  runtimes,
  timeControls,
  pending,
  onClose,
  onSubmit,
}: {
  accounts: AccountProfile[];
  runtimes: BotRuntime[];
  timeControls: TimeControl[];
  pending: boolean;
  onClose: () => void;
  onSubmit: (request: ChallengeRequest) => Promise<void>;
}) {
  const [opponent, setOpponent] = useState("");
  const [accountId, setAccountId] = useState(accounts[0]?.id ?? "");
  const [clock, setClock] = useState(() =>
    timeControlValue(defaultSelectedTimeControl(timeControls)),
  );
  const [rated, setRated] = useState(true);
  const [color, setColor] = useState("random");
  const [limit, increment] = useMemo(
    () => clock.split("+").map(Number),
    [clock],
  );
  const selected = accounts.find((account) => account.id === accountId);
  const runtime = runtimes.find((item) => item.accountId === accountId);
  const canSubmit = Boolean(opponent.trim()) && !pending;
  return (
    <Dialog.Root
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-20 bg-black/70 backdrop-blur-[7px]" />
        <Dialog.Content className="challenge-modal fixed left-1/2 top-1/2 z-[21] -translate-x-1/2 -translate-y-1/2 focus:outline-none">
          <div className="modal-head">
            <div className="modal-icon">
              <Swords size={20} />
            </div>
            <div>
              <span className="eyebrow">Lichess Bot API</span>
              <Dialog.Title>Create a challenge</Dialog.Title>
              <Dialog.Description>
                The selected account is connected before the challenge is sent.
              </Dialog.Description>
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
              if (!canSubmit) return;
              void onSubmit({
                accountId,
                opponent: opponent.trim(),
                clockLimit: limit * 60,
                clockIncrement: increment,
                rated,
                color,
                variant: "standard",
              });
            }}
          >
            <div className="modal-body">
              <div className="form-section">
                <div className="form-section-title">
                  <span>1</span>
                  <div>
                    <strong>Players</strong>
                    <small>Select your bot and its opponent</small>
                  </div>
                </div>
                <div className="form-grid">
                  <label>
                    <span>Play as</span>
                    <div className="select-wrap">
                      <select
                        value={accountId}
                        onChange={(event) => setAccountId(event.target.value)}
                      >
                        {accounts.map((account) => (
                          <option value={account.id} key={account.id}>
                            {account.username}
                          </option>
                        ))}
                      </select>
                      <ChevronDown size={15} />
                    </div>
                  </label>
                  <label>
                    <span>Opponent</span>
                    <div className="input-wrap">
                      <Search size={15} />
                      <input
                        autoFocus
                        value={opponent}
                        onChange={(event) => setOpponent(event.target.value)}
                        placeholder="Lichess username"
                      />
                    </div>
                  </label>
                </div>
                <div className="account-summary">
                  <span className="avatar">{selected?.username[0]}</span>
                  <p>
                    <strong>{selected?.username}</strong>
                    {/* One status, one word: this line used to print the
                        raw value while the badge four lines down called the
                        same value "Connected". */}
                    <small>
                      BOT account · {botStatusLabel(runtime?.status)}
                    </small>
                  </p>
                  <i>
                    <StatusDot status={runtime?.status ?? "stopped"} />
                    {runtime?.status === "online"
                      ? botStatusLabel(runtime.status)
                      : "Auto-connect"}
                  </i>
                </div>
              </div>
              <div className="form-section">
                <div className="form-section-title">
                  <span>2</span>
                  <div>
                    <strong>Game setup</strong>
                    <small>Configure the challenge conditions</small>
                  </div>
                </div>
                <div className="full-label">
                  <span>Time control</span>
                  <TimeControlPresets
                    controls={timeControls}
                    selected={clock}
                    onSelect={setClock}
                  />
                </div>
                <div className="form-grid triple-grid">
                  <label>
                    <span>Variant</span>
                    <div className="select-wrap">
                      <select value="standard" disabled>
                        <option value="standard">Standard</option>
                      </select>
                      <ChevronDown size={15} />
                    </div>
                  </label>
                  <div className="button-field">
                    <span>Play as</span>
                    <div className="segmented">
                      {["white", "random", "black"].map((item) => (
                        <button
                          type="button"
                          className={color === item ? "selected" : ""}
                          aria-pressed={color === item}
                          key={item}
                          onClick={() => setColor(item)}
                        >
                          {item}
                        </button>
                      ))}
                    </div>
                  </div>
                  <div className="button-field">
                    <span>Mode</span>
                    <div className="segmented">
                      {[true, false].map((item) => (
                        <button
                          type="button"
                          className={rated === item ? "selected" : ""}
                          aria-pressed={rated === item}
                          key={String(item)}
                          onClick={() => setRated(item)}
                        >
                          {item ? "Rated" : "Casual"}
                        </button>
                      ))}
                    </div>
                  </div>
                </div>
              </div>
              <div className="challenge-summary">
                <span>
                  <CircleDot size={16} />
                </span>
                <p>
                  <strong>
                    Ready to challenge {opponent || "an opponent"}
                  </strong>
                  <small>
                    {clock} · Standard · {rated ? "Rated" : "Casual"} · {color}{" "}
                    color
                  </small>
                </p>
                <em>via {selected?.username}</em>
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
                disabled={!opponent.trim() || pending}
              >
                <Swords size={17} />
                {pending ? "Connecting & sending…" : "Send challenge"}
              </Button>
            </div>
          </form>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
