import { useRef, useState } from "react";
import { Bot, FolderOpen, Plus, SlidersHorizontal, Trash2 } from "lucide-react";
import { EngineConfigurationDialog } from "../components/EngineConfigurationDialog";
import { RunnerEngineBrowser } from "../components/RunnerEngineBrowser";
import type { BusyKeys } from "../hooks/useActionRunner";
import type { ShowNotice } from "../hooks/useNotices";
import { fullDate, relativeSince, timeOfDay } from "../lib/format";
import type {
  AppSnapshot,
  EngineProfile,
  OpeningBookRequest,
  EngineOptionUpdate,
} from "../types";
import { Button, ConfirmDialog } from "../ui/primitives";

export function EnginesPage({
  snapshot,
  busy,
  remoteRunner = false,
  showNotice,
  onAdd,
  onRegister,
  onRemove,
  onSaveOptions,
  onRefreshOptions,
  onSaveBook,
  onClearBook,
}: {
  snapshot: AppSnapshot;
  busy: BusyKeys;
  remoteRunner?: boolean;
  /** Passed through to the configuration dialog's file pickers. */
  showNotice: ShowNotice;
  onAdd: () => void;
  onRegister: (rootId: string, relativePath: string) => Promise<boolean>;
  onRemove: (engine: EngineProfile) => void;
  onSaveOptions: (
    engine: EngineProfile,
    options: EngineOptionUpdate[],
  ) => Promise<boolean>;
  onRefreshOptions: (engine: EngineProfile) => Promise<boolean>;
  onSaveBook: (
    engine: EngineProfile,
    book: OpeningBookRequest,
  ) => Promise<boolean>;
  onClearBook: (engine: EngineProfile) => Promise<boolean>;
}) {
  const [configurationEngineId, setConfigurationEngineId] = useState<
    string | null
  >(null);
  const [browserOpen, setBrowserOpen] = useState(false);
  const [removing, setRemoving] = useState<EngineProfile | null>(null);
  const configurationReturnFocus = useRef<HTMLElement | null>(null);
  const configurationEngine = snapshot.engines.find(
    (engine) => engine.id === configurationEngineId,
  );

  /*
   * One dialog at a time.
   *
   * Every trigger on this page — the hero button, a card's Configure, a card's
   * Remove — sits *behind* whichever dialog is open: Radix puts
   * `pointer-events: none` on the body, traps focus, and aria-hides the
   * background, so a click that arrives here while a dialog is up did not come
   * from an operator who could see the control. Nothing stopped that click
   * from opening a second one, and the removal confirmation renders before the
   * configuration dialog, so it stacked *underneath* at the same z-index:
   * invisible and unclickable while the top dialog lived, then revealed as an
   * unrequested "Remove <engine>?" the moment that dialog closed. Refusing the
   * second open is what the modality already promises.
   */
  const dialogOpen =
    removing !== null || configurationEngineId !== null || browserOpen;

  function addOrBrowse() {
    if (dialogOpen) return;
    if (remoteRunner) setBrowserOpen(true);
    else onAdd();
  }

  function requestRemoval(engine: EngineProfile) {
    if (dialogOpen) return;
    setRemoving(engine);
  }

  function openConfiguration(engineId: string) {
    if (dialogOpen) return;
    configurationReturnFocus.current =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    setConfigurationEngineId(engineId);
  }

  function closeConfiguration() {
    setConfigurationEngineId(null);
    window.setTimeout(() => configurationReturnFocus.current?.focus(), 0);
  }

  return (
    <div className="module-content">
      <section className="module-hero">
        <div>
          <span className="eyebrow">
            {remoteRunner ? "Runner executables" : "Local executables"}
          </span>
          <h2>Engine profiles</h2>
          <p>
            Every executable is launched and UCI-probed on the machine that will
            run it before QueenUI accepts it.
          </p>
        </div>
        <Button
          variant="primary"
          onClick={addOrBrowse}
          disabled={busy.has(remoteRunner ? "register-engine" : "add-engine")}
        >
          {remoteRunner ? <FolderOpen size={17} /> : <Plus size={17} />}
          {busy.has("register-engine")
            ? "Registering…"
            : busy.has("add-engine")
              ? "Probing…"
              : remoteRunner
                ? "Browse trusted engines"
                : "Add engine"}
        </Button>
      </section>
      <section className="engine-grid">
        {snapshot.engines.map((engine) => (
          <article className="panel engine-card" key={engine.id}>
            <div className="engine-card-icon">
              <Bot />
            </div>
            <div className="engine-card-title">
              <h3>{engine.name}</h3>
              <ProbeBadge engine={engine} />
            </div>
            <p title={engine.path}>{engine.path}</p>
            <dl>
              <div>
                <dt>Author</dt>
                <dd>{engine.author || "Not reported"}</dd>
              </div>
              <div>
                <dt>UCI options</dt>
                <dd>{engine.optionCount}</dd>
              </div>
              <div>
                <dt>Assigned bots</dt>
                <dd>
                  {
                    snapshot.accounts.filter(
                      (account) => account.engineId === engine.id,
                    ).length
                  }
                </dd>
              </div>
              <div>
                <dt>Opening book</dt>
                <dd>
                  {engine.openingBook?.enabled
                    ? engine.openingBook.name
                    : "Disabled"}
                </dd>
              </div>
            </dl>
            <div className="engine-card-actions">
              <Button
                variant="secondary"
                onClick={() => openConfiguration(engine.id)}
              >
                <SlidersHorizontal size={14} />
                Configure
              </Button>
              <Button
                variant="ghost"
                className="text-claret hover:bg-claret/10 hover:text-claret-bright"
                disabled={busy.has(`engine-${engine.id}`)}
                onClick={() => requestRemoval(engine)}
              >
                <Trash2 size={14} />
                Remove
              </Button>
            </div>
          </article>
        ))}
        {snapshot.engines.length === 0 && (
          <button className="add-engine-card" onClick={addOrBrowse}>
            {remoteRunner ? <FolderOpen /> : <Plus />}
            <strong>Add your first engine</strong>
            <small>
              {remoteRunner
                ? "Choose from an administrator-configured engine root"
                : "Select a native Windows .exe"}
            </small>
          </button>
        )}
      </section>
      {/*
       * Removing a profile discards its saved UCI options and book config, and
       * any bot assigned to it loses its engine — it asks first, like the other
       * destructive actions in the app.
       */}
      <ConfirmDialog
        open={removing !== null}
        title={`Remove ${removing?.name ?? ""}?`}
        description={removalConsequence(snapshot, removing)}
        confirmLabel="Remove engine profile"
        pending={removing ? busy.has(`engine-${removing.id}`) : false}
        onCancel={() => setRemoving(null)}
        onConfirm={() => {
          if (removing) onRemove(removing);
          setRemoving(null);
        }}
      />
      {configurationEngine && (
        <EngineConfigurationDialog
          key={configurationEngine.id}
          engine={configurationEngine}
          remoteRunner={remoteRunner}
          busy={busy}
          showNotice={showNotice}
          onClose={closeConfiguration}
          onSaveOptions={onSaveOptions}
          onRefreshOptions={onRefreshOptions}
          onSaveBook={onSaveBook}
          onClearBook={onClearBook}
        />
      )}
      {remoteRunner && browserOpen && (
        <RunnerEngineBrowser
          onClose={() => setBrowserOpen(false)}
          onRegister={onRegister}
        />
      )}
    </div>
  );
}

