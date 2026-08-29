# Queen runner protocol v2

## Transport identity

Authenticated requests use either literal-loopback cleartext (for a separately
authenticated local/SSH channel) or HTTPS/WSS under QueenUI's exact-certificate
rustls verifier. Runner HTTPS disables redirects and proxy inheritance and has
no public-root, observed-pin, downgrade, or legacy-credential fallback.

`POST /v2/pair/redeem` accepts the one-time enrollment code only after the TLS
handshake has matched the independently imported SHA-256 DER fingerprint. All
other runner operations authenticate a bearer and resolve its credential
generation before idempotency admission.

## Durable idempotency

Every command key is bound to:

- the authenticated bearer generation;
- protocol version;
- HTTP method and normalized path;
- the SHA-256 digest of the exact serialized command body.

The runner authenticates and validates the request before inserting a unique
`pending` reservation in SQLite. A same-binding duplicate waits at most 30
seconds. If the original is still running it receives `202` with
`{ protocolVersion, requestId, status: "pending" }` and may poll by resending
the same request and key. A different binding under the same key and credential
generation receives `409`. A terminal same-binding retry receives the original
bounded typed response.

The client-visible safety horizon is 24 hours from first submission. Within 24
hours, retrying the same key is deduplicated or replayed. After that TTL the key
is a new request. A client must not retry a mutating command across that
boundary without first re-reading authoritative state. Credential rotation
makes all rows from the old generation dead even before their TTL expires.

Each command uses the following persisted state machine:

```text
pending -> done(outcome)
pending -> failed_deterministic(outcome)
pending -> failed_transient  [row deleted; retry executes fresh]
pending -- process crash --> ambiguous_crash
ambiguous_crash -- startup reconciliation --> done | failed_deterministic
```

Startup changes every interrupted `pending` row to `ambiguous_crash` before it
accepts commands. After QueenUI's core startup reconciliation checks Lichess
account/challenge state and the engine process table, any completion that still
cannot be proven becomes a persisted deterministic failure rather than being
silently executed again.

Command reconciliation families are explicit:

| Commands                                          | Startup authority                                 |
| ------------------------------------------------- | ------------------------------------------------- |
| account, bot, challenge, campaign, history import | Lichess account/challenge/history state           |
| engine register/remove/options/opening book       | engine registry, content store, and process table |
| log and diagnostic operations                     | runner log/diagnostic state                       |

## Trusted engine namespace

`GET /v2/engines/roots` returns administrator-chosen IDs and labels only.
`POST /v2/engines/browse` accepts a root ID, relative directory, optional
bounded page size, and opaque single-use cursor. Entries contain root-relative
metadata only. `registerEngine` accepts the root ID and relative file again;
browsing is not authorization, so registration fully re-resolves it through
the held root and installs the exact bytes as `engine-store/<sha256>`.

`POST /v2/engines/upload` and the legacy
`POST /v2/engines/register-path` are fixed dispatch refusals in the only
available `admin-installed` mode. Their handlers have no body extractor.

The database runs in WAL mode with a five-second busy timeout, admission-time
cleanup, a 10,000-row quota, 64 MiB aggregate stored-response quota,
256 KiB per-response bound, and a per-credential limit of 240 new keys per
minute. A pending row reserves the full per-response allowance until its actual
bounded outcome is stored, so concurrent completion cannot overrun the byte
cap. Quota and rate checks happen inside the same immediate transaction as
reservation.

This contract is durable retry deduplication, not indefinite authenticated
freshness. Pinned transport retires the LAN capture/replay path; requests may
execute again after the documented TTL or under a new credential generation.

## Availability lanes

Normal mutation admission, browser work, the reserved single upload lane,
blocking work, and queries have separate server-owned bounds. Account
lifecycle changes are owned by per-account actors. Stop/cancel uses the
actor's priority channel and never acquires normal admission; it signals
cancellation before bounded joins. Query inputs, rows, response bytes,
concurrency and wall time are capped. Log export is decoded through a byte cap.
Engine concurrency, Hash/RSS memory, virtual address space (Hash budget plus a
fixed tablebase mapping headroom — `RLIMIT_AS` is VAS, not RAM), configured CPU
threads, descendant
task count, per-engine and aggregate output rate, per-engine output bytes,
stored log bytes, and content-store temporary/installed bytes have server-owned
ceilings plus a disk free-space reserve. Output controls are sanitized and an
over-limit engine is killed immediately through process-tree cleanup. Because
trusted-engine mode has no upload ingress, the reserved upload lane and upload
temporary-byte use remain zero in production.
