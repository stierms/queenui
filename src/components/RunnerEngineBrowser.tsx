import { useCallback, useEffect, useMemo, useState } from "react";
import { ChevronRight, FileCog, Folder, Server } from "lucide-react";
import * as commands from "../api/commands";
import type {
  EngineBrowseEntry,
  EngineBrowseResponse,
  EngineRoot,
} from "../types";
import { Button, Dialog } from "../ui/primitives";

export function RunnerEngineBrowser({
  onClose,
  onRegister,
}: {
  onClose: () => void;
  onRegister: (rootId: string, relativePath: string) => Promise<boolean>;
}) {
  const [roots, setRoots] = useState<EngineRoot[]>([]);
  const [rootId, setRootId] = useState("");
  const [relativePath, setRelativePath] = useState("");
  const [page, setPage] = useState<EngineBrowseResponse | null>(null);
  const [selected, setSelected] = useState<EngineBrowseEntry | null>(null);
  const [loading, setLoading] = useState(true);
  const [registering, setRegistering] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const browse = useCallback(
    async (nextRoot: string, nextPath: string, cursor?: string) => {
      setLoading(true);
      setError(null);
      try {
        const response = await commands.browseEngineRoot({
          rootId: nextRoot,
          relativePath: nextPath,
          cursor: cursor ?? null,
          pageEntries: 100,
        });
        setRootId(nextRoot);
        setRelativePath(nextPath);
        setSelected(null);
        setPage((current) =>
          cursor && current
            ? {
                ...response,
                entries: [...current.entries, ...response.entries],
              }
            : response,
        );
      } catch (cause) {
        setError(cause instanceof Error ? cause.message : String(cause));
      } finally {
        setLoading(false);
      }
    },
    [],
  );

  useEffect(() => {
    let active = true;
    void commands
      .listEngineRoots()
      .then(async (available) => {
        if (!active) return;
        setRoots(available);
        if (available[0]) await browse(available[0].id, "");
        else setLoading(false);
      })
      .catch((cause) => {
        if (!active) return;
        setError(cause instanceof Error ? cause.message : String(cause));
        setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [browse]);

  const crumbs = useMemo(() => {
    const parts = relativePath ? relativePath.split("/") : [];
    return [
      {
        label: roots.find((root) => root.id === rootId)?.label ?? rootId,
        path: "",
      },
      ...parts.map((part, index) => ({
        label: part,
        path: parts.slice(0, index + 1).join("/"),
      })),
    ];
  }, [relativePath, rootId, roots]);

  async function registerSelected() {
    if (!selected || selected.kind !== "file") return;
    setRegistering(true);
    const registered = await onRegister(rootId, selected.relativePath);
    setRegistering(false);
    if (registered) onClose();
  }

  return (
    <Dialog.Root open onOpenChange={(open) => !open && onClose()}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-20 bg-black/70 backdrop-blur-[7px]" />
        <Dialog.Content className="runner-engine-browser fixed left-1/2 top-1/2 z-[21] -translate-x-1/2 -translate-y-1/2 focus:outline-none">
          <div className="modal-head">
            <div className="modal-icon">
              <Server size={20} />
            </div>
            <div>
              <span className="eyebrow">Administrator namespace</span>
              <Dialog.Title>Browse trusted engines</Dialog.Title>
              <Dialog.Description>
                Select an executable below a configured runner root. QueenUI
                copies its bytes into the runner&apos;s content-addressed store.
              </Dialog.Description>
            </div>
            <Dialog.Close asChild>
              <Button variant="icon" aria-label="Close engine browser">
                ×
              </Button>
            </Dialog.Close>
          </div>

          <div className="runner-browser-toolbar">
            <label>
              <span>Engine root</span>
              <select
                aria-label="Engine root"
                value={rootId}
                disabled={loading || roots.length === 0}
                onChange={(event) => void browse(event.target.value, "")}
              >
                {roots.map((root) => (
                  <option key={root.id} value={root.id}>
                    {root.label}
                  </option>
                ))}
              </select>
            </label>
            <nav aria-label="Engine folder">
              {crumbs.map((crumb, index) => (
                <span key={`${crumb.path}-${index}`}>
                  {index > 0 && <ChevronRight size={13} />}
                  <button onClick={() => void browse(rootId, crumb.path)}>
                    {crumb.label || "Root"}
                  </button>
                </span>
              ))}
            </nav>
          </div>

          <div
            className="runner-browser-entries"
            role="listbox"
            aria-label="Trusted engine files"
          >
            {loading && <p>Reading the bounded runner namespace…</p>}
            {!loading && error && (
              <p className="runner-browser-error">{error}</p>
            )}
            {!loading && !error && roots.length === 0 && (
              <p>No engine roots are configured on this runner.</p>
            )}
            {!loading && !error && page?.entries.length === 0 && (
              <p>This folder contains no visible trusted entries.</p>
            )}
            {!loading &&
              !error &&
              page?.entries.map((entry) => (
                <button
                  key={entry.relativePath}
                  role="option"
                  aria-selected={selected?.relativePath === entry.relativePath}
                  className={
                    selected?.relativePath === entry.relativePath
                      ? "selected"
                      : ""
                  }
                  onDoubleClick={() =>
                    entry.kind === "directory" &&
                    void browse(rootId, entry.relativePath)
                  }
                  onClick={() =>
                    entry.kind === "directory"
                      ? void browse(rootId, entry.relativePath)
                      : setSelected(entry)
                  }
                >
                  {entry.kind === "directory" ? (
                    <Folder size={18} />
                  ) : (
                    <FileCog size={18} />
                  )}
                  <span>
                    <strong>{entry.name}</strong>
                    <small>
                      {entry.kind === "directory"
                        ? "Folder"
                        : `${formatBytes(entry.size)} · ${entry.executable ? "executable" : "not executable"}`}
                    </small>
                  </span>
                </button>
              ))}
            {page?.nextCursor && !loading && (
              <Button
                variant="secondary"
                onClick={() =>
                  void browse(
                    rootId,
                    relativePath,
                    page.nextCursor ?? undefined,
                  )
                }
              >
                Load more
              </Button>
            )}
          </div>

          <div className="runner-browser-actions">
            <small>
              The browser intentionally discloses names, sizes, modification
              times, and executable bits inside configured roots to the paired
              bearer holder.
            </small>
            <Button variant="secondary" onClick={onClose}>
              Cancel
            </Button>
            <Button
              variant="primary"
              disabled={!selected?.executable || registering}
              onClick={() => void registerSelected()}
            >
              {registering ? "Registering…" : "Register selected engine"}
            </Button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MiB`;
}