/**
 * What the last UCI probe of a profile actually established, and when.
 *
 * The card used to print "UCI ready" on every profile unconditionally, which
 * the frontend had no evidence for: a retained profile whose executable has
 * since been deleted rendered identically to one that answered `uciok` a
 * second ago. `probeOk` is the backend's last result and `lastProbedAtMs` is
 * when it was taken. Loading the saved configuration does not re-probe, so the
 * timestamp is the claim's freshness bound and is shown beside it rather than
 * left implicit — a profile verified three weeks ago is a three-week-old
 * claim, not a live one.
 *
 * Both fields absent means no probe has ever been recorded for the profile (it
 * predates the fields), which is a third state, not a failure: it reads
 * neutral, never green.
 */
function probeClaim(
  engine: Pick<EngineProfile, "probeOk" | "lastProbedAtMs">,
  now = Date.now(),
): { tone: "ok" | "failed" | "unknown"; label: string; detail: string } {
  const at = engine.lastProbedAtMs;
  const when = at == null ? null : relativeSince(at, now);
  const stamp = at == null ? null : `${fullDate(at)} at ${timeOfDay(at)}`;
  if (engine.probeOk === true) {
    return {
      tone: "ok",
      label: when ? `UCI verified ${when}` : "UCI verified",
      detail: stamp
        ? `The executable answered a UCI probe on ${stamp}. QueenUI does not re-probe when it loads the configuration, so that is the most recent evidence it has.`
        : "The executable answered a UCI probe, but no time was recorded for it.",
    };
  }
  if (engine.probeOk === false) {
    return {
      tone: "failed",
      label: when ? `Probe failed ${when}` : "Probe failed",
      detail: stamp
        ? `The last UCI probe failed on ${stamp} — the executable may be gone, or may no longer answer UCI. The profile is kept so its saved options and book survive.`
        : "The last UCI probe of this profile failed. The profile is kept so its saved options and book survive.",
    };
  }
  return {
    tone: "unknown",
    label: "Not probed yet",
    detail:
      "No probe result is stored for this profile — it was saved before QueenUI recorded them, and loading the configuration does not probe.",
  };
}

function ProbeBadge({ engine }: { engine: EngineProfile }) {
  const claim = probeClaim(engine);
  return (
    <span
      className={`engine-probe engine-probe-${claim.tone}`}
      title={claim.detail}
    >
      {claim.label}
    </span>
  );
}

function removalConsequence(
  snapshot: AppSnapshot,
  engine: EngineProfile | null,
) {
  if (!engine) return "";
  const assigned = snapshot.accounts.filter(
    (account) => account.engineId === engine.id,
  );
  const base =
    "Its saved UCI options and opening-book policy are deleted. The executable itself is left alone.";
  if (assigned.length === 0) return base;
  return `${assigned.map((account) => account.username).join(", ")} ${assigned.length === 1 ? "is" : "are"} assigned to it and will have no engine until reassigned. ${base}`;
}
