# Headless runners

QueenUI can keep the complete Lichess and engine lifecycle on another machine.
The desktop is then a control client: closing it or losing its connection does
not stop the runner's account supervisors, games, campaigns, engine processes,
logs, or history.

## Architecture

The Rust implementation has four boundaries:

- `crates/queen-core` owns configuration, account supervisors, game streams,
  UCI processes, campaigns, opening books, history, logs, and diagnostics. It
  has no Tauri dependency.
- `crates/queen-protocol` owns the versioned request, response, capability, and
  event contracts.
- `crates/queen-runner` hosts one `queen-core` instance behind an authenticated
  HTTP and WebSocket API.
- `crates/queen-client` is used by the Tauri shell. The shell presents exactly
  the same commands whether it embeds `queen-core` or connects to a runner.

The runner is authoritative. Engine paths, opening-book paths, Lichess tokens,
game history, and logs all belong to its filesystem. The desktop stores one
versioned `RunnerIdentity { url, cert_fp, bearer, generation }` record in the
platform credential store. URL, pin, and bearer are never loaded separately.

The desktop also assigns a process-local, monotonically increasing backend
generation whenever it publishes a new active embedded or remote backend.
Runner-connection events and the envelopes carrying snapshot, log, and history
notifications all include that generation, so consumers can reject late data
from an earlier backend. This publication generation is separate from the
persisted runner-credential generation above.

## Security model and pairing

The runner serves TLS on every bind, including loopback. Its self-signed
certificate and private key are created on first initialization as separate
files. On Unix the key is mode `0600` and the public certificate is mode `0644`;
on Windows the separate key file contains only current-user DPAPI ciphertext.
QueenUI does not install this certificate into a public or operating-system root
store: one exact SHA-256 fingerprint of the DER certificate is the complete
trust decision for both HTTPS and WSS.

The certificate is the runner's long-lived identity, not a public-PKI lease;
there is no automatic certificate replacement. Replacing the certificate/key
pair is an explicit identity reset and every desktop must pair again with the
new independently carried fingerprint.

Runner traffic ignores proxy environment variables, does not follow redirects,
and never falls back to public roots, observed pins, plaintext, or an old
credential. Plain HTTP is accepted by the client only for literal
`127.0.0.0/8` or `::1` addresses. A DNS name such as `localhost` does not
qualify. That exception is intended only for a separately authenticated SSH
tunnel; the saved local port is then the endpoint identity, and a hostile local
process taking that port is outside the TLS identity claim.

Configure `QUEEN_RUNNER_PUBLIC_URL` once when starting the runner. It must be
the canonical HTTPS URL the desktop will use. The runner persists this
non-secret setting so the SSH carrier can later run `pair` without reproducing
the service environment. Then mint a ten-minute, one-use setup credential:

```sh
queen-runner pair --print
```

Paste the printed `queenui://pair?v=2&url=…&fp=…&enroll=…` value only into
QueenUI's in-app pairing input. It is not an OS protocol handler or a command-
line argument. QueenUI verifies the imported full pin before it sends any byte
of `enroll`; the runner atomically consumes the hashed enrollment record and
returns a new bearer over that pinned connection. The bearer never crosses the
webview boundary.

The setup code is a credential: whoever redeems a copied unexpired code first
wins. Its one-use property bounds the window but does not make clipboard,
terminal, or SSH output confidential. Use a trusted carrier. Minting a new code
atomically supersedes the previous one; five bad attempts revoke it.

For SSH fetch, QueenUI invokes a fixed trusted OpenSSH binary without a shell,
requires an already-known unchanged host key, bounds execution to 20 seconds
and output to 16 KiB, and parses the payload in Rust. A tunnel remains useful:

```sh
ssh -N \
  -o BatchMode=yes \
  -o ExitOnForwardFailure=yes \
  -o ServerAliveInterval=30 \
  -L 17788:127.0.0.1:7788 runner-host
```

For this deployment set the persisted public URL to
`https://127.0.0.1:17788`; the exact certificate pin, rather than a DNS SAN,
authenticates the runner. Keep the local-port takeover boundary in mind.

For hard-cutover rotation run `queen-runner pair --rotate --print` (or use the
SSH pairing command). Successful redeem mints generation N+1 and revokes
generation N in the same SQLite transaction. There is no grace window. If the
reply is lost, the old bearer is already dead; mint and redeem another rotate
code over the SSH/admin channel. Outstanding rotate codes survive restart,
while expiry, supersession, or five failed attempts leaves the old bearer valid.

The legacy `active-runner` bearer has no authenticated pin and is inert. At
most its URL is shown as an untrusted form hint. Explicit pairing stores the new
identity before deleting the legacy record; explicit Forget deletes both.

Runner-side Lichess tokens use one mode-`0600` file per account beneath the
dedicated service user's data directory. Windows desktop credentials continue
to use Windows Credential Manager. The secret-store interface is deliberately
replaceable if an external secret manager is needed later.

