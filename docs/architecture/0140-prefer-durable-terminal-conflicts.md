# ADR 0140: Prefer durable terminal conflicts

- Status: Accepted; process, race, and runner-envelope regressions GREEN
- Date: 2026-08-03
- Scope: M6 Taker Claim-versus-Refund decision precedence
- Extends: ADRs 0137 and 0139

## Context

Fresh run `m6refund538c629a` proved the 40-second outer action budget by
returning one durable Refund commit at generation two. The runner immediately
sent an opposite Claim to prove mutual exclusion. The service returned generic
action-unavailable because it checked the actor's current advertised action
before reading the existing Refund authorization.

The registry already guarantees one irreversible terminal winner, but its
atomic concurrent-winner error was also mapped to unavailable. Both paths hid
the stronger durable fact from the caller. The run was stopped and
quarantined; its funds and swap are not reused.

## Decision

While holding the per-swap actor lock and validated receipt custody, terminal
requests resolve in this order:

1. look up the exact request, action, swap, and generation for replay;
2. if another sole durable terminal authorization exists, return the fixed
   action-conflict result;
3. only with no durable authorization, read actor status and validate the
   expected generation and currently available action;
4. atomically admit the new action; and
5. execute the role actor while retaining the lock and custody checks.

If another process wins between steps three and four, the registry's
`ActionGenerationConflict` maps to the same action-conflict result. New
admission is not moved before actor validation.

## Components and decision flow

```mermaid
flowchart TD
    Request["Claim or Refund request"] --> Lock["Receipt validation and per swap lock"]
    Lock --> Replay{"Exact durable replay exists"}
    Replay -->|"yes"| Reconcile["Reconcile same actor effect"]
    Replay -->|"no"| Winner{"Any terminal winner exists"}
    Winner -->|"yes"| Conflict["Return fixed action conflict"]
    Winner -->|"no"| Status["Read actor generation and available action"]
    Status -->|"invalid"| Unavailable["Return generation or availability error"]
    Status -->|"valid"| Admit["Atomic registry admission"]
    Admit -->|"concurrent winner"| Conflict
    Admit -->|"won"| Effect["Execute role actor and revalidate custody"]
```

## Concurrent sequence

```mermaid
sequenceDiagram
    participant C as Claim caller
    participant F as Refund caller
    participant S as Taker service
    participant R as SQLite action registry
    participant A as Role actor

    C->>S: Claim at generation G
    F->>S: Refund at generation G
    S->>R: Exact replay and winner reads
    S->>A: Validate generation and action under lock
    S->>R: Admit Claim atomically
    R-->>S: Claim wins
    S->>A: Execute Claim
    S->>R: Admit or observe Refund
    R-->>S: Existing irreversible winner
    S-->>F: Fixed action conflict
    S-->>C: Claim commit or exact replay
```

## Atomicity and consequences

The precheck is read-only and improves error truthfulness; it does not replace
the registry's unique terminal-action constraint or atomic admission. A stale
read therefore cannot authorize both Claim and Refund: one insert wins, and the
loser observes the same fixed conflict. Exact replay remains idempotent and
reconciles only the already admitted action.

The public JSON-RPC error envelope is also fixed:
`error.data.category == "taker_action_conflict"`. Evidence consumers must
inspect the category field rather than treating `error.data` as a scalar.

```mermaid
flowchart LR
    Winner["Durable terminal winner"] --> Service["Taker service"]
    Service --> Envelope["JSON RPC error data category"]
    Envelope --> Runner["E2E evidence assertion"]
```

Fresh actual-node run `m6refund9e84d76a` proved that flow through the Refund
commit and opposite-Claim conflict. The protocol result was correct, but the
runner used the obsolete scalar assertion and stopped before exact replay. The
effect-bearing run is quarantined. The focused runner contract now locks the
object-shaped envelope; a fully fresh run remains required for the Refund
certificate.

## Rejected alternatives

- Consulting actor availability before the durable winner would return a weaker
  and transient result for a settled terminal decision.
- Moving admission before actor validation would persist requests that were
  never actionable at the requested generation.
- Treating `error.data` as an unstructured scalar would diverge from every
  service error emitted by the shared RPC helper.
