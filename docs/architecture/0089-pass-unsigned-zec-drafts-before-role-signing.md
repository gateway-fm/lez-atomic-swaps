# ADR 0089: Pass unsigned ZEC drafts before role signing

Status: Accepted and SDK component GREEN (2026-07-24)

## Context

The deterministic LEZ/Zebra provisioner currently constructs public chain facts,
generates both role credentials, and signs both sides in one process. That is a
valid M2 fixture but cannot prove the M5 maker-first user flow. The chain-fact
preparer must hand executable terms to the maker without receiving maker or
taker signing authority.

## Decision

Add a canonical bounded wire for `ZecAgreementDraftV1`. It contains only the
concrete schema prefix and unsigned agreement body. Decoding preflights the
application-ID allocation and total size, uses the existing bounded field
decoder, rejects trailing bytes, and runs the same complete semantic validation
required before maker signing.

The validated draft exposes only public facts needed for application
cross-binding: the exact body, maker and taker compressed public keys, canonical
commitment, and Zcash principal. Private keys, claim preimages, recovery keys,
and signatures are absent.

## Components

```mermaid
flowchart LR
    Prep[Local chain-fact preparer] -->|bounded unsigned draft| Maker[Maker application]
    Maker -->|validated maker proposal| Taker[Taker application]
    Taker -->|countersigned final wire| Maker
    Maker --> Store[(Atomic negotiation store)]
    Final[Final-wire actor config finalizer] --> Actors[Maker and taker actors]
    Maker --> Final
```

## Signing flow

```mermaid
sequenceDiagram
    participant P as Chain-fact preparer
    participant M as Maker process
    participant S as Maker SQLite
    participant T as Taker process
    participant F as Actor-config finalizer

    P->>M: Canonical unsigned draft wire
    M->>M: Bound decode and validate all executable public terms
    M->>M: Cross-bind Delivery offer, session, identities, amount, and expiry
    M->>M: Sign exact commitment with maker-owned key
    M->>S: Atomically reserve offer and stage exact proposal
    M-->>T: Durable maker proposal wire
    T->>T: Revalidate proposal and sign with taker-owned key
    T-->>M: Exact countersigned agreement wire
    M->>S: Atomically accept agreement and first-claim material
    M-->>F: Exact accepted final wire
    F->>F: Derive both role configs only from final wire and role-owned files
```

## Authority and atomicity

The unsigned draft is not an authorization and cannot reserve an offer, create
a coordinator, or submit a chain effect. The maker signs only after bounded
semantic validation and application-level offer cross-binding. The proposal is
sent only after the existing SQLite stage transaction commits; first-lock
authority appears only after the existing atomic final-acceptance transaction
commits the countersigned wire and encrypted local claim material.

This handoff therefore removes the provisioner's dual-signing authority without
inventing a distributed transaction. Cross-chain atomicity remains conditional
on the final agreement's ordered locks, role-correct disclosure, and refund
timelocks; the unsigned wire changes none of those rules.

## Consequences

The exact body can now cross a process boundary without JSON field duplication
or unsafe allocation. The full SDK suite, strict Clippy, and warning-fatal
Rustdoc are GREEN. The local preparer split, maker Chat endpoint, taker
countersign command, final-wire actor config finalizer, and actual corridor run
remain the next M5 PoC work. This component uses no chain RPC, Docker, faucet,
public endpoint, or external network.
