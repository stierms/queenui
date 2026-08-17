# Frontend architecture

QueenUI is a native desktop operations UI, not a document-style website. Its frontend choices should favor dense live data, predictable resizing, accessibility, and low runtime overhead.

## Styling system

Tailwind CSS v4 is QueenUI's canonical design system. It is integrated directly through the official Vite plugin and configured CSS-first in `src/App.css`.

The boundary is deliberate:

- shared colors, typography, spacing, focus states, and reusable controls use Tailwind theme tokens and utilities;
- component variants are composed with Class Variance Authority, `clsx`, and `tailwind-merge` in `src/ui/primitives.tsx`;
- fluid content widths and container queries remain in CSS where the browser's layout language is clearer than a long utility expression;
- chess-specific rendering—board geometry, SVG pieces, arrows, evaluation bar, and move telemetry—remains purpose-built CSS;
- viewport queries are reserved for application-shell changes such as the compact sidebar;
- Playwright checks compact, standard, and wide desktop viewport classes in Microsoft Edge on Windows and Chromium elsewhere.

New generic controls must be added to `src/ui/primitives.tsx` instead of introducing another one-off button or dialog class in `App.css`. It currently holds `Button`, `TooltipButton`, `Switch`, `SelectField`, `ConfirmDialog` (every destructive action in the app asks through this one) and `RowMenu`, plus the Radix re-exports.

## Visual identity — "Ebony & Bone"

The design language is a warm dark operations console. Its rules:

- **Neutrals** are a warm ebony ramp (`--bg #141210`, `--panel #1b1917`, hairlines `--line-1/--line/--line-3`), never blue-black.
- **Bone (`#e9e4d6`) is the primary action color** — filled bone buttons with dark text — and the strong-text tier.
- **Moss (`#8fae62`) means "alive" and nothing else**: live status dots, THINKING, active clocks, running campaigns. **Brass** is warning/backoff — and therefore _stale_, since frozen data is exactly a warning — **Claret** is error/danger, **Slate** is info. Never reuse a role color decoratively.
- **Nothing may claim to be live unless it is.** When the runner link is degraded the app has a single answer: the "Live on Lichess" eyebrow, the moss dot, the THINKING pulse, the best-move arrow and the clock interpolation all stop, the board is marked frozen, and one brass banner explains why. A ticking clock on data that stopped arriving is a _more_ convincing live game than a real one, which is the one failure mode this app must not have. The state lives in `src/lib/connection.ts`; it is derived from the backend's namespaced `queenui://runner-connection` event and from failed initial/retry snapshot loads. Ordinary command failures remain operation-specific notices: an API rejection is not evidence that the event link is stale. No "no snapshot for N seconds" timer is used, because an engine thinking for three minutes legitimately emits nothing.
- **Typography**: IBM Plex Sans (UI), IBM Plex Mono (every number: clocks, telemetry, ratings, eyebrow labels — mono 11px/600/0.08em uppercase), Spectral 600 for page titles only. All bundled via @fontsource; no remote assets.
- **The eval bar shows white's share** (bone over ebony) regardless of which side the bot plays. It also **never falls back to 0.00 between moves**: the backend clears telemetry when a new search starts and UCI emits scoreless `info` lines, so the last scored evaluation is held (`useRetainedEvaluation`) and the bar animates from there to the new score.
- **The topbar and the games toolbar dock while the page scrolls** (`--topbar-height`, `--z-topbar`, `--z-page-header`), so the page title and the live/all filter stay put through a tall stack of boards. Keep sticky layers below the dialog overlay (z 20).
- **Figurine notation** (`src/components/Figurine.tsx`) renders moves with piece glyphs wherever SAN appears — it is the app's signature detail; new move displays should use it.
- Radii (`--r-sm/md/lg/xl`), elevation (`--elev-1/2/3`), and the single `live-pulse` keyframe are tokens — do not introduce ad-hoc values.
- The primary composition target is a 2560×1440 window (half of a 5120×1440 screen); the workspace width-caps at `--dashboard-width` and the board absorbs surplus height (`--board-size` is viewport-height-aware).
- Board themes carry `light/dark/accent/highlight`; piece sets share a base but differ genuinely in geometry and finish (Regal sculpted, Neo minimal, Crystal glass). Black pieces rely on the halo rim for dark-square legibility — keep it when adding sets.

