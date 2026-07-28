# ADR 0100: fence maker actor process leases

- Status: Accepted; schema-v16 transactional/race foundation GREEN;
  inherited held-lock recovery component GREEN; acceptance handoff, remaining
  physical artifact checks, process supervisor, and actual-node composition
  pending
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

Registration of an already-durable swap is pair-bound and uses an immediate
SQLite transaction for insert-once/exact-replay behavior under competing
connections. The acceptance transaction is not yet joined to registration;
that handoff must close before the supervisor is wired.

An immediate SQLite transaction claims one due row, installs a random 16-byte
owner, increments a monotonic generation, and excludes every other claimant for
that swap. Every resolution requires the exact owner and generation. Distinct
rows may be leased independently.

Stored paths are lexically normalized and distinct. The held-lock slice now
derives one never-unlinked `<state-db>.maker-actor.lock` beside each role state
database, requires an absolute canonical effective-UID-owned mode-0700 parent,
secure-opens the mode-0600 regular lock with `openat2`, rejects symlinks and
multiple links, and revalidates named/open device and inode identity. The
supervisor must still apply equivalent owner, mode, link, inode, and recorded
SHA-256 checks to the config, program, and state database before complete
physical artifact binding is claimed.

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
    Supervisor --> LockA["Inherited kernel lock: swap A"]
    Supervisor --> LockB["Inherited kernel lock: swap B"]
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
completed design, the primary key, immediate claim, owner/generation fence,
secure physical artifact binding, and inherited kernel lock prevent concurrent
workers for one swap across a daemon crash. Schema v16 alone does not make that
crash-safety claim. The actor's exact
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
- XMR is not advertised yet because its role process is a multi-command
  ceremony rather than the one-shot Bitcoin/Zcash actor contract.
- Literal coordinator closure still requires atomic acceptance registration,
  full artifact validation, the daemon supervisor, a real role-process crash,
  and actual-node overlap evidence.
