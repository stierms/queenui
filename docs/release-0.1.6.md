# QueenUI 0.1.6

Syzygy tablebases keep working throughout long games, even when engines map
more than 32 GiB of table files.

- Linux engines now have a private writable memory budget of their Hash slice
  plus 2 GiB. Read-only shared tablebase mappings do not consume that budget.
  The main thread stack has a separate 16 MiB limit.
- Windows keeps its Job Object committed memory limit, with the same private
  memory headroom. Read-only file views do not consume committed memory.
- Engine mapping failures now produce a persistent runner diagnostics warning,
  including when engine log recording is disabled.
- Includes the Rust and frontend dependency updates from PRs #20 and #21,
  matching the Tauri JavaScript and Rust versions. Updates vulnerable npm
  dependencies and keeps the PGN parser compatible with Rust 1.94.1.

Validation covered a 200 GiB sparse read-only mapping and rejection of an
anonymous allocation above the budget on both Linux and native Windows. A
zigqueen 6.4.0 sweep on oldrig probed 365 legal six-man positions, reached
138.29 GiB of virtual address space, and reported zero tablebase mapping
failures. See [engine memory limits](engine-memory-limits.md) for the measurements
and test commands.

The new limits apply to engines spawned after updating QueenUI or the runner.