## Library policy

Libraries should be added when they remove complex behavior, not merely to replace small amounts of React or CSS.

### Adopted foundation

- **Radix Primitives:** owns behavior for dialogs, popovers, menus, tooltips, and future composite controls. Radix supplies focus management, keyboard interaction, portal layering, and collision handling; Tailwind owns presentation.
- **Playwright:** covers responsive layout and critical desktop workflow regressions. Windows CI runs the tests against Microsoft Edge.
- **Prettier and ESLint:** formatting and linting (typescript-eslint plus the React hooks rules) are enforced by `just check` so CSS and component changes remain reviewable.

### Add when scale requires it

- **TanStack Virtual:** add when game history, engine logs, or campaign audit trails become long enough that rendering the complete collection is measurable.
- **React Hook Form with Zod:** consider when engine options and account/campaign forms gain conditional fields or shared validation schemas.

### Not currently justified

- **TanStack Query:** QueenUI receives an authoritative snapshot and event stream from the Tauri backend rather than independently fetching cacheable HTTP resources.
- **Zustand or Redux:** current shared state is small and owned by one Tauri integration boundary. First extract domain hooks and split the large `App.tsx`; add a store only if prop coordination remains a demonstrated problem.
- **Animation frameworks:** the current transitions are small CSS effects and do not warrant additional runtime code.

## Module structure

`App.tsx` is a shell that wires navigation, dialogs, and shared hooks. Everything else lives in focused modules:

- `src/types/` — the shared data contracts. **Import from `src/types` (the barrel), never from `src/types/models` directly**: `models.ts` is the hand-written mirror of the Rust serde types and is meant to be replaceable by a generated `models.gen.ts`, while `helpers.ts` holds what is ours (`assertNever`, the snapshot lookups, and the guards that narrow the discriminants the backend leaves open).
- `src/api/commands.ts` — one typed wrapper per Tauri command; components never call `invoke` directly. `src/api/credentials.ts` is the same thing for the commands that _delete_ a stored secret, kept apart so a rename is one line.
- `src/api/events.ts` — one race-safe `subscribe` (unsubscribe-before-resolve safe, StrictMode safe) plus a named wrapper per event.
- `src/lib/` — pure helpers: chess/PGN derivation (with a bounded position cache keyed on game id + move list, so it hits across snapshots), connection/staleness state, display formatting (`format.ts` — the one home for byte, date and duration strings), navigation ids, time controls, appearance, evaluation formatting, move sounds, error text.
- `src/hooks/` — `useSnapshot` (snapshot + connection health + retry), `useRunnerSettings` (one owner for the execution target), `useNotices`, `useActionRunner` (busy-key set, no unhandled rejections), `useGamesInDisplayOrder`, `useMoveSounds`, `useRetainedEvaluation` (per-game evaluation continuity).
- `src/components/` — board, live game panel, telemetry, appearance controls, dialogs, toast, the connection banner, and other shared presentation. Page-sized features get their own folder (`components/logs/`, `components/scorebook/`) so a page module stays state + composition.
- **Closing is guarded while games run.** The backend intercepts `CloseRequested`, and only prevents it when live games exist, so a normal quit stays instant. It then emits `queenui://close-requested` and `CloseGuard` names the consequence — Lichess keeps the clocks running, so an abandoned game is lost on time — with the safe action focused. Confirming calls `confirm_close`, which uses `destroy` rather than `close` so the same question is not asked twice.
- `src/pages/` — Overview, Games, Scorebook, Challenges, Engines, Logs, Settings.
- `src/dev/preview.ts` — DEV-only presentation preview snapshot and URL-parameter plumbing.

Generic interactive pieces belong in the primitive layer; domain state stays with its feature until cross-feature coordination demonstrates a need for a store. Pure helpers get unit tests next to their module (`src/lib/*.test.ts`).
