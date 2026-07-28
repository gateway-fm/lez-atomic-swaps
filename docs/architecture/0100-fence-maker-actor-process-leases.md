# ADR 0100: fence maker actor process leases

- Status: Accepted; schema-v16 transactional/race foundation GREEN;
  inherited held-lock recovery, physical artifact binding, and atomic ZEC
  acceptance-registration store components plus the real ZEC sealed-config
  consumer GREEN; BTC commitment parity, daemon-owned provisioning,
  semantic config binding, process supervisor, and actual-node composition pending
- Date: 2026-07-28

## Context

Pair actors persist protocol state, intent bytes, observations, and recovery
authority in role-local databases. Those journals answer how one known swap
resumes, but not which accepted maker swaps need workers, which daemon
generation owns an attempt, or whether a retry is queued, backed off, terminal,
or operator-blocked.

A wall-clock lease expiry is unsafe: an old daemon may die while its child
remains alive. A new daemon stealing that lease after a TTL could run two
workers for one swap. Scheduling must not become a second protocol state
machine or source of effect authority.

## Decision

Schema v16 adds `maker_actor_processes`, containing only orchestration metadata:
application swap ID, Bitcoin/Zcash actor kind, immutable config/program paths
and hashes, role-state database path, schedule state, next-attempt time, lease
owner/generation, child PID/start ticks, attempt count, and payload-free failure
class.

The table never stores an agreement, key, transaction, chain observation,
protocol phase, deadline, or effect bytes. Those remain authoritative only in
the pair actor database and SDK journals.

Standalone registration of an already-durable swap is pair-bound and uses an
immediate SQLite transaction for insert-once/exact-replay behavior under
competing connections. The ZEC acceptance API now reuses that insert inside the
existing acceptance transaction: coordinator, binding, agreement, protected
claim material, completed negotiation, consumed offer, replay record, and one
queued immutable actor manifest commit or roll back together. The manifest is
part of the exact replay identity. A changed manifest conflicts, and a missing
or drifted scheduler row on replay fails closed instead of being silently
recreated. The actor's initial due time is deliberately not replay identity;
the scheduler may already have advanced after a lost response.

```mermaid
sequenceDiagram
    participant D as Maker daemon
    participant F as Owner-private filesystem
    participant Q as SQLite schema v16
    participant T as Taker Chat client
    D->>F: Prepare no-clobber maker config and exact hashes
    T->>D: Countersigned final agreement
    D->>Q: BEGIN IMMEDIATE
    D->>Q: Insert swap, agreement, binding, protected claim
    D->>Q: Insert immutable queued actor manifest
    D->>Q: Consume offer and persist exact replay result
    alt every write succeeds
        D->>Q: COMMIT
        D-->>T: Accepted swap ID
    else any write fails
        D->>Q: ROLLBACK all acceptance and scheduling rows
        D-->>T: Fail closed
    end
```

The storage primitive is GREEN. The running daemon still needs a maker-only,
no-clobber provisioner that prepares and semantically validates the exact
config before it calls this mandatory scheduled-acceptance API. Until that
wiring exists, the legacy unscheduled completion method remains only a
migration/test entry point and production handoff is not claimed closed.

An immediate SQLite transaction claims one due row, installs a random 16-byte
owner, increments a monotonic generation, and excludes every other claimant for
that swap. Every resolution requires the exact owner and generation. Distinct
rows may be leased independently.

Stored paths are lexically normalized and distinct. The held-lock component
derives one never-unlinked `<state-db>.maker-actor.lock` beside each role state
database, requires an absolute canonical effective-UID-owned mode-0700 parent,
secure-opens the mode-0600 regular lock with `openat2`, rejects symlinks and
multiple links, and revalidates named/open device and inode identity.

The physical artifact component secure-opens config and program without
following symlinks, requires stable named/open identity, bounded regular files,
single links, trusted ownership and modes, and exact recorded SHA-256 bytes.
It copies those bytes into write-sealed Linux memfds. The child executes only
the sealed program on FD 197, reads the sealed private config on FD 196, and
retains the lock on FD 198. Replacing or mutating the deployment paths after
verification cannot change those child bytes. The state database is rebound at
command handoff as either the same owner-private mode-0600 single-link inode or
the same absent path beneath its mode-0700 parent.

The real ZEC actor accepts exactly one configuration source: its legacy private
path or inherited FD 196. The inherited route is not a general file-descriptor
escape hatch. Before constructing Tokio, the actor reopens the fixed descriptor,
requires a regular anonymous effective-UID-owned mode-0600 inode with link count
zero and a 1-to-64-KiB size, requires `F_SEAL_SEAL`, `F_SEAL_SHRINK`,
`F_SEAL_GROW`, and `F_SEAL_WRITE`, reads from offset zero under that immutable
snapshot, and then applies the same strict config and alias validation as the
path route. It rejects every other descriptor number. This closes the ZEC
consumer half of the capability handoff; the daemon supervisor remains
responsible for comparing the parsed swap, role, state database, and agreement
commitment with the leased manifest before spawn.

```mermaid
sequenceDiagram
    participant S as Store harness or future supervisor
    participant M as Sealed memfd 196
    participant A as ZEC actor process
    S->>M: Copy verified config bytes and apply all four seals
    S->>A: Exec exact actor with config-fd 196
    A->>A: Parse exact descriptor before Tokio
    A->>M: Check owner mode links size and seals
    A->>M: Read immutable bytes from offset zero
    A->>A: Validate schema paths role and agreement binding
    alt any descriptor or binding check fails
        A-->>S: Generic failure and no actor JSON
    else all checks pass
        A-->>S: One command result
    end
```

