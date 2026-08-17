# QueenUI reference

## Working feature slice

- Tauri 2 desktop shell with a Rust backend
- React and TypeScript interface
- Persistent UCI engine profiles with executable probing and metadata discovery
- Per-engine UCI option profiles with handshake-aware controls, validation, defaults, and re-probing
- Per-engine Polyglot BIN and PGN opening books with an off switch, maximum ply depth, and randomized top-weight candidate breadth
- Lichess BOT token validation with tokens stored in Windows Credential Manager
- Independent Start/Stop supervisors and event-stream reconnection for multiple bot accounts
- Outgoing Standard chess challenges with account, opponent, time control, color, and rated/casual controls
- Continuous challenge campaigns that discover online bots, filter by the selected clock rating, randomize opponents, and maintain 1–8 parallel pending challenges/games until stopped
- Per-game UCI engine processes that receive the position and clocks and submit their best move through the Lichess Bot API
- Live-game workspace driven by Lichess game streams, including the board, clocks, status, and latest engine output
- Close confirmation while games are in progress, listing each game and its clock, so an accidental quit cannot lose them on time
- Engine flight recorder: the complete UCI conversation of every game, compressed per session, searchable by move, and exportable as a replayable transcript for engine development
- Application diagnostics with size- and age-based retention, replacing console output a bundled Windows build cannot show
- Engine reassignment for stopped accounts
- Responsive dark operations-console design with no remote runtime assets
- Optional authenticated headless runner: keep Lichess streams, games, UCI
  processes, logs, and history on a Linux or Windows machine while the desktop
  acts only as the control client

## Automatic challenge mode

Open **Challenges** to configure a continuous campaign for any connected bot:

- Choose a minimum and maximum opponent rating.
- Choose concurrency from 1 through 8. Pending challenges and active games both occupy capacity.
- Choose the clock, color, and rated/casual mode. Casual is the default for safer engine testing.
- Start matchmaking. QueenUI discovers up to 512 currently online bots, uses the relevant Bullet/Blitz/Rapid rating, excludes provisional and zero-game ratings, randomizes the eligible pool, and replenishes capacity after declines, timeouts, or finished games.
- Stop matchmaking to cancel outstanding challenges. Already accepted games continue normally.

Opponent cooldowns, two-second request spacing, serialized matchmaking API calls, and 60-second HTTP 429 backoff are built in. A bot is not challenged again by the same campaign for at least 15 minutes.

The live controller distinguishes discovery, idle, challenging, full-capacity, backoff, error, and stopped states. Its timestamped activity feed records the API queue and request phases, scan filter counts, every opponent attempt, challenge creation errors, decline reasons, 20-second timeouts, accepted games, completed games, and stop cleanup. The complete discovery operation—including its shared API-queue wait—has a 12-second deadline, and task panics are surfaced as controller errors, so the UI cannot remain indefinitely on a stale “discovering” state.

## Windows installers

QueenUI is packaged natively on Windows. On a Windows development machine, run:

```powershell
just package-windows
```

This creates both an NSIS `-setup.exe` and an MSI package beneath `src-tauri\target\release\bundle`. To build the NSIS package and open its installer immediately:

```powershell
just install-windows
```

### Build on Windows directly from WSL2

From this WSL2 checkout, use Windows interop to bootstrap the native toolchain and build on the Windows filesystem:

```sh
just wsl-windows-build
```

This calls `powershell.exe`, installs a checksum-verified portable Windows Node.js under `%LOCALAPPDATA%\QueenUI\toolchains`, installs any other missing Windows prerequisites, stages the source under `%LOCALAPPDATA%\QueenUI\wsl-build`, and builds with native Windows Node.js, Rust MSVC, Visual Studio Build Tools, WiX, and NSIS. Finished installers are copied back to `artifacts/windows` in the WSL workspace.

Build and open the Windows installer:

```sh
just wsl-windows-install
```

For the full unattended build → install → launch smoke test:

```sh
just wsl-windows-smoke
```

The [Windows CI workflow](../.github/workflows/windows.yml) runs the same recipes on GitHub's `windows-latest` runner. It builds both installers, silently installs the NSIS package, launches the installed executable for a smoke test, and uploads both installers as workflow artifacts. Run it manually from GitHub's **Actions → Windows CI and installers → Run workflow** screen whenever you want a fresh Windows test build.

Pushing a tag such as `v0.1.0` publishes the already-tested installers as a GitHub Release. Installers are currently unsigned development builds, so Windows SmartScreen may warn during manual testing. Production releases should add Windows code-signing credentials before public distribution.

## Core architecture

Each configured account is owned by an independent Rust supervisor. A supervisor owns its single Lichess event stream, reconnect state, and active game sessions. Every active game session owns a dedicated UCI engine process. The same Tauri-independent core runs embedded in the desktop or inside a headless runner. In remote mode the React/Tauri desktop only sends commands and presents emitted state, so closing the complete desktop cannot stop a game.

The challenge subsystem is a core domain rather than a button around an API call. Direct challenges and continuous rating-range campaigns share the same account connection and game lifecycle. Incoming challenge policies, reusable named presets, history, and scheduled campaign windows can extend the same subsystem.
