# Engine memory limits and Syzygy mappings

QueenUI budgets Hash separately from engine overhead. The private-memory ceiling
for each newly spawned engine is:

```text
(total_engine_memory_mb * 1 MiB / simultaneous_engines) + 2 GiB
```

For oldrig's configured 16 GiB Hash budget and eight engines, this is 4 GiB per
engine. The aggregate Hash ceiling stays 16 GiB; total private-memory allowances
are 32 GiB across eight processes. Read-only tablebase pages are shared through
the page cache, reclaimable, and excluded from this allocation ceiling. RSS is
not the quantity bounded by either platform's limit.

## Platform accounting

Both [Stockfish](https://github.com/official-stockfish/Stockfish/blob/master/src/syzygy/tbprobe.cpp)
and [Fathom](https://github.com/jdart1/Fathom/blob/master/src/tbprobe.c) map the
entire table file shared read-only on first access. They retain successful
mappings until tablebase teardown or reinitialization. Current upstream
Stockfish and Fathom exit on mapping failure; the measured zigqueen integration
continues without the table. The diagnostics matcher covers both behaviors.

Linux uses `RLIMIT_DATA`, with equal soft and hard limits. Since Linux 4.7, it
checks `brk`, `sbrk`, and data `mmap` allocations. The kernel's
[`may_expand_vm`](https://github.com/torvalds/linux/blob/v6.6/mm/mmap.c) checks
`mm->data_vm` against this limit;
[`is_data_mapping`](https://github.com/torvalds/linux/blob/v6.6/mm/internal.h)
classifies private writable mappings that lack the stack flags. Consequently,
`MAP_SHARED | PROT_READ` file mappings do not count. Read-only private file maps
also do not count. Ordinary pthread stack mappings count as data; the main
`VM_GROWSDOWN` stack is accounted separately and QueenUI caps it at 16 MiB with
`RLIMIT_STACK`. See also
[`getrlimit(2)`](https://man7.org/linux/man-pages/man2/getrlimit.2.html).

QueenUI no longer sets `RLIMIT_AS` on Linux. Any limit inherited from a service
manager or shell still applies. The mmap allocation guard requires Linux 4.7 or
later with normal data-limit enforcement (`ignore_rlimit_data` disabled).
This is an allocation guard, not an RSS or cgroup limit: shared anonymous
mappings and kernel-accounted stack mappings are outside `RLIMIT_DATA`.
The existing address-space fallback on other Unix platforms is not covered by
these Linux/Windows guarantees.

Windows retains `JOB_OBJECT_LIMIT_PROCESS_MEMORY`, applying the same private
budget. Microsoft defines
[`ProcessMemoryLimit`](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_extended_limit_information)
as a limit on virtual memory **commitment**, not all reserved virtual address
space. The native regression below proves that a 200 GiB `PAGE_READONLY` /
`FILE_MAP_READ` file view survives a 2,080 MiB job limit while excessive
`VirtualAlloc(MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE)` fails. A control child
without QueenUI's job limit must successfully commit the same amount, excluding
system-wide commit exhaustion as a false positive. Windows copy-on-write views
have different commitment accounting; this result concerns read-only views.

## Measurements on 2026-10-12

Measured the immutable zigqueen 6.4.0 engine-store binary
`403d672e5c5f8865ea9123838591a5f5cfeccca4777f20fd6356bb58c62b458d`
and Stockfish 19's Linux universal binary, copied into QueenUI's ignored
`artifacts/resource-limits/` directory. No zigqueen sources or live runner
processes were changed.

The measurement host was x86-64 WSL2 Linux 6.6.87.2. Each separate engine
completed `uci`, configured Threads and Hash, `isready`, `ucinewgame`, another
`isready`, then `position startpos` / `go depth 14` and cooperative `quit`.
Sampled `/proc/<pid>/status` every 5 ms through startup, option changes, and
search. These are observed peaks, not guaranteed maxima for every position.

| Engine         | Threads | Hash MiB | Peak VmData MiB | Peak above Hash MiB |
| -------------- | ------: | -------: | --------------: | ------------------: |
| zigqueen 6.4.0 |       1 |      256 |          502.14 |              246.14 |
| zigqueen 6.4.0 |       1 |     2048 |         3126.14 |             1078.14 |
| zigqueen 6.4.0 |       2 |      256 |          982.80 |              726.80 |
| zigqueen 6.4.0 |       2 |     2048 |         3759.14 |             1711.14 |
| Stockfish 19   |       1 |      256 |          310.43 |               54.43 |
| Stockfish 19   |       1 |     2048 |         2102.43 |               54.43 |
| Stockfish 19   |       2 |      256 |          342.56 |               86.56 |
| Stockfish 19   |       2 |     2048 |         2134.56 |               86.56 |

The largest observed overhead was 1,711.14 MiB, including transient allocation
peaks while configuring and searching. The fixed 2,048 MiB headroom leaves
336.86 MiB (19.7%) above that peak. The separate main stack peaked at 780 KiB
for zigqueen and 136 KiB for Stockfish, below the 16 MiB stack ceiling. These
settings cover oldrig's configured engines (Hash 256 MiB, Threads 1) and its
per-engine configuration ceilings (Hash 2,048 MiB, Threads 2). Engines with
larger networks or higher thread ceilings need their own overhead measurements.

Read-only inspection of four live oldrig engines found VmData of 516,872 to
519,532 KiB while VmSize was 3,308,944 to 28,669,184 KiB. This agrees with private
allocation staying roughly constant as table mappings accumulate.

## Regression tests

The regular queen-core UCI tests compile a child fixture and start it through
`UciEngine::start_governed`. With 32 MiB Hash plus 2 GiB headroom, the child:

1. Creates a 200 GiB sparse file and maps it shared read-only. On Windows it
   marks the file sparse with `FSCTL_SET_SPARSE` before extending it, then uses
   `CreateFileMappingW` and `MapViewOfFile`.
2. Reads its first and last bytes, touching only two pages.
3. Successfully allocates 16 MiB privately.
4. Fails to allocate more than the full private budget (`ENOMEM` on Linux).
5. Unmaps and removes the sparse file.

An ungoverned control allocation verifies the oversized request could otherwise
succeed. All allocation probes occur after the UCI handshake so the Windows
child has been assigned to the actual runner Job Object. The test runs on
64-bit Linux and Windows and creates no resident 200 GiB allocation.

```sh
cargo test --manifest-path crates/queen-core/Cargo.toml --lib uci::tests -- --nocapture
```

The stderr regression sends 206 Fathom failures, invalid UTF-8, and another mmap
error through a real child. It expects exactly one persistent warning per
engine process, both with and without flight recording. With recording enabled,
all 206 original lines are retained and the warning includes the account and
game IDs. Warnings always identify the engine executable and the first failed
mapping. An oversized stderr line also trips the output ceiling without
recording enabled. Repeated failures cannot evict unrelated diagnostics from
the bounded ring.

## Full-set integration check

The manual integration test uses the production engine spawn and limits, sends
`go depth 1` for every generated six-man position, checks mapping warnings, and
requires VmSize to exceed the former 34 GiB cap. Generate legal positions with
python-chess, then opt into the ignored Rust test:

```sh
python3 scripts/generate-syzygy-positions.py /path/to/syzygy/6 > positions.tsv
QUEENUI_SYZYGY_TEST_ENGINE=/path/to/zigqueen-6.4.0 \
QUEENUI_SYZYGY_TEST_PATH=/path/to/syzygy/3-4-5:/path/to/syzygy/6 \
QUEENUI_SYZYGY_TEST_POSITIONS="$PWD/positions.tsv" \
cargo test --manifest-path crates/queen-core/Cargo.toml --lib \
  uci::tests::full_syzygy_sweep_under_runner_limits -- --exact --ignored --nocapture
```

Alternatively, the generator accepts `--table-list` with one material name per
line, allowing positions to be generated elsewhere when the full-set host does
not have python-chess installed.

Ran the test on oldrig (Linux 7.0.0-31-generic) against the engine-store binary
above and `/home/mstie/zq/syzygy/3-4-5:/home/mstie/zq/syzygy/6`. Its full set
contained 1,020 files totaling 161,209,573,952 bytes (150.14 GiB). Executed a
stripped queen-core test binary and the positions from in-memory file
handles, at niceness 15 on one CPU, without deploying or restarting the runner.
Only the isolated validation child received these new limits.

| Result                               |                        Value |
| ------------------------------------ | ---------------------------: |
| Six-man positions searched           |                          365 |
| Read-only `.rtbw` / `.rtbz` mappings |                          833 |
| Mapping failure warnings             |                            0 |
| RLIMIT_DATA soft and hard            |  4,294,967,296 bytes (4 GiB) |
| RLIMIT_STACK soft and hard           |    16,777,216 bytes (16 MiB) |
| RLIMIT_AS                            |        unlimited (inherited) |
| Final VmSize                         | 145,006,236 KiB (138.29 GiB) |
| Final VmData                         |     527,752 KiB (515.38 MiB) |
| Final RSS                            | 1,050,068 KiB (1,025.46 MiB) |
| Elapsed test time                    |                 3.70 seconds |

A legal position per WDL table does not necessarily fault every DTZ file: drawn
positions can skip DTZ probing. The 833 mappings still exercise accumulation
well beyond the former cap. The test child exited cooperatively; existing bot
engines and the runner retained their original limits.

Final automated verification passed 189 queen-core tests (plus this separately
run integration test), 44 queen-runner tests, and all 16 native Windows UCI
tests. Linux core/runner and native Windows core Clippy checks passed with
warnings denied. The Windows allocation control succeeded, and the governed
private commitment failed with error 1455 while the 200 GiB view remained usable.
