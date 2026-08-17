# QueenUI

**A desktop workspace for running Lichess bots.** You bring UCI engines and
Lichess `BOT` accounts; QueenUI runs the fleet — matchmaking, live boards,
engine lifecycles, and an optional headless runner so the games don't live
on your desktop.

![Four live games in the grid view](docs/screenshots/game-grid.png)

## What QueenUI is

- **A bot fleet operator.** Multiple Lichess `BOT` accounts, each isolated:
  one bot, engine, or network failure never interrupts another.
- **A matchmaking campaign runner.** Rating range, clocks, rated/casual,
  concurrency — it finds opponents and keeps games flowing, and stopping it
  lets running games finish.
- **A live cockpit.** Every board in a grid at a glance, or one game in
  focus with engine telemetry; frozen and failed games say so instead of
  pretending.
- **A remote runner.** A small service on another machine drives the
  engines and plays on even when the desktop is closed. Switching between
  local and remote is live — no restart.
- **Built to survive.** Crashes reconcile against Lichess before automation
  resumes, and interrupted games are picked back up.

## What QueenUI is not

Everything else — deliberately. If you want any of the following, better
tools already exist and QueenUI does not try to compete with them:

- **Not a general chess GUI.** No local engine-vs-engine matches, no
  tournament runner, no gauntlets — that's [Cutechess](https://github.com/cutechess/cutechess)
  or [fastchess](https://github.com/Disservin/fastchess).
- **Not an analysis workbench.** No opening prep, no database, no infinite
  analysis board — that's [En Croissant](https://encroissant.org/),
  [Nibbler](https://github.com/rooklift/nibbler), or Lichess itself.
- **Not for playing chess yourself.** It operates bots. It only accepts
  accounts with the `BOT` title and will never move a piece for a human.
- **Lichess only, Standard chess only.**

## Getting started

1. **Engines** → **Add engine** → pick a UCI executable.
2. **Connect bot** → paste a Lichess `BOT` token. The app tells you right
   there whether the token can play and matchmake.
3. Start the account, start matchmaking, watch the grid.

![One game in focus with engine telemetry](docs/screenshots/game-focus.png)

To run engines on another machine, see [headless runners](docs/runner.md) —
pairing is a one-time code, and the security model is documented there.

## Development

Install [`just`](https://just.systems/) and run `just` to list the canonical
project commands (the same recipes CI uses):

```sh
just install   # dependencies
just dev       # run the app
just check     # full verification
```

## Documentation

- [Reference](docs/REFERENCE.md) — feature inventory, campaign behavior, packaging, architecture
- [Headless runners](docs/runner.md) and the [runner protocol](docs/runner-protocol.md)
- [Persistence](docs/persistence.md) and [logs](docs/logs.md)
- [Frontend architecture](docs/frontend-architecture.md)

Licensed [GPL-3.0-or-later](LICENSE). Third-party notices:
[docs/THIRD-PARTY-NOTICES.md](docs/THIRD-PARTY-NOTICES.md).