## Trusted-engine mode

The public/default runner mode is `engine_admin: "admin-installed"`. It is a
scope reduction, not an engine sandbox: remote upload and arbitrary-path
registration are absent and their legacy routes refuse at dispatch without
reading a request body. `secure-install` is a reserved value which makes
startup fail until the separate containment design and preflight exist.

An administrator configures dedicated engine roots in
`runner-config.json`. The runner opens each root once as a held directory
authority. The desktop browser sends only `{ rootId, relativePath }`; the
runner rejects parent/absolute/empty components, links, magic links, mount
crossings, overlong/deep paths, untrusted ownership and group/world-writable
directories or files. Enumeration is incremental, paginated, rate/concurrency/
work/time bounded, and never returns an absolute host path.

Registration re-resolves the selection through the held authority and copies
the bytes to `engine-store/<sha256>` using a synchronized temporary file and
atomic installation. Store files are mode `0555`; launches use that store
identity, never the mutable source path. Re-registering changed source bytes
creates another identity and unreferenced identities are collected. Temporary
plus installed bytes share an administrator-capped store quota, and copies are
refused before writing when they would consume the configured free-space
reserve. Startup removes interrupted-copy temporaries before accepting work.

This browser intentionally lets a paired bearer holder enumerate names,
root-relative paths, sizes, mtimes, and executable bits inside every configured
root. Do not place sensitive metadata there. Engines are **not isolated** from
the runner user: a malicious or exploited engine can access that user's files,
runner/Lichess secrets, network, and mutable engine dependencies. The
administrator trust decision is the executable itself. UCI option values sent
to that trusted executable are the operator's decision and are not classified
or administrator-allowlisted by the runner. Opening books remain separately
administrator-allowlisted because the runner opens those files. The environment
allowlist, process-tree cleanup and resource ceilings are hygiene and
availability controls, not confidentiality containment.

### Upgrading a pre-trusted-engine runner

A runner data directory whose `queenui.json` still registers engines by direct
path makes the current runner refuse to start with: “A registered runner engine
predates trusted-engine storage; remove it from config and re-register through a
configured root”. Config-file administration is the supported migration
surface: keep a backup, clear the legacy `engines` entries from
`$QUEEN_RUNNER_DATA_DIR/queenui.json`, then re-register each engine through a
configured root.

## Example LAN integration environment

For example, a Linux host on the LAN with a Ryzen 9 5950X, 32 logical CPUs, and
128 GB RAM can run the runner while the desktop maintains the tunnel. Install
and enable these user services:

```text
runner-host: queen-runner.service
local:       queen-runner-tunnel@runner-host.service
```

Use the local and remote service status commands for checks. There is no
shared bearer file to copy between hosts. Per-account Lichess tokens live under
`$QUEEN_RUNNER_DATA_DIR/secrets/<account-id>.token` (for example,
`~/.local/share/queenui-runner/secrets/example-bot.token`). The runner stores
only bearer hashes in `runner.sqlite3`; the desktop stores the returned
composite identity in its credential store.

Stockfish 17.1 is installed at
`~/.local/lib/queenui/stockfish`. Add its parent directory as a
configured engine root and re-register it through **Browse trusted engines**;
startup deliberately rejects the legacy arbitrary-path registration until that
migration is complete. Configure Threads and Hash in QueenUI based on intended
game concurrency rather than assigning the entire host to every per-game
process.

If the `operator` account has systemd linger disabled, its user service runs
only while that user has a login session. To guarantee runner startup and
survival without any login session, an administrator must run once:

```sh
sudo loginctl enable-linger operator
```

## Desktop workflow

1. Keep the SSH tunnel running when using the tunnel topology.
2. Open Settings → Game runner.
3. Pair by trusted SSH alias or paste a freshly minted v2 payload.
4. Test the pinned connection and save Remote runner mode.

Saving runner settings switches between the embedded local runner and a remote
runner live. Before an embedded-to-remote switch, QueenUI atomically closes
command, game-task, and outgoing-challenge admission and waits for in-flight
commands and reservations. It first refuses locally owned games or known
outgoing challenges. It then asks Lichess for both `nowPlaying` and outgoing
challenges for every enabled account. Any live game or unresolved challenge
refuses the switch, and an API or credential failure fails closed because
QueenUI cannot prove that the handover is safe. Failures before the embedded
core is drained leave the previous backend active. Once the remote intent is
saved and embedded draining begins, bounded-join errors are recorded as
diagnostics and the switch proceeds because the old core has already been
destructively drained. If loading a requested embedded core fails, live remote
control is restored and a restart retries the saved configuration.

