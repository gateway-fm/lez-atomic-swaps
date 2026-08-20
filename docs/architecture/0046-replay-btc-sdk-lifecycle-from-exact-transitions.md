# ADR 0046: Replay the BTC SDK lifecycle from exact validated transitions

Status: Accepted at the deterministic public SDK component boundary. Pushed
`0c78f3d` subsequently closes the public durable codec, exact CAS store
port, typed chain ports/runtime, restart/replay coverage, and lifecycle example.
Production supplies concrete durable storage and persist-before-send journals;
actual-node actor evidence remains a separate composition boundary.

## Context

A stored revision number is not evidence that either chain effect occurred.
The former public BTC facade could resume only revision zero even though the
reference actor and lower components already supported both claims, ordered
refunds, and revisions one through four. Adding setters for a phase or revision
would let corrupted or substituted storage skip validation.

Discovery and negotiation are pre-lock capabilities. Keeping those adapters in
the active swap type would also allow an application to accidentally make
post-lock recovery depend on a peer transport.

## Decision

The pre-lock `BtcLifecycleSdk` composes application-supplied `OfferDiscovery`
and `NegotiationChannel` ports, validates the countersigned agreement, and
activates only complete agreement-bound lock, claim, and signed-refund material.
Activation returns `ActiveBtcSwap`, whose type contains neither pre-lock port.

The active state retains an ordered transition log. Revision one is the exact
Taker first-lock evidence, revision two the Maker second lock, revision three
either the revealing claim or first direction-correct refund, and revision four
either the follow-up claim or second refund. Resume requires the stored revision
to equal the transition count, reconstructs the coordinator from the agreement,
and revalidates every transition in order. Exact historical replay returns the
original revision without adding an effect.

Applying a new transition uses clone, validate, then commit. A validation or
coordinator failure therefore leaves the phase, revision, effect history, and
restart envelope unchanged. This is in-memory atomic mutation, not atomic disk
persistence or a distributed transaction with Bitcoin or LEZ.

## Components and capability boundary

```mermaid
flowchart LR
    Discovery["Offer discovery port"] --> Prelock["BTC lifecycle SDK"]
    Negotiation["Negotiation port"] --> Prelock
    Agreement["Validated agreement and prepared effects"] --> Prelock
    Prelock --> Active["Active BTC swap without peer ports"]
    Chain["Canonical chain evidence"] --> Transition["Exact lifecycle transition"]
    Transition --> Active
    Active --> Envelope["Application-owned restart envelope"]
    Envelope --> Replay["Validate ordered transition log"]
    Replay --> Active
    Store["Public process-durable store port open"] -.-> Envelope
```

## Claim and restart sequence

```mermaid
sequenceDiagram
    participant A as Application adapter
    participant S as Active BTC swap
    participant C as Coordinator clone
    participant D as Application-owned storage

    A->>S: Exact canonical first-lock evidence
    S->>C: Validate agreement, chain, bytes, and confirmations
    C-->>S: Revision 1 candidate
    S->>S: Commit clone only after complete validation
    S-->>D: Restart envelope with transition 1
    A->>S: Exact canonical second-lock evidence
    S->>C: Replay revision 1 then validate revision 2
    C-->>S: Both legs locked
    S-->>D: Restart envelope with transitions 1 and 2
    A->>S: Revealing claim then follow-up claim evidence
    S->>C: Recover material and validate exact follow-up plan
    C-->>S: Revisions 3 and 4 completed
    S-->>D: Terminal restart envelope
```

## Ordered refund sequence

```mermaid
sequenceDiagram
    participant A as Application adapter
    participant S as Active BTC swap
    participant R as Pure recovery projection
    participant C as Coordinator clone

    A->>S: Two exact canonical lock transitions
    A->>R: Fresh Bitcoin and LEZ clocks plus canonical states
    R-->>S: Direction-correct first refund observation
    S->>C: Revalidate complete recovery snapshot
    C-->>S: Revision 3 Maker leg refunded
    A->>R: Fresh snapshot with both exact refunds
    R-->>S: Direction-correct second refund observation
    S->>C: Revalidate order, identities, deadlines, and finality
    C-->>S: Revision 4 refunded
```

## Evidence and remaining boundary

External-consumer tests cover both trade directions and both roles through
revisions one to four, claim and refund terminality, resume after every
revision, exact historical replay, atomic failed-transition rollback, role,
agreement, revision, chain, finality, and byte substitution. An in-memory
discovery/negotiation test exercises the public pre-lock ports, and a
compile-fail doctest proves that `ActiveBtcSwap` cannot renegotiate.

The restart envelope in this decision was a validation boundary supplied to
application-owned storage. Pushed `0c78f3d` adds the canonical disk codec,
exact create/CAS contract, and typed runtime without embedding an endpoint or
pretending the reference in-memory store is process-durable. The existing
reference actor retains the concrete persist-before-send and actual-node
evidence; production applications provide their durable store and effect
journals.
