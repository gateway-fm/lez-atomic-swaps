# ADR 0139: Bound service actions above actor bridges

- Status: Accepted; focused runner contract GREEN
- Date: 2026-08-03
- Scope: M6 ZEC terminal-action liveness and bounded cancellation
- Extends: ADRs 0137 and 0138

## Context

A fresh isolated Refund journey reached `refund_available`, both actors were
at `both_legs_locked`, and the finalized LEZ timestamp exceeded the signed
refund deadline. The owner service then began the generation-fenced Refund,
but its Unix-socket client stopped waiting after 15 seconds. ADR 0138 had
correctly raised the actor's inner LEZ bridge budget to 30 seconds because one
historical account read alone measured about 9.78 seconds.

The outer caller therefore had a shorter deadline than the operation it asked
the actor to perform. The run stopped after two Taker LEZ submissions
(Initialize and Fund); no third Refund submission appeared before cleanup.
Retrying the spent swap is forbidden, so the live correction requires fresh
identities, chain allocations, and output roots.

## Decision

Read-only and normally local service calls retain a 15-second query budget.
Terminal Claim and Refund calls receive a finite 40-second action budget. The
service RPC helper accepts the explicit budget, caps it to the unchanged
monotonic corridor deadline, and retains the one-second Unix-socket connection
bound.

Forty seconds strictly dominates the generated actor's 30-second bridge
request while retaining ten seconds for process scheduling, JSON-RPC framing,
durable service admission, and response delivery. It does not extend the
signed chain deadline, retry a submission, or permit work beyond the corridor
clock.

## Components and clocks

```mermaid
flowchart LR
    User["Taker mini app or runner"] --> Client["Owner Unix socket client"]
    Client -->|"queries at most 15 seconds"| Service["Taker service"]
    Client -->|"terminal action at most 40 seconds"| Service
    Service --> Registry["Generation fenced SQLite admission"]
    Service --> Actor["Role fixed ZEC actor"]
    Actor -->|"bridge at most 30 seconds"| Sidecar["LEZ sidecar"]
    Sidecar --> Indexer["Isolated LEZ finalized indexer"]
    Client -.->|"all capped by 130 second Refund corridor"| Clock["Monotonic run clock"]
```

## Refund sequence

```mermaid
sequenceDiagram
    actor U as Taker user
    participant C as Owner client
    participant S as Taker service
    participant R as Action registry
    participant A as ZEC actor
    participant L as LEZ sidecar

    U->>C: Refund expected generation G
    C->>S: Refund with 40 second outer budget
    S->>R: Atomically admit Refund at G
    R-->>S: Durable one winner decision
    S->>A: Drive admitted Refund
    A->>L: Observe or submit with 30 second bridge budget
    alt Completed within actor budget
        L-->>A: Pinned evidence or transaction result
        A-->>S: Durable actor result
        S-->>C: Commit or exact replay
    else Actor or corridor budget expires
        A-->>S: Typed unavailable or uncertain
        S-->>C: Bounded failure
    end
```

## Atomicity and consequences

The service still commits Claim-versus-Refund selection before actor I/O.
Actor and sidecar journals still own effect idempotency and uncertain-send
reconciliation. Enlarging only the caller wait cannot create a second effect;
