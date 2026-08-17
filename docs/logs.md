# Logs

QueenUI records two very different things under the same page, because they answer two different questions.

- **Engine sessions** — the complete UCI conversation of every game an engine plays. Audience: whoever develops that engine. Question: "what exactly did my engine do on move 23 of that game?"
- **App diagnostics** — QueenUI's own operational events. Audience: the operator. Question: "why did that bot stop last night?"

Both live under `logs/` in the app config directory, beside `queenui.json` and `history.jsonl` (see [persistence](persistence.md)).

```text
logs/index.jsonl                   one session summary per line
logs/diagnostics.jsonl             app diagnostics, append-only
logs/sessions/<session-id>.uci.gz  one game's conversation
```

## Engine sessions

### What is captured

Everything that crosses the process boundary, in both directions: the `uci` handshake and every `setoption`, `position`, `go` and `stop` QueenUI sends; every line the engine writes to stdout, `info` lines included; and the engine's stderr, which used to be discarded outright (`Stdio::null()`) and is exactly where an engine reports a missing network file or a failed assertion.

There is one seam for this. All engine traffic passes through `UciEngine::send` and `UciEngine::next_line` in `src-tauri/src/uci.rs`, so both tee into the recorder and nothing else in the engine layer needs to know that recording exists.

Stderr needs more care than it looks. It is only piped when a recorder exists, because piping a stream nobody drains blocks a chatty engine once the pipe buffer fills — mid-search, in a rated game. For the same reason the drain reads raw bytes and decodes lossily rather than reading lines: one non-UTF-8 byte (a Windows-1252 path in an error message, an assertion dump) would end a line-based reader, leaving the pipe piped and undrained, which is the same deadlock by another route.

### Format

A session file is gzip, and inside it a header block of `# key: value` lines followed by one event per line:

```text
<ms-since-session-start>\t<dir>\t<text>
```

`<dir>` is `>` sent, `<` received, `!` engine stderr, `#` a QueenUI note. Only the first two tabs separate fields, so engine output containing tabs survives intact.

The header is what makes a file worth handing to someone else: engine name, path, size and modification time, the UCI options actually applied, the opening book, the account, opponent, colour, clock, initial FEN, the QueenUI version and the OS. A log without that context is an anecdote.

Notes are the spine of the outline. Each `go` is preceded by `search ply=… move=… color=… wtime=… btime=…`, each result followed by `bestmove uci=… elapsed=…`, and book moves, engine restarts, failed searches and the final status get their own notes. The Logs page parses only these to build the per-move outline, so "jump to move 23" costs one pass over the file rather than a chess engine of its own.

### Why gzip, and why per session

UCI output is about as compressible as text gets — thousands of near-identical `info` lines — so gzip returns roughly 10-15×: a 2.5 MB rapid game becomes ~40 KB, and a thousand games fit in ~40 MB. `flate2`'s `miniz_oxide` backend keeps this pure Rust, and a `.gz` opens in whatever tool the recipient already has.

One file per session (rather than one big log with an offset index) makes export a copy, deletion a delete, and retention a matter of removing whole files. The stream is flushed once per completed move, which bounds a crash to the move in progress and keeps a live session readable while it is still being written. A truncated file decodes to its valid prefix instead of erroring.

### Why no search index

Sorting and filtering run over `index.jsonl`, which is small and loaded into memory like the game history is. Full-text search decompresses only the sessions that survive the metadata filter, at roughly 500 MB/s. Searching a thousand games is a couple of seconds, and the most recently opened session is cached decompressed so paging, outline building and in-session search are instant. An inverted index would be a second thing to keep correct for a workload measured in seconds.

### Not blocking the game

These are real rated games. The writer entry points (`sent`, `received`, `stderr`, `note`) only stamp a timestamp and push onto a channel; a dedicated OS thread does every gzip and file operation. If that thread ever falls far enough behind to queue 50 000 events, further events are dropped and counted in `droppedLines` rather than growing memory without bound — a visibly incomplete log is better than a stalled search or an exhausted heap.

