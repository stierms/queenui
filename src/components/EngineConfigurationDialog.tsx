import { useEffect, useMemo, useState } from "react";
import { BookOpen, SlidersHorizontal, Trash2, Upload } from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import * as commands from "../api/commands";
import type { BusyKeys } from "../hooks/useActionRunner";
import type { ShowNotice } from "../hooks/useNotices";
import { formatBytes } from "../lib/format";
import { pickPath } from "../lib/fileDialog";
import {
  assertNever,
  uciControlKind,
  type EngineProfile,
  type OpeningBookRequest,
  type OpeningBookAsset,
  type UciOption,
  type EngineOptionUpdate,
} from "../types";
import { Button, ConfirmDialog, Dialog, Switch, Tabs } from "../ui/primitives";

type BookEdits = Partial<OpeningBookRequest>;

/**
 * Engine configuration dialog.
 *
 * Local state tracks only the user's *edits* (dirty values) and lays them
 * over the engine profile from the latest snapshot, so live snapshot
 * events never wipe in-progress changes, while re-probes and clears still
 * reconcile into the fields the user has not touched. Mount it with
 * `key={engine.id}` so switching engines starts from a clean slate.
 */
export function EngineConfigurationDialog({
  engine,
  remoteRunner = false,
  busy,
  showNotice,
  onClose,
  onSaveOptions,
  onRefreshOptions,
  onSaveBook,
  onClearBook,
}: {
  engine: EngineProfile;
  remoteRunner?: boolean;
  busy: BusyKeys;
  /** So a failed file dialog is reported rather than silently doing nothing. */
  showNotice: ShowNotice;
  onClose: () => void;
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
  const options = useMemo(() => engine.options ?? [], [engine.options]);
  const [optionEdits, setOptionEdits] = useState<Record<string, string>>({});
  const optionValues = useMemo(
    () => ({
      ...Object.fromEntries(
        options.map((option) => [
          option.name,
          option.value ?? option.defaultValue ?? "",
        ]),
      ),
      ...optionEdits,
    }),
    [options, optionEdits],
  );
  const [bookEdits, setBookEdits] = useState<BookEdits>({});
  const [remoteBookAssets, setRemoteBookAssets] = useState<OpeningBookAsset[]>(
    [],
  );
  const [loadingRemoteBooks, setLoadingRemoteBooks] = useState(remoteRunner);
  const [remoteBookError, setRemoteBookError] = useState<string | null>(null);
  const savedBook = engine.openingBook ?? null;
  const bookPath = bookEdits.path ?? savedBook?.path ?? "";
  const bookEnabled = bookEdits.enabled ?? savedBook?.enabled ?? true;
  const maxPlies = bookEdits.maxPlies ?? savedBook?.maxPlies ?? 20;
  const topMovePercent =
    bookEdits.topMovePercent ?? savedBook?.topMovePercent ?? 10;
  const savingOptions = busy.has(`options-${engine.id}`);
  const refreshingOptions = busy.has(`refresh-options-${engine.id}`);
  /*
   * Saving and clearing the book shared one busy key, so removing a book made
   * the *Save* button read "Validating…" — the wrong control describing the
   * wrong operation. Separate keys, separate pending states.
   */
  const savingBook = busy.has(`book-${engine.id}`);
  const clearingBook = busy.has(`book-clear-${engine.id}`);
  const dirty =
    Object.keys(optionEdits).length > 0 || Object.keys(bookEdits).length > 0;
  const [confirmingDiscard, setConfirmingDiscard] = useState(false);
  const [confirmingRemoveBook, setConfirmingRemoveBook] = useState(false);
  const [confirmingReset, setConfirmingReset] = useState(false);

  useEffect(() => {
    if (!remoteRunner) return;
    let active = true;
    void commands
      .listOpeningBookAssets()
      .then((assets) => {
        if (active) setRemoteBookAssets(assets);
      })
      .catch((cause) => {
        if (!active) return;
        setRemoteBookError(
          cause instanceof Error ? cause.message : String(cause),
        );
      })
      .finally(() => {
        if (active) setLoadingRemoteBooks(false);
      });
    return () => {
      active = false;
    };
  }, [remoteRunner]);

  /*
   * Every confirmation this dialog can raise. Each is its own Radix layer, so
   * opening one moves focus out of the engine dialog's content and Radix
   * reports that as an interaction *outside* — which used to run the dirty
   * guard and stack "Discard your changes?" on top of the question the
   * operator had only just been asked. The guard steps aside while one of
   * them is open, but still prevents the default dismissal, or the engine
   * dialog would close out from under its own confirmation.
   */
  const confirmationOpen =
    confirmingDiscard || confirmingRemoveBook || confirmingReset;

  /**
   * The single decision behind every route out of this dialog: may it close
   * now, or does something have to be asked first? Escape, an overlay click
   * and the × all go through here, so none of them can quietly discard an
   * edit session — potentially sixty options — that the others prompt about.
   *
   * Returns whether the dialog may close. Raising the discard question is a
   * side effect of answering "not yet".
   */
  function mayClose() {
    if (confirmationOpen) return false;
    if (!dirty) return true;
    setConfirmingDiscard(true);
    return false;
  }

  /**
   * Escape and an overlay click used to throw away an in-progress edit
   * session with no prompt. The dirty state that drives the question already
   * existed. Radix decides the dismissal for these two, so a refusal has to
   * be a `preventDefault`.
   */
  function guardDismiss(event: { preventDefault: () => void }) {
    if (!mayClose()) event.preventDefault();
  }

  async function chooseBook() {
    if (remoteRunner) return;
    const selected = await pickPath(
      () =>
        open({
          multiple: false,
          filters: [
            {
              name: "Opening books",
              extensions: ["bin", "pgn"],
            },
          ],
        }),
      showNotice,
      "choose an opening book",
    );
    if (typeof selected === "string")
      setBookEdits((edits) => ({ ...edits, path: selected }));
  }

  function setOption(name: string, value: string) {
    setOptionEdits((current) => ({ ...current, [name]: value }));
  }

  function setBookEdit<Key extends keyof OpeningBookRequest>(
    key: Key,
    value: OpeningBookRequest[Key],
  ) {
    setBookEdits((edits) => ({ ...edits, [key]: value }));
  }

  async function saveOptions() {
    const succeeded = await onSaveOptions(
      engine,
      options
        .filter((option) => uciControlKind(option.optionType) !== "button")
        .map((option) => ({
          name: option.name,
          value: optionValues[option.name] ?? "",
        })),
    );
    if (succeeded) setOptionEdits({});
  }

  async function saveBook() {
    const succeeded = await onSaveBook(engine, {
      path: bookPath,
      enabled: bookEnabled,
      maxPlies,
      topMovePercent,
    });
    if (succeeded) setBookEdits({});
  }

  async function clearBook() {
    const succeeded = await onClearBook(engine);
    if (succeeded) setBookEdits({});
  }

  return (
    <>
      <Dialog.Root
        open
        onOpenChange={(open) => {
          if (!open) onClose();
        }}
      >
        <Dialog.Portal>
          <Dialog.Overlay className="fixed inset-0 z-20 bg-black/70 backdrop-blur-[7px]" />
          <Dialog.Content
            className="engine-config-modal fixed left-1/2 top-1/2 z-[21] -translate-x-1/2 -translate-y-1/2 focus:outline-none"
            onEscapeKeyDown={guardDismiss}
            onInteractOutside={guardDismiss}
          >
            <div className="modal-head">
              <div className="modal-icon">
                <SlidersHorizontal size={20} />
              </div>
              <div>
                <span className="eyebrow">Engine profile</span>
                <Dialog.Title>Configure {engine.name}</Dialog.Title>
                <Dialog.Description>
                  Book policy and UCI values apply the next time this engine is
                  started.
                </Dialog.Description>
              </div>
              {/*
               * Deliberately not a `Dialog.Close`: that reaches
               * `onOpenChange` directly and so bypassed the guard Escape and
               * overlay clicks run, making × the one dismissal that silently
               * threw away pending UCI and book edits.
               */}
              <Button
                variant="icon"
                className="text-lg leading-none"
                aria-label="Close"
                onClick={() => {
                  if (mayClose()) onClose();
                }}
              >
                ×
              </Button>
            </div>

            <Tabs.Root defaultValue="book" className="engine-config-tabs">
              <Tabs.List
                aria-label="Engine configuration"
                className="config-tab-list"
              >
                <Tabs.Trigger value="book">
                  <BookOpen size={15} /> Opening book
                </Tabs.Trigger>
                <Tabs.Trigger value="uci">
                  <SlidersHorizontal size={15} /> UCI options
                  <span>{options.length}</span>
                </Tabs.Trigger>
              </Tabs.List>

              <Tabs.Content value="book" className="config-tab-content">
                <div className="book-source-card">
                  <span className="book-source-icon">
                    <BookOpen size={22} />
                  </span>
                  <div>
                    <strong>
                      {bookPath
                        ? bookPath.split(/[\\/]/).pop()
                        : "No opening book selected"}
                    </strong>
                    <small>
                      {savedBook && bookPath === savedBook.path
                        ? `${savedBook.format === "polyglot" ? "Polyglot BIN" : "Portable PGN"} · ${savedBook.entryCount.toLocaleString()} weighted move entries`
                        : "Polyglot .bin and portable .pgn are supported"}
                    </small>
                    {bookPath && <code title={bookPath}>{bookPath}</code>}
                  </div>
                  {remoteRunner ? (
                    <div className="runner-book-source">
                      {remoteBookError ? (
                        <input
                          aria-label="Opening book path on runner"
                          value={bookPath}
                          placeholder="/home/user/books/openings.bin"
                          onChange={(event) =>
                            setBookEdit("path", event.target.value)
                          }
                        />
                      ) : (
                        <select
                          aria-label="Approved opening book on runner"
                          value={bookPath}
                          disabled={loadingRemoteBooks}
                          onChange={(event) =>
                            setBookEdit("path", event.target.value)
                          }
                        >
                          <option value="">
                            {loadingRemoteBooks
                              ? "Loading approved books…"
                              : "Select an approved book"}
                          </option>
                          {savedBook &&
                            !remoteBookAssets.some(
                              (asset) => asset.path === savedBook.path,
                            ) && (
                              <option value={savedBook.path}>
                                {savedBook.name} · current managed copy
                              </option>
                            )}
                          {remoteBookAssets.map((asset) => (
                            <option value={asset.path} key={asset.path}>
                              {asset.name} · {formatBytes(asset.size)}
                            </option>
                          ))}
                        </select>
                      )}
                      <small
                        className={remoteBookError ? "runner-book-error" : ""}
                      >
                        {remoteBookError
                          ? `${remoteBookError}. Exact-path entry remains available as a compatibility fallback.`
                          : loadingRemoteBooks
                            ? "Reading the runner administrator’s approved assets…"
                            : remoteBookAssets.length === 0 && !savedBook
                              ? "No opening books are approved in the runner configuration."
                              : "Only assets approved by the runner administrator are shown."}
                      </small>
                    </div>
                  ) : (
                    <Button
                      variant="secondary"
                      onClick={() => void chooseBook()}
                    >
                      <Upload size={15} /> {bookPath ? "Replace" : "Import"}
                    </Button>
                  )}
                </div>

                <div
                  className={`book-policy ${bookPath ? "" : "book-policy-disabled"}`}
                >
                  <div className="book-enable-row">
                    <div>
                      <strong>Use opening book</strong>
                      <small>
                        Disable this to send every position directly to the UCI
                        engine.
                      </small>
                    </div>
                    <Switch
                      checked={bookEnabled}
                      aria-label="Use opening book"
                      disabled={!bookPath}
                      onCheckedChange={(checked) =>
                        setBookEdit("enabled", checked)
                      }
                    />
                  </div>

                  <label className="book-setting-row">
                    <span>
                      <strong>Maximum book depth</strong>
                      <small>
                        {/*
                         * Clearing the field yields Number("") === 0, which read
                         * as "through ply 0 · approximately move 0" until blur
                         * clamped it. An empty field has no depth to describe.
                         */}
                        {maxPlies >= 1
                          ? `Use the book through ply ${maxPlies} · approximately move ${Math.ceil(maxPlies / 2)}`
                          : "Enter how deep the book may be used"}
                      </small>
                    </span>
                    <input
                      aria-label="Maximum book plies"
                      type="number"
                      min="1"
                      max="200"
                      value={maxPlies}
                      disabled={!bookPath || !bookEnabled}
                      onBlur={() =>
                        setBookEdit("maxPlies", Math.max(1, maxPlies))
                      }
                      onChange={(event) =>
                        setBookEdit(
                          "maxPlies",
                          Math.min(200, Number(event.target.value)),
                        )
                      }
                    />
                  </label>

                  <div className="book-percentage-setting">
                    <div>
                      <span>
                        <strong>Candidate breadth</strong>
                        <small>
                          Randomly choose among the top {topMovePercent}% of
                          moves after ranking by book weight.
                        </small>
                      </span>
                      <b>{topMovePercent}%</b>
                    </div>
                    <div className="book-percentage-presets">
                      {[1, 5, 10, 25, 50, 100].map((percentage) => (
                        <button
                          type="button"
                          className={
                            topMovePercent === percentage ? "selected" : ""
                          }
                          aria-pressed={topMovePercent === percentage}
                          disabled={!bookPath || !bookEnabled}
                          onClick={() =>
                            setBookEdit("topMovePercent", percentage)
                          }
                          key={percentage}
                        >
                          {percentage}%
                        </button>
                      ))}
                    </div>
                  </div>
                </div>

                <div className="config-actions">
                  {savedBook && (
                    <Button
                      variant="danger"
                      disabled={savingBook || clearingBook}
                      onClick={() => setConfirmingRemoveBook(true)}
                    >
                      <Trash2 size={14} />{" "}
                      {clearingBook ? "Removing…" : "Remove book"}
                    </Button>
                  )}
                  <Button
                    variant="primary"
                    disabled={
                      !bookPath || maxPlies < 1 || savingBook || clearingBook
                    }
                    onClick={() => void saveBook()}
                  >
                    {savingBook ? "Validating…" : "Save book policy"}
                  </Button>
                </div>
              </Tabs.Content>

              <Tabs.Content
                value="uci"
                className="config-tab-content uci-config-tab"
              >
                <div className="uci-options-intro">
                  <div>
                    <strong>
                      {options.length} option{options.length === 1 ? "" : "s"}{" "}
                      reported by the engine
                    </strong>
                    <small>
                      Values are validated against the UCI handshake before they
                      are saved.
                    </small>
                    {remoteRunner && (
                      <small className="runner-filesystem-note">
                        Filesystem options such as SyzygyPath must point to
                        files or directories on the remote runner.
                      </small>
                    )}
                  </div>
                  <div className="uci-intro-actions">
                    <Button
                      variant="secondary"
                      disabled={refreshingOptions}
                      onClick={() => void onRefreshOptions(engine)}
                    >
                      {refreshingOptions ? "Probing…" : "Re-probe engine"}
                    </Button>
                    {/* Overwrites every option at once — the same dialog
                        asks before discarding those edits, and Settings asks
                        before its identically-labelled reset. */}
                    <Button
                      variant="secondary"
                      disabled={options.length === 0}
                      onClick={() => setConfirmingReset(true)}
                    >
                      Reset defaults
                    </Button>
                  </div>
                </div>
                <div className="uci-option-list">
                  {options.length === 0 && (
                    <div className="uci-options-empty">
                      This engine did not report configurable UCI options.
                    </div>
                  )}
                  {options.map((option) => (
                    <label className="uci-option-row" key={option.name}>
                      <span>
                        <strong>{option.name}</strong>
                        <small>
                          {option.optionType}
                          {option.min != null && option.max != null
                            ? ` · ${option.min}–${option.max}`
                            : option.defaultValue != null
                              ? ` · default ${option.defaultValue || "empty"}`
                              : ""}
                          {remoteRunner &&
                          option.name.toLowerCase().includes("path")
                            ? " · path on runner"
                            : ""}
                        </small>
                      </span>
                      <UciOptionControl
                        option={option}
                        value={optionValues[option.name] ?? ""}
                        onChange={(value) => setOption(option.name, value)}
                      />
                    </label>
                  ))}
                </div>
                <div className="config-actions">
                  <Button
                    variant="primary"
                    disabled={savingOptions || options.length === 0}
                    onClick={() => void saveOptions()}
                  >
                    {savingOptions ? "Saving…" : "Save UCI options"}
                  </Button>
                </div>
              </Tabs.Content>
            </Tabs.Root>
          </Dialog.Content>
        </Dialog.Portal>
      </Dialog.Root>
      <ConfirmDialog
        open={confirmingDiscard}
        title="Discard your changes?"
        description="The engine settings you edited here have not been saved."
        confirmLabel="Discard and close"
        pending={false}
        onCancel={() => setConfirmingDiscard(false)}
        onConfirm={() => {
          setConfirmingDiscard(false);
          onClose();
        }}
      />
      <ConfirmDialog
        open={confirmingReset}
        title="Reset every UCI option?"
        description={`All ${options.length} option${options.length === 1 ? "" : "s"} go back to the value ${engine.name} reports as its default. Nothing is saved until you press Save UCI options.`}
        confirmLabel="Reset to defaults"
        pending={false}
        onCancel={() => setConfirmingReset(false)}
        onConfirm={() => {
          setOptionEdits(
            Object.fromEntries(
              options.map((option) => [option.name, option.defaultValue ?? ""]),
            ),
          );
          setConfirmingReset(false);
        }}
      />
      <ConfirmDialog
        open={confirmingRemoveBook}
        title="Remove the opening book?"
        description={`${engine.name} plays every position with its own search until a book is configured again.`}
        confirmLabel="Remove book"
        pending={clearingBook}
        onCancel={() => setConfirmingRemoveBook(false)}
        onConfirm={() => {
          setConfirmingRemoveBook(false);
          void clearBook();
        }}
      />
    </>
  );
}

/**
 * The control a single UCI option renders as.
 *
 * Exhaustive over `UciOptionType` with `assertNever`, and the mapping from the
 * engine's free-form type string happens once in `uciControlKind` — an
 * unrecognised type falls back to a text field by rule rather than by a silent
 * `else`.
 */
function UciOptionControl({
  option,
  value,
  onChange,
}: {
  option: UciOption;
  value: string;
  onChange: (value: string) => void;
}) {
  const kind = uciControlKind(option.optionType);
  switch (kind) {
    case "check":
      return (
        <button
          type="button"
          className={`uci-check ${value === "true" ? "selected" : ""}`}
          aria-label={option.name}
          aria-pressed={value === "true"}
          onClick={() => onChange(value === "true" ? "false" : "true")}
        >
          {value === "true" ? "On" : "Off"}
        </button>
      );
    case "combo":
      return (
        <select
          aria-label={option.name}
          value={value}
          onChange={(event) => onChange(event.target.value)}
        >
          {option.choices.map((choice) => (
            <option value={choice} key={choice}>
              {choice}
            </option>
          ))}
        </select>
      );
    case "button":
      return <span className="uci-button-note">Momentary</span>;
    case "spin":
    case "string":
      return (
        <input
          aria-label={option.name}
          type={kind === "spin" ? "number" : "text"}
          min={option.min ?? undefined}
          max={option.max ?? undefined}
          value={value}
          onChange={(event) => onChange(event.target.value)}
        />
      );
    default:
      return assertNever(kind);
  }
}
