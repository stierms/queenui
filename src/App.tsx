import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { open, save } from "@tauri-apps/plugin-dialog";
import { Swords } from "lucide-react";
import * as commands from "./api/commands";
import * as credentials from "./api/credentials";
import { onCloseRequested } from "./api/events";
import type { PieceSetId } from "./ChessPiece";
import { AccountComposer } from "./components/AccountComposer";
import { ChallengeComposer } from "./components/ChallengeComposer";
import { CloseGuard } from "./components/CloseGuard";
import { ConnectionBanner } from "./components/ConnectionBanner";
import { Sidebar } from "./components/Sidebar";
import { Toast } from "./components/Toast";
import {
  hasPreviewParam,
  logsPreviewSource,
  previewState,
} from "./dev/preview";
import { useActionRunner } from "./hooks/useActionRunner";
import { useGamesInDisplayOrder } from "./hooks/useGamesInDisplayOrder";
import { useMoveSounds } from "./hooks/useMoveSounds";
import { useNotices } from "./hooks/useNotices";
import { useRunnerSettings } from "./hooks/useRunnerSettings";
import { useSnapshot } from "./hooks/useSnapshot";
import {
  storedBoardTheme,
  storedPieceSet,
  type BoardThemeId,
} from "./lib/appearance";
import { resumeMoveAudio } from "./lib/audio";
import { countLiveGames, liveGamesOnly, pgnForGame } from "./lib/chess";
import { pickPath } from "./lib/fileDialog";
import type { NavId } from "./lib/navigation";
import { storedTimeControls, timeControlsStorageKey } from "./lib/timeControls";
import {
  scopeGapNotice,
  storedScopeGaps,
  tokenScopeGap,
  tokenScopeStorageKey,
  type TokenScopeGap,
} from "./lib/tokenScopes";
import { ChallengesPage } from "./pages/ChallengesPage";
import { EnginesPage } from "./pages/EnginesPage";
import { GamesPage } from "./pages/GamesPage";
import { LogsPage } from "./pages/LogsPage";
import { OverviewPage } from "./pages/OverviewPage";
import { ScorebookPage } from "./pages/ScorebookPage";
import { SettingsPage } from "./pages/SettingsPage";
import {
  emptySnapshot,
  runtimeFor,
  type AccountProfile,
  type AddAccountResult,
  type LiveGame,
  type TimeControl,
} from "./types";
import { Button } from "./ui/primitives";
import "./App.css";

const CLOSE_KEY = "confirm-close";

