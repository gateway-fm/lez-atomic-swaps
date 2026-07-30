# ADR 0116: Run bounded independent maker workers

- Status: Accepted for the M5 daemon concurrency component
- Date: 2026-07-30

## Context

The maker daemon previously opened one actor-supervisor store connection and ran
one blocking scheduling loop. Two durable actors were isolated, but a slow actor
serialized every peer. RFP-003 R5 requires concurrent swaps with independent
state, escrow, and deadlines; the single loop could not satisfy the application
part of that requirement.

## Decision

Expose `--actor-worker-count` only with `--actor-supervisor`, bounded to 1 through
32 and defaulting to one. Startup performs the existing exhaustive abandoned-
lease recovery once before readiness. The daemon then opens one WAL SQLite
connection per worker, reuses one random daemon lease-owner identity, and runs
exactly that many scoped OS threads under one aggregate blocking task. Each
worker uses the existing per-row CAS, generation fence, per-swap kernel lock,
bounded child process, and shared cancellation signal.

```mermaid
flowchart TB
    Owner[Maker operator] -->|bounded CLI flags| Daemon[Maker daemon]
    Daemon --> Recovery[Single pre-readiness recovery pass]
    Daemon --> Aggregate[Scoped worker aggregate]
    Aggregate --> W1[Worker 1 and SQLite connection]
    Aggregate --> W2[Worker 2 and SQLite connection]
    Aggregate --> WN[Worker N and SQLite connection]
    W1 --> Store[(WAL maker database)]
    W2 --> Store
    WN --> Store
    W1 --> A1[Swap A actor and kernel lock]
    W2 --> A2[Swap B actor and kernel lock]
```

## Overlap and failure sequence

```mermaid
sequenceDiagram
    participant D as Maker daemon
    participant W1 as Worker 1
    participant W2 as Worker 2
    participant S as SQLite WAL
    participant A as Slow actor A
    participant B as Terminal actor B
    D->>S: recover abandoned leases before readiness
    D->>W1: start with connection and shared owner
    D->>W2: start with connection and shared owner
    par Independent claims
        W1->>S: CAS claim A generation 1
        W2->>S: CAS claim B generation 1
    end
    W1->>A: status under A kernel lock
    W2->>B: status under B kernel lock
    Note over A,B: both attempts overlap
    B-->>W2: valid terminal projection
    W2->>S: fenced Terminal resolution
    Note over W1,S: A remains live and Leased
    A-->>W1: nonzero exit after test release
    W1->>S: fenced Backoff resolution
    D->>W1: shared cancellation on SIGTERM
    D->>W2: shared cancellation on SIGTERM
    D->>D: join every scoped worker before exit
```

## Atomicity and isolation argument

Concurrency does not create a cross-chain transaction. It preserves local
authority by composing existing independent linearization points:

```mermaid
flowchart LR
    Due[Due row] --> CAS{CAS queued to leased}
    CAS -->|winner| Fence[Owner plus generation fence]
    CAS -->|loser| Retry[Observe another due row]
    Fence --> Lock[Per-swap kernel lock]
    Lock --> Child[One bounded actor child]
    Child --> Resolve[Atomic fenced resolution]
    Resolve --> Terminal[Terminal or Backoff]
    Other[Other swap row and lock] -. disjoint authority .- Fence
```

One worker cannot claim a row already leased by another. An actor child inherits
only its own kernel-lock descriptor; cancellation kills and reaps the exact
process group and clears the fenced child identity before shutdown completes.
One worker exit or panic cancels every peer, and the aggregate joins every thread
before returning. The database remains the durable authority; worker count does
not partition or duplicate state.

## Evidence and limits

The black-box daemon journey proves a terminal actor completes while a disjoint
actor is simultaneously live and leased. Releasing the second actor to a typed
failure moves only it to Backoff. Both retain distinct manifests/state paths,
one attempt and generation, exact child cleanup, responsive owner health,
restart equality, and no new invocation. The deterministic journey passed ten
of ten repetitions in 0.49 to 0.54 seconds with no observed flake.

No chain RPC, Docker service, faucet, DNS, public network, or funds participate.
This closes daemon worker concurrency, not full R5: accepted application
agreements, distinct escrows/deadlines, actual chain effects, arbitrary load,
and crash/reorg stress remain separate gates.
