import { useState } from "react";
import {
  Activity,
  Bot,
  FolderOpen,
  Gamepad2,
  Plus,
  Swords,
} from "lucide-react";
import type { PieceSetId } from "../ChessPiece";
import { tokenStorageCopy } from "../api/credentials";
import { AwaitingBackend } from "../components/AwaitingBackend";
import { BackendUnavailable } from "../components/BackendUnavailable";
import { StaleMark } from "../components/ConnectionBanner";
import { EmptyPage } from "../components/EmptyPage";
import { LiveGamePanel } from "../components/LiveGamePanel";
import { ReplaceTokenDialog } from "../components/ReplaceTokenDialog";
import { StatusDot } from "../components/StatusDot";
import type { BusyKeys } from "../hooks/useActionRunner";
import type { BoardThemeId } from "../lib/appearance";
import { countLiveGames } from "../lib/chess";
import { botStatusLabel } from "../lib/format";
import type { ConnectionState } from "../lib/connection";
import type { NavId } from "../lib/navigation";
import {
  scopeGapDetail,
  scopeGapHeadline,
  type TokenScopeGap,
} from "../lib/tokenScopes";
import {
  engineNameForGame,
  runtimeFor,
  type AccountProfile,
  type AppSnapshot,
  type LiveGame,
} from "../types";
import { Button, ConfirmDialog, RowMenu } from "../ui/primitives";