Reading has the same constraint from the other direction. Decoding one session is megabytes of gzip plus a full-file parse, and a cross-session search multiplies that by the archive; a Tauri command body with no await point holds its runtime worker for the whole duration. Every log command therefore runs on a blocking thread, so a multi-second search cannot compete with the game streams and engine pipes this feature exists to observe.

### Retention

Two caps, both configurable in Settings, both enforced together because they fail in different ways: `maxTotalMb` (default 2 GB) bounds a busy campaign, `maxAgeDays` (default 90) bounds a machine that plays rarely. Pruning removes by age first, then oldest-first until the size cap is met, and runs at startup, when a session closes, and when the policy changes. Live sessions are never pruned. Capture can be switched off entirely, in which case no session is opened and stderr goes back to being discarded.

### Single-instance ownership

Each local engine authority acquires an operating-system lock for its own data directory before starting automation. That includes the embedded desktop backend and the standalone runner process: a second authority pointed at the same directory exits before it can drive an account, write an engine session, or mutate the shared indexes. The lock belongs to an open file handle rather than a sentinel PID file, so the operating system releases it automatically after a crash and a replacement authority can recover ownership.

A remote-mode desktop is a client, not a local engine authority, and does not acquire this lock. It dispatches engine-session and index operations to the runner, whose process holds the lock on the runner's data directory. The desktop can still write its own settings, pairing credential, and diagnostics log in its local app-data directory, but it does not create local engine session logs or indexes there.

A live session still has no index record until it closes. After a crash, orphan adoption therefore waits until the file has been unchanged for five minutes, then re-checks an adopted file's modification time before any prune, clear, or delete. This protects the valid prefix of the interrupted recording while normal startup recovery resumes from the same exclusively owned data directory.

### When to revisit

- **Sessions become slow to open** at multi-hundred-megabyte sizes: add a periodic line-offset index inside each file so a page fetch can seek instead of decoding from the start.
- **Cross-session search becomes slow** past a few thousand sessions: parallelise the sweep across cores before considering an index.
- **Ratio matters more than portability**: zstd with a dictionary trained on one engine's output beats gzip substantially on files this repetitive, at the cost of a C dependency.

## App diagnostics

QueenUI reported operational problems with `eprintln!`, which in a bundled Windows build has no console to reach — the messages were written nowhere. They now go to a bounded in-memory ring (1000 entries, what the Logs tab renders) and to `logs/diagnostics.jsonl`, so they survive a restart.

Each entry carries a level (`info` < `warn` < `error`; the filter is a minimum, not an equality), a source (`engine`, `lichess`, `campaign`, `storage`, `app`), the account and game it concerns when it concerns one, a one-line operator-facing message, and the underlying error as detail.

Diagnostics keep their own 90-day horizon rather than following the engine-log age cap. They are a few entries an hour, not a transcript archive, and coupling them would mean shortening how long transcripts are kept silently discarded months of operational history.

The sink is process-global. Some of the most valuable reports come from places that have no access to app state — recovering from a corrupt configuration happens before the state exists — and threading a handle through those would be worse than the problem. It is installed first thing in Tauri setup, and an observer forwards each entry to the UI as `queenui://diagnostic` so the page appends without polling.

High-frequency parse failures are aggregated rather than reported per line: a malformed NDJSON stream produces one "skipped N unreadable entries" diagnostic with the first error as detail, instead of thousands of rows that bury everything else.

## Contracts

`crates/queen-core/src/enginelog.rs` and `crates/queen-core/src/diagnostics.rs` own their own serde types, the same way `crates/queen-core/src/history.rs` owns the Scorebook's. The frontend mirrors them in `src/types/models.ts` and reaches them through the typed wrappers in `src/api/commands.ts` (`list_log_sessions`, `get_log_page`, `get_log_outline`, `search_log_session`, `search_log_sessions`, `export_log_session`, `delete_log_session`, `clear_log_sessions`, `get_logs_overview`, `set_log_retention`, `get_diagnostics`, `clear_diagnostics`) plus two events, `queenui://logs-updated` and `queenui://diagnostic`.
