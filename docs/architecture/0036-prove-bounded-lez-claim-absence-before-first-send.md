# ADR 0036: Prove bounded LEZ claim absence before the first send

Status: Accepted and implemented through the reference actor in pushed commit
`66d352f`. Reproducible actual-node execution through the public actor processes
remains pending.

## Context

The actor must reconcile a durable public LEZ claim with chain truth before it
may consume its one permitted send. A missing transaction in a partial indexer
history, an immature requested window, a moving finalized tip, or an ambiguous
transport outcome is not proof that the claim is absent. Treating any of those
states as absence could duplicate an effect after restart.

The existing finalized claim observer returned only affirmative found facts.
That is sufficient for lifecycle projection but cannot safely distinguish the
one initial submission case from temporary unavailability.

## Decision

The LEZ bridge exposes an additive strict
`classify_finalized_witnessed_claim` method. Its actor-facing result has four
classes:

- `PresentExact` carries the complete validated canonical claim facts;
- `NotFound` carries the exact completely scanned window and stable finalized
  tip;
- `Unavailable` identifies missing node finality/history or a moving tip; and
- `Uncertain` identifies timeout or transport ambiguity.

Only `NotFound` satisfies the chain-absence precondition for an initial send.
It does not itself authorize submission. The caller must also win the separate
`Prepared` to `Started` public-effect compare-and-swap from ADR 0033.

`NotFound` is returned only after every block in the caller-owned bounded
window is available as finalized by numeric ID and hash, the blocks form the
expected ancestry, the exact matching claim is absent from the complete scan,
and a second finalized-tip read is identical. The response echoes the exact
window. The proof is bounded, never global.

```mermaid
flowchart TB
    Actor["Role fixed reference actor"]
    Request["Exact terms, transcript, target, and bounded window"]
    Sidecar["Authenticated local LEZ v0.2 sidecar"]
    Indexer["Pinned finalized indexer API"]
    Present["PresentExact with complete claim facts"]
    Identity["Compare durable ID, exact bytes, and signature"]
    Conflict["ConflictingPresence"]
    Absent["NotFound with stable complete scan"]
    Unavailable["Unavailable"]
    Uncertain["Uncertain"]
    Journal[("Public effect journal")]

    Actor --> Request
    Request --> Sidecar
    Sidecar --> Indexer
    Indexer --> Sidecar
    Sidecar --> Present
    Sidecar --> Absent
    Sidecar --> Unavailable
    Sidecar --> Uncertain
    Present --> Identity
    Identity -->|"Match"| Journal
    Identity -->|"Conflict"| Conflict
    Conflict --> Journal
    Absent --> Journal
    Unavailable -.-> Actor
    Uncertain -.-> Actor
```

Presence maps to `PresentExact` reconciliation. Stable complete absence maps
to `Absent` reconciliation. Unavailable and uncertain states map to
`Uncertain` reconciliation and can never consume send authority. A later
poll may use a fresh request identity and a deliberately selected later
window; durable `Started` or `Unknown` state still cannot rearm.

`PresentExact` is reconciled against the complete durable public identity. An
exact match advances monotonically to accepted observation-only state. A
different transaction ID, exact byte sequence, or aggregate signature maps to
`ConflictingPresence`: `Prepared` becomes `Unknown` atomically without
returning `SubmitOnce` or calling transport. This deliberately sacrifices
liveness after contradictory positive chain evidence so a later `NotFound`
cannot turn an RPC or witness contradiction into a duplicate send.

```mermaid
sequenceDiagram
    participant Actor as Role fixed actor
    participant Journal as Public effect journal
    participant Sidecar as LEZ sidecar
    participant Indexer as Finalized indexer

    Actor->>Journal: Persist exact public bytes and expected ID
    Actor->>Sidecar: Classify exact claim in bounded window
    Sidecar->>Indexer: Read stable finalized tip and complete ID/hash ancestry
    Indexer-->>Sidecar: Blocks, transactions, and historical account facts
    Sidecar->>Indexer: Reread finalized tip
    alt Exact claim present
        Sidecar-->>Actor: PresentExact
        alt ID, exact bytes, and signature match durable effect
            Actor->>Journal: Reconcile PresentExact
        else Positive evidence conflicts with durable effect
            Actor->>Journal: Burn authority as ConflictingPresence
            Journal-->>Actor: Unknown and ObserveOnly
        end
    else Complete stable scan has no claim
        Sidecar-->>Actor: NotFound
        Actor->>Journal: CAS Prepared to Started
    else History, finality, or tip unavailable
        Sidecar-->>Actor: Unavailable
        Actor->>Journal: Observe only
    else Timeout or transport ambiguity
        Sidecar-->>Actor: Uncertain
        Actor->>Journal: Observe only
    end
```

## Trust and atomicity boundary

The scan trusts the capability-authenticated pinned sidecar and the pinned
official indexer's finalized, by-ID, by-hash, and historical-account
semantics. Logos v0.2 does not provide a cryptographic account proof or an
atomic multi-read snapshot token; stable finalized-tip bracketing is the local
PoC compensation and remains an upstream production-readiness exception.

This decision does not make the indexer read, SQLite CAS, RPC send, and later
lifecycle projection atomic. Exact public bytes become durable first. Only one
caller can consume send authority. A crash or ambiguous result after that
point leaves observation-only recovery.

An accepted submission is not lifecycle evidence. The actor remains at its
predecessor revision until a later `PresentExact` response passes the complete
finalized evidence binding and the recovery-store predecessor CAS.

## Consequences

- Protocol, client, adapter, and pinned-sidecar tests distinguish exact
  presence, definitive bounded absence, immature/partial history, moving tips,
  timeouts, and transport failures.
- The legacy affirmative finalized observer remains externally compatible; a
  definitive absence maps back to its legacy unavailable result.
- The actor now composes LEZ completion, persist-before-presence, bounded
  classification, one-attempt submission, peerless observation, and revisions
  three and four without treating upstream flakiness as absence or retry
  authority.
- Eight focused LEZ actor tests cover both owned directions, both peer roles,
  stable absence, later-window finalized presence, unavailable/uncertain
  classes, `Started`/`Unknown` restart, activation reruns, contradictory
  bytes/signatures, and out-of-window evidence. The complete actor gate is 34
  library tests plus seven CLI integration tests.
- Actual local-node actor execution and retained terminal evidence remain
  required before M3 local-PoC certification.
