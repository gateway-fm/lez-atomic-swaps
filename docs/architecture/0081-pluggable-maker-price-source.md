# ADR 0081: Read maker prices through a pluggable source boundary

Status: Accepted; local and isolated C-API process adapters GREEN — 2026-07-28

## Context

M5 requires both operator-configured prices and a Logos-module C-API price
source. Offer publication must consume one validated contract instead of
depending directly on SQLite or a foreign ABI.

## Decision

The maker daemon owns a synchronous `PriceSource` boundary. Its local adapter
reads the exact route from the already-locked authoritative SQLite owner and
returns a reduced integer ratio, the source record revision, and daemon-trusted
observation time. It never substitutes another pair or direction.

```mermaid
flowchart LR
    Operator["Maker operator"] --> CLI["lez-maker quote"]
    CLI -->|"owner-local JSON-RPC"| Daemon["lez-maker-daemon"]
    Daemon --> Port["PriceSource"]
    Port --> Local["LocalPriceSource"]
    Local --> DB[("SQLite schema v12")]
    Port -.-> Parent["Bounded process adapter"]
    Parent -->|"exact path, route, time; bounded JSON"| Worker["one-shot price worker"]
    Worker -->|"libloading in only unsafe crate"| Module["pinned Logos module .so"]
    Module -.-> Worker
    Daemon --> Offer["Future signed offer publisher"]
```

The checked-in provisional ABI v1 uses only fixed-width `repr(C)` values, an
explicit version symbol, exact integer ratios, a nonzero source revision and
observation time, route echo, and reserved-zero fields. A one-shot worker loads
the module through `libloading`, copies the response, validates structure,
route, units, revision, bounds and freshness, and emits bounded typed JSON. The
ABI receives no database handle, wallet key, signing key, socket, or fund-moving
authority. Native abort is process-contained. The parent now enforces owner-only
real-file paths, single links, non-writable artifacts, a pinned module SHA-256,
pre/post-call revalidation, a five-second hard maximum, exact-child kill/reap,
an empty environment, null input/diagnostics, and a 4 KiB output ceiling.

```mermaid
sequenceDiagram
    actor Operator
    participant CLI as Maker CLI
    participant Daemon as Maker daemon
    participant Source as Selected price source
    participant DB as SQLite
    participant Worker as One-shot worker
    participant Module as Logos module

    Operator->>CLI: quote exact pair and direction
    CLI->>Daemon: maker_price_quote route
    Daemon->>Daemon: Read trusted Unix time
    alt Local source
        Daemon->>Source: quote route and trusted time
        Source->>DB: Read exact route price
        DB-->>Source: Integer ratio and revision
        Source-->>Daemon: Quote and observation time
    else Logos C-API source
        Daemon->>Worker: Exact route and trusted time
        Worker->>Module: ABI v1 request
        Module-->>Worker: Status and fixed-width response
        Worker->>Worker: Validate ABI, route, ratio, revision, time
        Worker-->>Daemon: Typed JSON or failed process
    end
    Daemon-->>CLI: Typed quote
```

The current daemon serializes this bounded read with its store mutex. The trait
does not require `Sync` because `rusqlite::Connection` is not `Sync`; a future
dedicated persistence actor may move the boundary without changing callers.

## Atomicity and nonclaims

Process isolation preserves daemon memory/state atomicity when a foreign module
aborts, hangs, or returns malformed data: no quote or store mutation is
produced. Pre/post artifact hashing detects ordinary mutation, but the same-UID
boundary is crash containment rather than an OS security sandbox; a malicious
same-UID replace-and-restore race remains production hardening.

The quote is one SQLite snapshot read and identifies its source revision, so an
offer publisher can bind the exact price record it observed. This does not
atomically bind a quote to a future offer or chain effect. Offer publication
must persist its own immutable price and policy revisions in one transaction,
and cross-chain atomicity remains the responsibility of the pair protocol.

## Evidence

Unit tests prove exact-route lookup, exact integer preservation, revision
reporting, trusted-time propagation, and fail-closed missing-route behavior.
The black-box operator journey configures a 5:2 ZEC route, kills and restarts
the daemon, and obtains the same revision-1 quote through the real CLI and Unix
socket.

Four actual-C process tests compile versioned shared-library fixtures and prove
one exact successful quote, typed missing/unavailable results, ABI and symbol
mismatch rejection, route/ratio/revision/time/reserved-field rejection, and
native-abort containment. All-target tests, strict Clippy and warning-fatal
Rustdoc pass for the boundary crate.

Four parent-process tests additionally prove exact domain conversion, typed
unavailability without substitution, abort and hang containment, exact timeout
reap, oversized output rejection, and mutation/mode/hard-link/hash rejection.

The local implementation is one complete M5 adapter. The worker plus bounded
parent are GREEN sub-slices of the second adapter; daemon/store wiring, revision
anti-rollback, retry identity, and atomic signed-offer binding remain open.
LOGOS-021 still prevents eventual-upstream ABI compatibility claims.
