# Persistence

QueenUI stores durable state in two files next to each other in the app config directory.

## Application config — `queenui.json`

Engines, accounts, campaign settings, and per-engine options. Written atomically (temp file + fsync + rename); a corrupt file is backed up as `queenui.json.corrupt` and replaced with defaults instead of failing startup. Lichess tokens are never stored here. The embedded Windows app uses Windows Credential Manager; runner mode uses one service-user-owned mode-`0600` file per account under `$QUEEN_RUNNER_DATA_DIR/secrets/`. See [Headless runners](runner.md#security-model-and-pairing).

## Game history — `history.jsonl`

One JSON object per line per finished game (`GameRecord` in `crates/queen-core/src/history.rs`), including the optional telemetry block for QueenUI-played games. Append-only, deduplicated by game id, loaded fully into memory at startup; corrupt lines are skipped and counted. All Scorebook aggregation runs in Rust over the in-memory records.

### Why JSONL and not SQLite

The access pattern is append-only writes and load-everything reads, which JSONL serves with zero dependencies, no schema migrations (fields are added with `#[serde(default)]`), crash tolerance by construction, and a greppable, diffable file. The aggregation logic (streaks, lab metrics, config cohorts) is easier to maintain and unit-test as plain Rust than as SQL, and SQLite would not remove that code — it would only narrow what gets loaded.

### When to revisit (triggers)

Move to SQLite (rusqlite) when any of these becomes true:

1. The archive approaches **~50–100k games** — at ~1–3 KB per record that is 100–250 MB loaded at startup and a noticeable parse pause.
2. A feature needs **per-move storage** (full move lists, per-move eval tables, an opening explorer over own games), which multiplies data volume and wants indexed queries.
3. Memory footprint or startup time becomes a complaint in normal use.

### Migration path

`HistoryStore` is the single seam: load, append, and query all go through `crates/queen-core/src/history.rs`. The migration is one rusqlite-backed reimplementation of that module plus a one-time importer that walks the existing `history.jsonl`. Nothing outside the module — commands, stats contract, frontend — changes. Indexes to add at that point: `finished_at_ms`, `account_id`, `engine_id`, `perf`.