export function OverviewPage({
  snapshot,
  game,
  connectedCount,
  loading,
  busy,
  connection,
  stale = false,
  unavailable = false,
  awaitingBackend = false,
  remoteRunner = false,
  runnerUrl,
  tokenScopeGaps,
  onRetry,
  onAddEngine,
  onAddAccount,
  onChallenge,
  onToggle,
  onRemoveAccount,
  onReplaceToken,
  onAssignEngine,
  onNavigate,
  moveSoundsEnabled,
  onToggleMoveSounds,
  boardTheme,
  pieceSet,
  onBoardThemeChange,
  onPieceSetChange,
  onExportPgn,
}: {
  snapshot: AppSnapshot;
  game?: LiveGame;
  connectedCount: number;
  loading: boolean;
  busy: BusyKeys;
  connection?: ConnectionState;
  stale?: boolean;
  unavailable?: boolean;
  /**
   * A different backend is running and none of its data has arrived yet, so the
   * snapshot below is empty by design. Distinct from `unavailable`: the service
   * is answering, and the previous runner's games are still being played
   * somewhere — they are just not this app's to show.
   */
  awaitingBackend?: boolean;
  /** Games run on a remote runner, so engines live on that machine. */
  remoteRunner?: boolean;
  /**
   * Named in the disconnect confirmation, which has to say *where* the token
   * it deletes is stored — and in remote mode that is the runner, not this PC.
   */
  runnerUrl?: string | null;
  /**
   * Scope gaps recorded when each account's token was validated, keyed by
   * account id. Kept as a map rather than folded into `AccountProfile` because
   * scopes are not in the snapshot — Lichess reports them once, on the connect
   * call — so this is App's record, not the backend's.
   */
  tokenScopeGaps?: Readonly<Record<string, TokenScopeGap>>;
  onRetry?: () => void;
  onAddEngine: () => void;
  onAddAccount: () => void;
  onChallenge: () => void;
  onToggle: (account: AccountProfile) => void;
  onRemoveAccount?: (account: AccountProfile) => Promise<boolean>;
  /**
   * Swaps the account's stored Lichess token. Resolves false when the backend
   * refused it — a token for the wrong account, an unreachable runner — and the
   * dialog stays open on the paste that failed, because nothing was stored.
   */
  onReplaceToken?: (account: AccountProfile, token: string) => Promise<boolean>;
  onAssignEngine: (account: AccountProfile, engineId: string) => void;
  onNavigate: (page: NavId) => void;
  moveSoundsEnabled: boolean;
  onToggleMoveSounds: () => void;
  boardTheme: BoardThemeId;
  pieceSet: PieceSetId;
  onBoardThemeChange: (theme: BoardThemeId) => void;
  onPieceSetChange: (set: PieceSetId) => void;
  onExportPgn: (game: LiveGame) => void;
}) {
  const [removing, setRemoving] = useState<AccountProfile | null>(null);
  const [replacing, setReplacing] = useState<AccountProfile | null>(null);

  if (loading) {
    return (
      <div className="dashboard-content" aria-busy="true">
        <EmptyPage
          icon={<Activity />}
          title="Loading…"
          copy="Connecting to the QueenUI backend."
        />
      </div>
    );
  }

  /*
   * Order matters: an unreachable backend leaves the snapshot empty, and the
   * onboarding branch below reads an empty snapshot as a fresh install. This
   * check has to come first, or a dead remote runner is presented as
   * "First-run setup — put your engine in the chair."
   */
  if (unavailable) {
    return (
      <div className="dashboard-content">
        <BackendUnavailable
          detail={connection?.backendDetail ?? null}
          retrying={loading}
          onRetry={onRetry ?? (() => {})}
        />
      </div>
    );
  }

  /*
   * Same trap, one step further in: a backend generation change empties the
   * snapshot deliberately, and an empty snapshot still reads as a fresh install
   * here. Without this branch, switching to an unreachable runner would offer
   * "put your engine in the chair" to an operator whose fleet is playing on a
   * machine QueenUI cannot currently see.
   */
  if (awaitingBackend) {
    return (
      <div className="dashboard-content">
        <AwaitingBackend
          detail={connection?.detail ?? null}
          retrying={loading}
          onRetry={onRetry ?? (() => {})}
        />
      </div>
    );
  }

  if (snapshot.engines.length === 0) {
    return <Onboarding remoteRunner={remoteRunner} onAddEngine={onAddEngine} />;
  }

  /*
   * `isLiveGame`, not a bare `status === "started"`. Lichess reports a game as
   * `created` between the challenge being accepted and the first move, and
   * every other surface — the sidebar badge, the Games counter, the board that
   * is already on screen, the close guard, and `is_live` on the Rust side —
   * counts that as live. This strip alone said "0 live games" over a live
   * board, which is the one thing a status strip must not do.
   */
  const liveGameCount = countLiveGames(snapshot.games);
  const somethingLive = !stale && (connectedCount > 0 || liveGameCount > 0);

  return (
    <div className="dashboard-content">
      <section className="panel status-strip" role="status">
        <i
          className={`strip-dot ${somethingLive ? "live" : ""}`}
          aria-hidden="true"
        />
        <span>
          {connectedCount}/{snapshot.accounts.length} bot
          {snapshot.accounts.length === 1 ? "" : "s"} connected
        </span>
        <b>·</b>
        <span>
          {liveGameCount} live game{liveGameCount === 1 ? "" : "s"}
        </span>
        <b>·</b>
        <span>
          {snapshot.engines.length} engine
          {snapshot.engines.length === 1 ? "" : "s"}
        </span>
        <b>·</b>
        <span>
          {snapshot.accounts.length} account
          {snapshot.accounts.length === 1 ? "" : "s"}
        </span>
        {stale && <StaleMark label="Last known" />}
      </section>

      <section className={`content-grid ${game ? "has-live-game" : ""}`}>
        {game ? (
          <LiveGamePanel
            game={game}
            engineName={engineNameForGame(snapshot, game)}
            moveSoundsEnabled={moveSoundsEnabled}
            onToggleMoveSounds={onToggleMoveSounds}
            boardTheme={boardTheme}
            pieceSet={pieceSet}
            stale={stale}
            onBoardThemeChange={onBoardThemeChange}
            onPieceSetChange={onPieceSetChange}
            onExportPgn={onExportPgn}
          />
        ) : (
          <article className="panel no-live-game">
            <div className="empty-icon">
              <Gamepad2 />
            </div>
            <h2>No live game</h2>
            <p>
              Start a bot or create a challenge. A live board appears here as
              soon as Lichess starts the game.
            </p>
            <Button variant="primary" onClick={onChallenge}>
              <Swords size={17} />
              Create challenge
            </Button>
          </article>
        )}
      </section>

      <section className="panel fleet-panel">
        <div className="panel-heading">
          <div>
            <span className="eyebrow">Accounts</span>
            <h2>Bot fleet</h2>
          </div>
          <div className="fleet-heading-actions">
            {/* In remote mode this navigates to Engines, where the trusted
                engine browser lives — there is no local file to add. */}
            <Button variant="ghost" onClick={onAddEngine}>
              {remoteRunner ? <FolderOpen size={15} /> : <Bot size={15} />}
              {remoteRunner ? "Runner engines" : "Add engine"}
            </Button>
            <Button variant="secondary" onClick={onAddAccount}>
              <Plus size={15} />
              Connect bot
            </Button>
          </div>
        </div>
        {snapshot.accounts.length === 0 ? (
          <div className="fleet-empty">
            <p>No Lichess bot accounts yet.</p>
            <Button variant="secondary" onClick={onAddAccount}>
              Connect the first account
            </Button>
          </div>
        ) : (
          <div className="fleet-table">
            {snapshot.accounts.map((account) => {
              const runtime = runtimeFor(snapshot, account.id);
              const engine = snapshot.engines.find(
                (item) => item.id === account.engineId,
              );
              const stopped = runtime.status === "stopped";
              const scopeGap = tokenScopeGaps?.[account.id];
              return (
                <div className="fleet-row" key={account.id}>
                  <span className="avatar large-avatar">
                    {account.username[0]}
                  </span>
                  <div className="fleet-name">
                    <strong>{account.username}</strong>
                    <small>lichess.org/@/{account.username}</small>
                  </div>
                  <div>
                    <small>Status</small>
                    <span className="inline-status">
                      <StatusDot status={runtime.status} />
                      {botStatusLabel(runtime.status)}
                    </span>
                  </div>
                  <div>
                    <small>Rating</small>
                    <strong>{account.rating ?? "—"}</strong>
                  </div>
                  <div className="fleet-engine">
                    <small>Engine profile</small>
                    <select
                      aria-label={`Engine for ${account.username}`}
                      value={engine?.id ?? ""}
                      disabled={!stopped || busy.has(`assign-${account.id}`)}
                      onChange={(event) =>
                        onAssignEngine(account, event.target.value)
                      }
                    >
                      {snapshot.engines.map((item) => (
                        <option value={item.id} key={item.id}>
                          {item.name}
                        </option>
                      ))}
                    </select>
                  </div>
                  {/*
                   * The pending label used to be a bare "…", which is the one
                   * moment the control most needs a name: disabled, working,
                   * and announced as "horizontal ellipsis". Every other
                   * pending control in the app keeps its verb.
                   */}
                  <button
                    className={stopped ? "start-button" : "stop-button"}
                    disabled={busy.has(`bot-${account.id}`)}
                    onClick={() => onToggle(account)}
                  >
                    {busy.has(`bot-${account.id}`)
                      ? stopped
                        ? "Starting…"
                        : "Stopping…"
                      : stopped
                        ? "Start"
                        : "Stop"}
                  </button>
                  <RowMenu
                    label={`Actions for ${account.username}`}
                    items={[
                      {
                        key: "configure",
                        label: "Configure engine",
                        onSelect: () => onNavigate("Engines"),
                      },
                      /*
                       * Deliberately available while the bot is running, unlike
                       * the two entries around it. Replacing the token writes
                       * the secret and nothing else — the games in flight hold
                       * the client they started with — and a token that has
                       * just been revoked is exactly the case where stopping
                       * the fleet first is the wrong instruction.
                       */
                      ...(onReplaceToken
                        ? [
                            {
                              key: "replace-token",
                              label: "Replace token…",
                              disabled: busy.has(`replace-token-${account.id}`),
                              onSelect: () => setReplacing(account),
                            },
                          ]
                        : []),
                      ...(onRemoveAccount
                        ? [
                            {
                              key: "disconnect",
                              label: "Disconnect account…",
                              danger: true,
                              // A running bot is mid-game by definition;
                              // stopping first is the rule the engine
                              // selector already follows.
                              disabled:
                                !stopped ||
                                busy.has(`remove-account-${account.id}`),
                              hint: stopped ? undefined : "Stop the bot first",
                              onSelect: () => setRemoving(account),
                            },
                          ]
                        : []),
                    ]}
                  />
                  {/* Arrives by snapshot push while the operator may be
                      looking elsewhere, so it is announced and framed. */}
                  {runtime.error && (
                    <p className="runtime-error" role="alert">
                      <strong>{account.username} reported a problem</strong>{" "}
                      {runtime.error}
                    </p>
                  )}
                  {/*
                   * The connect-time scope verdict, kept on the card for as
                   * long as the token lacks the scopes. A toast was not enough:
                   * the fact it reports is permanent until someone mints a new
                   * token, and the incident that produced this notice was a
                   * gap nobody saw until matchmaking answered 403 hours later.
                   *
                   * The blocking grade (`bot:play` absent) is an alert; the
                   * matchmaking grade is a status, because the bot really does
                   * play — it just cannot find opponents on its own.
                   */}
                  {scopeGap && (
                    <p
                      className={`scope-gap${scopeGap.canPlayGames ? "" : " scope-gap-blocking"}`}
                      role={scopeGap.canPlayGames ? "status" : "alert"}
                    >
                      <strong>
                        {scopeGapHeadline(scopeGap, account.username)}
                      </strong>{" "}
                      {scopeGapDetail(scopeGap)}
                    </p>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </section>

      {/*
       * Mounted only while it is open, so the pasted token lives exactly as
       * long as the dialog does: cancelling it discards the field rather than
       * leaving a secret in a hidden component's state.
       */}
      {replacing && onReplaceToken && (
        <ReplaceTokenDialog
          account={replacing}
          pending={busy.has(`replace-token-${replacing.id}`)}
          remoteRunner={remoteRunner}
          runnerUrl={runnerUrl}
          onClose={() => setReplacing(null)}
          onSubmit={(token) => {
            const account = replacing;
            /*
             * Closed only on success. A refusal means nothing was stored — the
             * token belongs to another account, or the runner is too old for
             * the command — and closing on it would leave the account on its
             * old token behind a dialog that looked like it had done the job.
             */
            void onReplaceToken(account, token).then((succeeded) => {
              if (succeeded) setReplacing(null);
            });
          }}
        />
      )}

      {/*
       * Removing an account deletes its Lichess token. There was no way to do
       * that at all — the backend command existed end to end with no frontend
       * surface — so a token QueenUI wrote could never be taken back.
       *
       * The description names the machine the token is deleted *from*, via the
       * same `tokenStorageCopy` the account dialog uses to say where it was
       * put. It used to describe the deletion with no location at all, which
       * in remote mode invited the reading that a secret sitting on the runner
       * was being cleaned off this PC.
       */}
      <ConfirmDialog
        open={removing !== null}
        title={`Disconnect ${removing?.username ?? ""}?`}
        description={`QueenUI deletes the stored Lichess token from ${tokenStorageCopy(remoteRunner, runnerUrl).where} and forgets the account. Games already played stay in the scorebook; nothing is changed on Lichess.`}
        confirmLabel="Disconnect and delete token"
        pending={removing ? busy.has(`remove-account-${removing.id}`) : false}
        onCancel={() => setRemoving(null)}
        onConfirm={() => {
          const account = removing;
          if (!account || !onRemoveAccount) return;
          void onRemoveAccount(account).then((succeeded) => {
            if (succeeded) setRemoving(null);
          });
        }}
      />
    </div>
  );
}

/**
 * The first-run screen.
 *
 * Every sentence here names a machine, so the copy has to follow the runner.
 * The screen used to say "Windows UCI engine", "launched locally" and "Nothing
 * is uploaded" whatever the runner was; the remote branch that replaced them
 * then described an upload the app no longer performs. Trusted-engine mode
 * refuses `/v2/engines/upload` and arbitrary-path registration outright, and
 * the only remote add flow the frontend has is `RunnerEngineBrowser` — pick an
 * executable below a root the runner's administrator configured, which the
 * runner copies into its own content-addressed store. The remote copy names
 * that flow, and no longer names the runner's operating system: QueenUI reads
 * that from the runner (Settings shows it) rather than assuming it.
 *
 * The remote button navigates to Engines, where the browser lives, so it says
 * where it goes; the notice App raises reports the move itself.
 */
export function Onboarding({
  remoteRunner = false,
  onAddEngine,
}: {
  remoteRunner?: boolean;
  onAddEngine: () => void;
}) {
  return (
    <section className="onboarding">
      <span className="onboarding-kicker">First-run setup</span>
      <h2>Put your engine in the chair.</h2>
      <p>
        {remoteRunner
          ? "QueenUI needs a UCI engine on the runner before it can connect a bot account and play. You pick one from the engine roots the runner's administrator configured, and the runner launches and validates it there before it is saved."
          : "QueenUI needs a Windows UCI engine before it can connect a bot account and play. The executable is launched locally and validated before it is saved."}
      </p>
      <div className="onboarding-flow">
        <div className="active">
          <span>1</span>
          <strong>Add engine</strong>
          <small>
            {remoteRunner
              ? "Pick one from a configured runner root"
              : "Select and probe an .exe"}
          </small>
        </div>
        <i>→</i>
        <div>
          <span>2</span>
          <strong>Connect Lichess</strong>
          <small>
            {remoteRunner
              ? "Secure a BOT token, stored by the runner"
              : "Secure a BOT token"}
          </small>
        </div>
        <i>→</i>
        <div>
          <span>3</span>
          <strong>Challenge</strong>
          <small>Start playing</small>
        </div>
      </div>
      <Button variant="primary" className="h-[42px] px-5" onClick={onAddEngine}>
        {remoteRunner ? <FolderOpen size={18} /> : <Plus size={18} />}
        {remoteRunner
          ? "Browse engines on the runner"
          : "Choose UCI engine executable"}
      </Button>
      <small className="security-note">
        {remoteRunner
          ? "The engine runs on the runner. QueenUI copies the executable you pick into the runner's own store and probes it there; nothing is sent from this PC."
          : "Engine probing occurs on this PC. Nothing is uploaded."}
      </small>
    </section>
  );
}