Before leaving a remote backend for embedded mode or a different remote URL,
the backend asks the current runner for `handoverInventory`. This protocol query
counts the runner core's live-game ownership (active games, unfinished task
reservations, and durable game intents) and outgoing-challenge ownership rather
than counting presentation rows in a snapshot. If either count is nonzero, the
settings request must explicitly acknowledge the current runner URL reported by
the error; a confirmation naming any other URL is refused against fresh state.
If the runner is unreachable or too old to support the query, acknowledgement
is still required because its ownership cannot be verified. An acknowledged
switch changes only which backend **this desktop** uses. It does not stop the
old runner: its enabled accounts, games, and challenges continue unseen by this
desktop, by design. The inventory is an instantaneous observation of a runner
that deliberately remains live, so its ownership can change immediately after
the response and the acknowledgement is disclosure rather than a quiesce
fence. Do not enable those same Lichess accounts in the embedded data directory;
the one-account-one-authority rule below still applies.

When remote mode is active, **Browse trusted engines** opens the scoped browser
for the administrator-configured roots. Selecting an executable registers the
content-addressed copy after a runner-side UCI probe. The desktop neither knows
nor submits the root's absolute host path. Remote upload and free-form path
registration are deliberately unavailable.

Opening books are runner-opened files. Current runners expose only the
administrator-provided `opening_book_allowlist` entries through an authenticated
asset selector, so the desktop does not require operators to type an exact
runner path. The runner also accepts the exact managed copy already attached to
that engine when only its policy is being edited; no other arbitrary path is
accepted.
UCI option values are different: the runner passes them to the trusted engine
without an administrator value allowlist. Those values are the operator's
decision, while the administrator's trust decision is the executable itself.
Log exports travel in the other direction and are capped before being written
to the path selected on the desktop.

Starting a bot persists its desired enabled state. A restarted headless runner
automatically reconnects enabled accounts; stopping a bot clears that intent.

## Service installation

Build the portable release binary:

```sh
just runner-build
```

Install `crates/queen-runner/target/release/queen-runner` as
`~/.local/bin/queen-runner` and install `deploy/systemd/queen-runner.service`.
Set `QUEEN_RUNNER_PUBLIC_URL` in the unit's environment file for the first
start. The client-side tunnel template is
`deploy/systemd/queen-runner-tunnel@.service`.

The runner reads:

- `QUEEN_RUNNER_LISTEN` — defaults to `127.0.0.1:7788`;
- `QUEEN_RUNNER_DATA_DIR` — absolute data path; defaults to the platform's
  per-user data directory on Unix;
- `QUEEN_RUNNER_PUBLIC_URL` — canonical HTTPS identity URL, persisted on use.

The security and availability settings are config-file-only in
`$QUEEN_RUNNER_DATA_DIR/runner-config.json`; there is no protocol command which
can alter them. A minimal example is:

```json
{
  "engine_admin": "admin-installed",
  "engine_roots": [
    {
      "id": "stable",
      "label": "Stable engines",
      "path": "/opt/queenui/engines"
    }
  ],
  "opening_book_allowlist": ["/opt/queenui/assets/openings.bin"],
  "limits": {
    "normal_commands": 32,
    "query_concurrency": 4,
    "blocking_workers": 4,
    "simultaneous_engines": 8,
    "total_engine_memory_mb": 16384,
    "total_engine_cpu_threads": 16,
    "total_engine_tasks": 256,
    "engine_output_bytes_per_second": 1048576,
    "total_engine_output_bytes_per_second": 4194304,
    "engine_output_total_bytes": 67108864,
    "engine_log_bytes": 2147483648,
    "engine_store_bytes": 8589934592,
    "minimum_free_disk_bytes": 268435456
  }
}
```

Roots and allowlisted assets must be absolute, runner/root-owned, and not
group/world-writable. Restart the service after administrator changes; roots
are intentionally opened only at startup.

Desktop bearer environment overrides were removed. They cannot supply an
authenticated certificate pin and are not a fallback.

## Automation authority

QueenUI holds an operating-system lock for the full lifetime of one data
directory's automation authority. A second desktop or runner process pointed
at that directory exits before account automation can start, and the OS
releases the lock after a crash or after the desktop switches its automation
authority to a remote runner. This is not an exclusive lock over every advisory
write in the directory: the desktop's process-global diagnostics log remains
there and can continue writing while remote mode is active. Configuring the
same Lichess BOT account in two different data directories is deliberately
unsupported, not presented as a distributed
lease: separate machines cannot enforce one without an authoritative
Lichess-side primitive. Each BOT account must therefore appear in exactly one
QueenUI data directory. Two instances driving one account would submit
conflicting moves and race each other on the Lichess API — nothing inside
QueenUI can referee that, so the one-account-one-authority rule is on the
operator.

## Remaining hardening

- Persist active campaign intent, not only account intent.
- Add multi-runner scheduling only when one authority genuinely needs several
  compute workers; the current protocol intentionally controls one runner.

The durable request/retry contract is specified in
[`runner-protocol.md`](runner-protocol.md).
