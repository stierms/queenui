import {
  BookOpen,
  Bot,
  Gamepad2,
  LayoutDashboard,
  Plus,
  Settings,
  Swords,
  TerminalSquare,
} from "lucide-react";
import { botStatusLabel } from "../lib/format";
import type { NavId } from "../lib/navigation";
import { runtimeFor, type AppSnapshot } from "../types";
import { StatusDot } from "./StatusDot";

const navItems: Array<{ label: NavId; icon: typeof LayoutDashboard }> = [
  { label: "Overview", icon: LayoutDashboard },
  { label: "Games", icon: Gamepad2 },
  { label: "Scorebook", icon: BookOpen },
  { label: "Challenges", icon: Swords },
  { label: "Engines", icon: Bot },
  { label: "Logs", icon: TerminalSquare },
];

export function Sidebar({
  snapshot,
  activeNav,
  liveGameCount,
  activeCampaigns,
  stale = false,
  onNavigate,
  onAddAccount,
}: {
  snapshot: AppSnapshot;
  activeNav: NavId;
  liveGameCount: number;
  activeCampaigns: number;
  /** Counts and statuses below come from a snapshot that may be out of date. */
  stale?: boolean;
  onNavigate: (page: NavId) => void;
  onAddAccount: () => void;
}) {
  return (
    <aside className="sidebar">
      <div className="brand">
        <div className="brand-mark">♛</div>
        <div>
          <strong>QueenUI</strong>
          <span>Chess operations</span>
        </div>
      </div>
      <nav className="main-nav" aria-label="Main navigation">
        <p className="section-label">Workspace</p>
        {navItems.map(({ label, icon: Icon }) => (
          /*
           * No `aria-label` on the button: it would replace the whole subtree
           * for name computation, and the badges live in that subtree. With
           * the label coming from the visible text instead, the Games item
           * announces as "Games, 3 live games" rather than just "Games" —
           * previously the counts reached assistive technology by no path at
           * all, because the badge's own `aria-label` sat on an `<em>` (role
           * `emphasis`, which prohibits an author name) and was dropped too.
           */
          <button
            className={activeNav === label ? "active" : ""}
            key={label}
            aria-current={activeNav === label ? "page" : undefined}
            onClick={() => onNavigate(label)}
          >
            <Icon size={17} strokeWidth={1.8} />
            <span>{label}</span>
            {label === "Games" && liveGameCount > 0 && (
              <em>
                {/*
                 * The digit is hidden and restated in full by the sr-only
                 * text: name computation concatenates node text without
                 * inserting separators, so "Games" + "3" + "live games" would
                 * otherwise be announced as one run-together word. The comma
                 * is a real character and survives that concatenation.
                 */}
                <span aria-hidden="true">{liveGameCount}</span>
                <span className="sr-only">
                  {`, ${liveGameCount} live game${liveGameCount === 1 ? "" : "s"}`}
                </span>
              </em>
            )}
            {label === "Challenges" && activeCampaigns > 0 && (
              <em>
                <span aria-hidden="true">{activeCampaigns}</span>
                <span className="sr-only">
                  {`, ${activeCampaigns} active campaign${activeCampaigns === 1 ? "" : "s"}`}
                </span>
              </em>
            )}
          </button>
        ))}
      </nav>
      <section className={`sidebar-fleet ${stale ? "is-stale" : ""}`}>
        <div className="sidebar-section-title">
          <p className="section-label">Bot fleet</p>
          <button aria-label="Add bot" onClick={onAddAccount}>
            <Plus size={15} />
          </button>
        </div>
        {stale && (
          <p className="sidebar-stale" role="status">
            Last known state — waiting for the runner
          </p>
        )}
        {snapshot.accounts.length === 0 && (
          <p className="sidebar-empty">No accounts connected</p>
        )}
        {snapshot.accounts.map((account) => {
          const runtime = runtimeFor(snapshot, account.id);
          return (
            <button
              className="mini-bot"
              key={account.id}
              aria-label={account.username}
              onClick={() => onNavigate("Overview")}
            >
              <span className="avatar">{account.username[0]}</span>
              <span className="mini-bot-copy">
                <strong>{account.username}</strong>
                {/*
                 * Backend strings land here verbatim and an error can be
                 * arbitrarily long, so the full text is available on hover
                 * rather than being silently clipped to a fixed width.
                 *
                 * An error also has to *look* like one: it used to occupy the
                 * same slot and styling as the status word, so "reconnecting"
                 * and a stack of failure text were the same shape, and a bot
                 * in trouble read as a bot going about its business.
                 */}
                <small
                  className={runtime.error ? "mini-bot-error" : undefined}
                  title={runtime.error || botStatusLabel(runtime.status)}
                >
                  {runtime.error
                    ? `Error: ${runtime.error}`
                    : botStatusLabel(runtime.status)}
                </small>
              </span>
              <StatusDot status={runtime.status} />
            </button>
          );
        })}
      </section>
      <button
        className={`settings-link ${activeNav === "Settings" ? "active" : ""}`}
        aria-label="Settings"
        aria-current={activeNav === "Settings" ? "page" : undefined}
        onClick={() => onNavigate("Settings")}
      >
        <Settings size={17} />
        <span>Settings</span>
      </button>
    </aside>
  );
}