function App() {
  const {
    gamesPreview,
    logsPreview,
    enginesPreview,
    presentationPreview,
    previewSnapshot,
  } = previewState();
  const [activeNav, setActiveNav] = useState<NavId>(
    gamesPreview
      ? "Games"
      : logsPreview
        ? "Logs"
        : enginesPreview
          ? "Engines"
          : "Overview",
  );
  const { notice, showNotice, dismissNotice } = useNotices();
  const {
    snapshot,
    loading,
    connection,
    stale,
    unavailable,
    awaitingBackend,
    retry,
  } = useSnapshot(
    !presentationPreview,
    presentationPreview ? previewSnapshot : emptySnapshot,
    () =>
      showNotice(
        "error",
        "Can't reach the QueenUI backend service — restart the app and try again.",
      ),
  );
  const { busy, runAction } = useActionRunner(showNotice);
  const [challengeOpen, setChallengeOpen] = useState(false);
  const [accountOpen, setAccountOpen] = useState(false);
  // `?close-preview` raises the guard without a real close event, which a
  // browser has no way to deliver.
  const [closeRequested, setCloseRequested] = useState(() =>
    hasPreviewParam("close-preview"),
  );
  // How many games the backend counted when it blocked the close. The snapshot
  // can be momentarily behind that count, which is why the event carries it.
  const [closeRequestedGames, setCloseRequestedGames] = useState(0);
  const challengeReturnFocus = useRef<HTMLElement | null>(null);
  const accountReturnFocus = useRef<HTMLElement | null>(null);
  const [moveSoundsEnabled, setMoveSoundsEnabled] = useState(
    () => localStorage.getItem("queenui-move-sounds") !== "off",
  );
  const [boardTheme, setBoardTheme] = useState<BoardThemeId>(storedBoardTheme);
  const [pieceSet, setPieceSet] = useState<PieceSetId>(storedPieceSet);
  const {
    settings: runnerSettings,
    error: runnerSettingsError,
    setSettings: setRunnerSettings,
  } = useRunnerSettings(!presentationPreview);
  const [timeControls, setTimeControls] =
    useState<TimeControl[]>(storedTimeControls);
  /*
   * What each account's stored Lichess token turned out to be able to do,
   * learned on the connect call and nowhere else — the snapshot has no scope
   * field to read it back from. Seeded from disk so the warning outlives a
   * restart: the token is still short a scope in the morning.
   */
  const [scopeGaps, setScopeGaps] =
    useState<Record<string, TokenScopeGap>>(storedScopeGaps);
  const gamesInDisplayOrder = useGamesInDisplayOrder(snapshot.games);
  const displaySnapshot = useMemo(
    () => ({ ...snapshot, games: gamesInDisplayOrder }),
    [snapshot, gamesInDisplayOrder],
  );
  const liveGames = liveGamesOnly(gamesInDisplayOrder);
  /*
   * The number every surface quotes, from one definition in `countLiveGames`;
   * `liveGames` stays the *list* the boards and the close guard render.
   *
   * Settings used to be quoted this number too, to ask before a switch that
   * would take a remote runner's live games off this screen. It no longer is:
   * that question is the backend's, which counts the runner's games by asking
   * the runner (`verify_remote_handover`) instead of trusting a snapshot this
   * app may hold a stale copy of — and refuses the save until the answer is
   * acknowledged.
   */
  const liveGameCount = countLiveGames(snapshot.games);
  const selectedGame = liveGames[0];
  const remoteRunner = runnerSettings?.activeMode === "remote";
  const connectedCount = snapshot.accounts.filter(
    (account) => runtimeFor(snapshot, account.id).status !== "stopped",
  ).length;
  const activeCampaigns = snapshot.campaignRuntimes.filter(
    (campaign) => campaign.status !== "stopped",
  ).length;
  const challengeReady =
    snapshot.engines.length > 0 && snapshot.accounts.length > 0;
  useMoveSounds(snapshot.games, moveSoundsEnabled);

  // The runner target decides which machine an engine path refers to, so a
  // failure to read it is a correctness problem, not a cosmetic one.
  useEffect(() => {
    if (runnerSettingsError) {
      showNotice(
        "error",
        `Could not read the runner settings (${runnerSettingsError}). QueenUI is assuming this computer.`,
      );
    }
  }, [runnerSettingsError, showNotice]);

  useEffect(() => {
    localStorage.setItem(
      "queenui-move-sounds",
      moveSoundsEnabled ? "on" : "off",
    );
  }, [moveSoundsEnabled]);

  useEffect(() => {
    localStorage.setItem("queenui-board-theme", boardTheme);
    localStorage.setItem("queenui-piece-set", pieceSet);
  }, [boardTheme, pieceSet]);

  useEffect(() => {
    localStorage.setItem(timeControlsStorageKey, JSON.stringify(timeControls));
  }, [timeControls]);

  useEffect(() => {
    localStorage.setItem(tokenScopeStorageKey, JSON.stringify(scopeGaps));
  }, [scopeGaps]);

  /*
   * One entry per account whose token is short a scope; a `null` gap deletes
   * the entry, which is how a reconnect with a complete token — or a
   * disconnect — takes the warning off the card.
   */
  const recordScopeGap = useCallback(
    (accountId: string, gap: TokenScopeGap | null) => {
      setScopeGaps((current) => {
        if (!gap) {
          if (!(accountId in current)) return current;
          const next = { ...current };
          delete next[accountId];
          return next;
        }
        return { ...current, [accountId]: gap };
      });
    },
    [],
  );

  /*
   * What a validated token is announced as, and what is remembered about it.
   *
   * Both commands that ever see a token's OAuth scopes answer with the same
   * envelope — the connect and the in-place replacement — and both run through
   * here, so the record and the receipt cannot come apart. A replacement that
   * fixes a gap therefore takes the notice off the account card, and one that
   * introduces a gap raises it: the stored gap describes the token the account
   * currently holds, and after a replacement that is the token just pasted.
   *
   * `success` is reached only when the token carries the whole required set. A
   * gap is never announced as a receipt — "connected securely" over a token
   * that cannot run matchmaking is the sentence that started all of this.
   */
  const announceTokenVerdict = useCallback(
    (result: AddAccountResult, success: string) => {
      const gap = tokenScopeGap(result);
      recordScopeGap(result.account.id, gap);
      if (!gap) {
        showNotice("success", success);
        return;
      }
      const notice = scopeGapNotice(gap, result.account.username);
      showNotice(notice.kind, notice.message);
    },
    [recordScopeGap, showNotice],
  );

  // The backend blocks the close and asks only when games are in progress, so
  // a quit with nothing running is never interrupted. The payload is the
  // backend's own count, which the snapshot may briefly trail.
  useEffect(
    () =>
      onCloseRequested((liveGameCount) => {
        setCloseRequestedGames(liveGameCount);
        setCloseRequested(true);
      }),
    [],
  );

  /*
   * Retire the guard when the games it warned about have all finished.
   *
   * The flag used to clear only when the operator pressed "Keep playing". If
   * the last live game ended while the dialog was open — thirty seconds of
   * hesitation in a bullet game — the dialog unmounted, the window stayed
   * open, and the flag stayed true forever, so the next game to start raised
   * an "abandon your games" dialog nobody asked for, over a live board.
   *
   * The ref makes this precise: only clear once the guard has actually seen
   * live games, so a request that arrives before the snapshot catches up is
   * not swallowed.
   */
  const guardSawGames = useRef(false);
  useEffect(() => {
    if (!closeRequested) {
      guardSawGames.current = false;
      return;
    }
    if (liveGames.length > 0) {
      guardSawGames.current = true;
      return;
    }
    if (guardSawGames.current) setCloseRequested(false);
  }, [closeRequested, liveGames.length]);

  useEffect(() => {
    const unlockAudio = () => {
      if (moveSoundsEnabled) resumeMoveAudio();
    };
    window.addEventListener("pointerdown", unlockAudio, { once: true });
    return () => window.removeEventListener("pointerdown", unlockAudio);
  }, [moveSoundsEnabled]);

  async function addEngine() {
    const path = await pickPath(
      () =>
        open({
          multiple: false,
          filters: [{ name: "Windows chess engine", extensions: ["exe"] }],
        }),
      showNotice,
      "choose an engine executable",
    );
    if (!path) return;
    await runAction(
      "add-engine",
      () => commands.addEngine(path),
      "Engine validated and added",
      "add the engine",
    );
  }

  function openAccountComposer() {
    if (!snapshot.engines.length) {
      setActiveNav("Engines");
      showNotice(
        "error",
        "Add a UCI engine before connecting a Lichess bot account.",
      );
      return;
    }
    accountReturnFocus.current =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    setAccountOpen(true);
  }

  function closeAccountComposer() {
    setAccountOpen(false);
    window.setTimeout(() => accountReturnFocus.current?.focus(), 0);
  }

  function openChallengeComposer() {
    if (!snapshot.accounts.length) {
      /*
       * Two different prerequisites, two different destinations — and, until
       * now, one message for both: the no-engine branch navigated to Engines
       * while telling the operator to connect a Lichess account, which is the
       * step it was not taking them to.
       */
      if (!snapshot.engines.length) {
        setActiveNav("Engines");
        showNotice(
          "error",
          "Add a UCI engine first — a challenge needs an engine and a Lichess bot account.",
        );
        return;
      }
      showNotice(
        "error",
        "Connect a Lichess BOT account before creating a challenge.",
      );
      openAccountComposer();
      return;
    }
    challengeReturnFocus.current =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    setChallengeOpen(true);
  }

  function closeChallengeComposer() {
    setChallengeOpen(false);
    window.setTimeout(() => challengeReturnFocus.current?.focus(), 0);
  }

  async function toggleBot(account: AccountProfile) {
    const runtime = runtimeFor(snapshot, account.id);
    const stopping = runtime.status !== "stopped";
    await runAction(
      `bot-${account.id}`,
      () =>
        stopping ? commands.stopBot(account.id) : commands.startBot(account.id),
      stopping
        ? `${account.username} stopped`
        : `${account.username} is connecting`,
      stopping ? `stop ${account.username}` : `start ${account.username}`,
    );
  }

  const toggleMoveSounds = useCallback(() => {
    setMoveSoundsEnabled((enabled) => {
      const next = !enabled;
      if (next) resumeMoveAudio();
      return next;
    });
  }, []);

  const exportPgn = useCallback(
    async (game: LiveGame) => {
      const safeOpponent = game.opponent.replace(/[^a-zA-Z0-9_-]+/g, "_");
      const path = await pickPath(
        () =>
          save({
            defaultPath: `${game.botUsername}_vs_${safeOpponent}_${game.id}.pgn`,
            filters: [{ name: "Portable Game Notation", extensions: ["pgn"] }],
          }),
        showNotice,
        "save the PGN",
      );
      if (!path) return;
      await runAction(
        `export-${game.id}`,
        () => commands.writePgnFile(path, pgnForGame(game)),
        "PGN exported successfully",
        "write the PGN file",
      );
    },
    [runAction, showNotice],
  );

  const onExportPgn = useCallback(
    (game: LiveGame) => void exportPgn(game),
    [exportPgn],
  );

  /*
   * One entry per NavId. A `Record` keyed on the union replaces the seven-deep
   * ternary whose `else` branch rendered the Logs page, so a renamed page is a
   * compile error rather than a silently wrong screen.
   */
  const pages: Record<NavId, () => ReactNode> = {
    Overview: () => (
      <OverviewPage
        snapshot={displaySnapshot}
        game={selectedGame}
        connectedCount={connectedCount}
        loading={loading}
        busy={busy}
        connection={connection}
        stale={stale}
        unavailable={unavailable}
        awaitingBackend={awaitingBackend}
        remoteRunner={remoteRunner}
        runnerUrl={runnerSettings?.url}
        onRetry={retry}
        onAddEngine={() => {
          if (remoteRunner) {
            setActiveNav("Engines");
            /*
             * Reports what just happened rather than dressing an instruction
             * as a completed action — it renders with the success glyph. It
             * used to offer an upload: trusted-engine mode refuses
             * `/v2/engines/upload`, and browsing an administrator-configured
             * root is the only remote add flow there is.
             */
            showNotice(
              "success",
              "Opened Engines — pick one from a root the runner's administrator configured.",
            );
          } else {
            void addEngine();
          }
        }}
        onAddAccount={openAccountComposer}
        onChallenge={openChallengeComposer}
        onToggle={(account) => void toggleBot(account)}
        tokenScopeGaps={scopeGaps}
        onRemoveAccount={async (account) => {
          const succeeded = await runAction(
            `remove-account-${account.id}`,
            () => credentials.removeLichessAccount(account.id),
            `${account.username} disconnected and its token deleted`,
            `disconnect ${account.username}`,
          );
          // The token is gone, so the verdict about it is too. A refused
          // removal leaves both where they were.
          if (succeeded) recordScopeGap(account.id, null);
          return succeeded;
        }}
        /*
         * The repair that keeps the account. Until this existed, a revoked or
         * under-scoped token could only be fixed by disconnecting and
         * connecting again, which deletes the secret, drops the campaign, and
         * rebuilds the profile from the connect dialog's engine picker — an
         * operator fixing a token lost settings for it.
         *
         * No success line from `runAction`: what to say depends on the scopes
         * the replacement turned out to carry, exactly as on the connect.
         */
        onReplaceToken={async (account, token) => {
          let result: AddAccountResult | undefined;
          const succeeded = await runAction(
            `replace-token-${account.id}`,
            async () => {
              result = await commands.updateLichessAccountToken(
                account.id,
                token,
              );
            },
            undefined,
            `replace ${account.username}'s token`,
          );
          if (!succeeded || !result) return false;
          announceTokenVerdict(
            result,
            `${account.username}'s token replaced — games and matchmaking already running keep the old token; the new one is used from the next start.`,
          );
          return true;
        }}
        onAssignEngine={(account, engineId) =>
          void runAction(
            `assign-${account.id}`,
            () => commands.updateAccountEngine(account.id, engineId),
            `${account.username} will use ${snapshot.engines.find((engine) => engine.id === engineId)?.name ?? "the selected engine"}`,
            `assign an engine to ${account.username}`,
          )
        }
        onNavigate={setActiveNav}
        moveSoundsEnabled={moveSoundsEnabled}
        onToggleMoveSounds={toggleMoveSounds}
        boardTheme={boardTheme}
        pieceSet={pieceSet}
        onBoardThemeChange={setBoardTheme}
        onPieceSetChange={setPieceSet}
        onExportPgn={onExportPgn}
      />
    ),
    Engines: () => (
      <EnginesPage
        snapshot={snapshot}
        busy={busy}
        remoteRunner={remoteRunner}
        showNotice={showNotice}
        onAdd={() => void addEngine()}
        onRegister={(rootId, relativePath) =>
          runAction(
            "register-engine",
            () => commands.registerEngine(rootId, relativePath),
            "Engine copied, validated, and registered",
            "register the engine",
          )
        }
        onRemove={(engine) =>
          void runAction(
            `engine-${engine.id}`,
            () => commands.removeEngine(engine.id),
            `${engine.name} removed`,
            `remove ${engine.name}`,
          )
        }
        onSaveOptions={(engine, options) =>
          runAction(
            `options-${engine.id}`,
            () => commands.updateEngineOptions(engine.id, options),
            `${engine.name} UCI options saved`,
            `save the UCI options for ${engine.name}`,
          )
        }
        onRefreshOptions={(engine) =>
          runAction(
            `refresh-options-${engine.id}`,
            () => commands.refreshEngineOptions(engine.id),
            `${engine.name} UCI options refreshed`,
            `re-probe ${engine.name}`,
          )
        }
        onSaveBook={(engine, book) =>
          runAction(
            `book-${engine.id}`,
            () => commands.configureOpeningBook(engine.id, book),
            `${engine.name} opening book saved`,
            `save the opening book for ${engine.name}`,
          )
        }
        onClearBook={(engine) =>
          runAction(
            `book-clear-${engine.id}`,
            () => commands.clearEngineOpeningBook(engine.id),
            `${engine.name} opening book removed`,
            `remove the opening book from ${engine.name}`,
          )
        }
      />
    ),
    Games: () => (
      <GamesPage
        snapshot={displaySnapshot}
        busy={busy}
        /*
         * The failure text is the whole value of a retained game error, so the
         * dismissal has to be able to fail out loud: an older runner answers
         * `dismissGameError` with the "update queen-runner" refusal, and a card
         * that disappeared on a refused call would destroy the only copy of
         * that text the operator had. `runAction` keeps it on screen and says
         * why; the snapshot removes the card when the backend really did.
         */
        onDismissGameError={(game) =>
          void runAction(
            `dismiss-game-${game.id}`,
            () => commands.dismissGameError(game.id),
            "Game error dismissed",
            `dismiss the error for game ${game.id}`,
          )
        }
        moveSoundsEnabled={moveSoundsEnabled}
        onToggleMoveSounds={toggleMoveSounds}
        boardTheme={boardTheme}
        pieceSet={pieceSet}
        stale={stale}
        onBoardThemeChange={setBoardTheme}
        onPieceSetChange={setPieceSet}
        onExportPgn={onExportPgn}
      />
    ),
    Scorebook: () => (
      <ScorebookPage
        busy={busy}
        runAction={runAction}
        showNotice={showNotice}
      />
    ),
    Challenges: () => (
      <ChallengesPage
        snapshot={snapshot}
        timeControls={timeControls}
        busy={busy}
        onDirectChallenge={openChallengeComposer}
        onStart={(settings) =>
          runAction(
            `campaign-${settings.accountId}`,
            () => commands.startCampaign(settings),
            "Automatic matchmaking started",
            "start automatic matchmaking",
          )
        }
        onStop={(accountId) =>
          runAction(
            `campaign-${accountId}`,
            () => commands.stopCampaign(accountId),
            "Matchmaking stopped; active games will finish",
            "stop matchmaking",
          )
        }
      />
    ),
    Settings: () => (
      <SettingsPage
        boardTheme={boardTheme}
        pieceSet={pieceSet}
        moveSoundsEnabled={moveSoundsEnabled}
        onBoardThemeChange={setBoardTheme}
        onPieceSetChange={setPieceSet}
        onToggleMoveSounds={toggleMoveSounds}
        timeControls={timeControls}
        onTimeControlsChange={setTimeControls}
        runnerSettings={runnerSettings}
        runnerSettingsError={runnerSettingsError}
        onRunnerSettingsChange={setRunnerSettings}
      />
    ),
    Logs: () => (
      <LogsPage
        snapshot={snapshot}
        busy={busy}
        runAction={runAction}
        showNotice={showNotice}
        source={logsPreview ? logsPreviewSource() : undefined}
      />
    ),
  };

  return (
    <div className="app-shell">
      <Sidebar
        snapshot={snapshot}
        activeNav={activeNav}
        liveGameCount={liveGameCount}
        activeCampaigns={activeCampaigns}
        stale={stale}
        onNavigate={setActiveNav}
        onAddAccount={openAccountComposer}
      />

      <main className="workspace">
        <header className="topbar">
          <div>
            <h1>{activeNav}</h1>
            {activeNav === "Overview" && (
              <p>
                {snapshot.accounts.length} bot account
                {snapshot.accounts.length === 1 ? "" : "s"} ·{" "}
                {snapshot.engines.length} engine profile
                {snapshot.engines.length === 1 ? "" : "s"}
              </p>
            )}
          </div>
          <div className="topbar-actions">
            <Button
              variant="primary"
              className="new-challenge-button"
              disabled={!challengeReady}
              // A `title` on a disabled button's wrapper is invisible to the
              // keyboard; the reason lives on the button itself so focus and
              // assistive technology both reach it.
              aria-describedby={
                challengeReady ? undefined : "new-challenge-hint"
              }
              onClick={openChallengeComposer}
            >
              <Swords size={17} />
              New challenge
            </Button>
            {!challengeReady && (
              <span id="new-challenge-hint" className="topbar-hint">
                Add a UCI engine and connect a Lichess bot account first
              </span>
            )}
          </div>
        </header>
        <ConnectionBanner connection={connection} onRetry={retry} />
        {pages[activeNav]()}
      </main>

      {challengeOpen && (
        <ChallengeComposer
          accounts={snapshot.accounts}
          runtimes={snapshot.runtimes}
          timeControls={timeControls}
          pending={busy.has("challenge")}
          onClose={closeChallengeComposer}
          onSubmit={async (request) => {
            const succeeded = await runAction(
              "challenge",
              () => commands.createChallenge(request),
              `Challenge sent to ${request.opponent}`,
              `challenge ${request.opponent}`,
            );
            if (succeeded) closeChallengeComposer();
          }}
        />
      )}
      {accountOpen && (
        <AccountComposer
          engines={snapshot.engines}
          pending={busy.has("account")}
          remoteRunner={remoteRunner}
          runnerUrl={runnerSettings?.url}
          onClose={closeAccountComposer}
          /*
           * The connect answers with the token's OAuth scopes, and this is the
           * only moment QueenUI ever sees them. Three outcomes, three
           * receipts — a play-only token used to get the full-capability one,
           * and the operator found out from a 403 in the middle of a campaign.
           *
           * The account is stored in all three cases (the backend writes it
           * before it looks at scopes), so the dialog closes in all three: a
           * dialog left open would claim the connect had not happened.
           */
          onSubmit={async (token, engineId) => {
            let result: AddAccountResult | undefined;
            const succeeded = await runAction(
              "account",
              async () => {
                result = await commands.addLichessAccount(token, engineId);
              },
              // No fixed success line: what to say depends on the answer.
              undefined,
              "connect the Lichess account",
            );
            if (!succeeded || !result) return;
            announceTokenVerdict(
              result,
              "Lichess BOT account connected securely",
            );
            closeAccountComposer();
          }}
        />
      )}
      {closeRequested && (liveGames.length > 0 || closeRequestedGames > 0) && (
        <CloseGuard
          games={liveGames}
          reportedCount={closeRequestedGames}
          pending={busy.has(CLOSE_KEY)}
          onKeepPlaying={() => setCloseRequested(false)}
          onClose={() => {
            void runAction(
              CLOSE_KEY,
              () => commands.confirmClose(),
              undefined,
              "close QueenUI",
            );
          }}
        />
      )}
      {notice && <Toast notice={notice} onDismiss={dismissNotice} />}
    </div>
  );
}

export default App;