The future pair adapters must still parse the exact config and prove its
internal role-state path equals the scheduler manifest before spawning. That
semantic check cannot be inferred from a content digest alone and is not moved
into this pair-neutral store layer.

Time never releases `leased`. The daemon first acquires the per-swap exclusive
kernel lock and maps a cloned close-on-exec descriptor to child FD 198. Exact-
pinned `command-fds` 0.3.3 performs the child-only descriptor mapping without a
process-wide close-on-exec race or repository-local unsafe code; its Google
upstream and Apache-2.0 license pass the workspace dependency-policy gate. The
old parent and every inherited child must exit before another daemon can acquire
the lock. A non-cloneable held-lock value then authorizes one immediate SQLite
transaction that replaces owner and increments generation while the row remains
`leased`; no queued/unleased interval is observable. PID/start ticks remain
diagnostic, not fencing authority.

This capability assumes an effective-UID-private local filesystem with working
`flock` semantics. Different-UID service isolation and remote filesystems need
separate production validation; lock files are deliberately never unlinked.

```mermaid
flowchart LR
    AppDB["Application SQLite schema v16"] --> Schedule["maker_actor_processes metadata"]
    Schedule --> Supervisor["Pair-neutral process supervisor"]
    Supervisor --> ArtifactsA["Sealed program FD 197 and config FD 196: A"]
    Supervisor --> ArtifactsB["Sealed program FD 197 and config FD 196: B"]
    Supervisor --> LockA["Inherited lock FD 198: swap A"]
    Supervisor --> LockB["Inherited lock FD 198: swap B"]
    ArtifactsA --> ActorA["Opaque pair actor A"]
    ArtifactsB --> ActorB["Opaque pair actor B"]
    LockA -->|"child FD 198"| ActorA["Opaque pair actor A"]
    LockB -->|"child FD 198"| ActorB["Opaque pair actor B"]
    ActorA --> StateA["Role SQLite A: protocol authority"]
    ActorB --> StateB["Role SQLite B: protocol authority"]
    ActorA --> Nodes["Shared local chain nodes"]
    ActorB --> Nodes
```

## Crash and peer-isolation flow

```mermaid
sequenceDiagram
    participant D1 as Daemon generation 1
    participant Q as Schema-v16 scheduler
    participant A as Actor A
    participant B as Actor B
    participant D2 as Daemon generation 2
    D1->>Q: claim A and B with owner 1
    Q-->>D1: independent generation-1 leases
    D1->>A: spawn with inherited lock A
    D1->>B: spawn with inherited lock B
    A--xD1: result lost or coordinator crashes
    B-->>D1: peer result independently fenced
    D2->>Q: list leased rows
    D2->>A: try lock A
    alt old A still alive
        A-->>D2: kernel lock remains busy
        D2->>Q: leave lease untouched
    else old A is dead
        A-->>D2: kernel lock acquired
        D2->>Q: atomically transfer A to owner 2 and generation 2
        D2->>A: restart same immutable config
    end
    D2->>B: peer row remains independent
```

## Atomicity argument

The scheduler decides only whether and when to invoke an opaque actor. In the
completed design, accepted-swap creation and immutable ZEC actor registration
share one rollback boundary, so a committed scheduled acceptance cannot expose
the missing-row handoff. The primary key, immediate claim, owner/generation
fence, secure physical artifact binding, and inherited kernel lock then prevent
concurrent workers for one swap across a daemon crash. Schema v16 or the
transactional API alone does not make the composed crash-safety claim; daemon
provisioning and execution remain required. The actor's exact
durable intent and observe-before-rebroadcast remain the at-most-once public-
effect boundary. Different swaps use unique rows, configs, state databases,
locks, agreements, escrows, outpoints, and deadlines.

## Consequences

- Store tests prove transactional exact registration, pair binding, stable due
  order, competing-connection same-row exclusion and distinct-row progress,
  restart enumeration, stale-fence rejection, half-open backoff, and peer
  isolation.
- Process-boundary tests prove child inheritance retains the lock after parent
  release, live-child exclusion, exact post-exit recovery, stale-recovery
  rejection, cross-swap rejection, peer immutability, unsafe-parent rejection,
  and hard-link rejection.
- Artifact tests prove the executed program and read config remain the exact
  verified bytes after both deployment paths are replaced. Wrong hashes,
  config symlinks, program hard links, unsafe state modes, and unexpected state
  creation fail closed.
- Atomic acceptance tests force a later mutation failure and observe zero swap,
  agreement, binding, claim-material, actor, and replay rows; success exposes
  exactly one queued row. Exact and delayed replay preserve it, a changed
  manifest conflicts, and a deleted scheduler row fails closed.
- The real ZEC binary test replaces the deployment config after the sealed
  snapshot is created and still reports the snapshot's role/state. An ordinary
  linked file, a memfd missing `F_SEAL_WRITE`, path-plus-FD ambiguity, and any
  descriptor other than 196 fail closed. These tests use only local process and
  kernel primitives; no RPC, node, Docker, faucet, or network participates.
- XMR is not advertised yet because its role process is a multi-command
  ceremony rather than the one-shot Bitcoin/Zcash actor contract.
- Literal coordinator closure still requires daemon-owned maker-only artifact
  provisioning, pair-specific config-to-state validation, the daemon
  supervisor, a real role-process crash, and actual-node overlap evidence.
