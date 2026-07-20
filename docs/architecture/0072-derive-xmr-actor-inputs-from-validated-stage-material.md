# ADR 0072: Derive XMR actor inputs from validated stage material

Status: Accepted as an M4 composition rule; public session descriptors are
implemented and actual role-process execution remains pending.

Date: 2026-07-20

## Context

The two-stage XMR agreement already commits the participant identities, DLEQ
proofs, aggregate signing keys, exact future LEZ messages, windows, shared
Monero address, withheld claim partial, and activation commitment. Accepting
those values again as independent CLI strings would create a second, weaker
protocol schema and allow fields from different swaps to be cross-wired.

Role processes also need exact claim/refund adaptor contexts. Their session IDs
use a private SDK domain and the validated Stage-A commitment; actors must not
copy that cryptographic derivation.

## Decision

All XMR lifecycle actors derive protocol inputs from canonical stage material:

1. decode Stage A only with `XmrAgreementV1::from_wire`;
2. decode Stage B only with `XmrActivatedAgreementV1::from_wire` and the
   actor-owned private Monero view key;
3. derive tag-13 terms only from `lez_initialize_plan`; and
4. derive claim/refund contexts only from descriptors minted by the validated
   `XmrAgreementV1`.

Node URLs, chain/deployment identity, run/request identity, state directory,
and the role's LEZ key remain runtime configuration. Before node I/O, the actor
must compare its role and LEZ identity with the validated plan.

`XmrAdaptorSessionDescriptorV1` has private fields. Its checked `context`
method rederives the purpose-separated ID inside the SDK, rebuilds the exact
untweaked adaptor context, and rechecks the durable binding. Descriptors can be
created only from contexts retained by an already validated agreement.

This decision does not define how independent actors validate and countersign
unsigned Stage-A/Stage-B bodies. That is the next composition boundary, and it
must not give Maker the Taker claim partial before finalized tag 14.

## Component view

```mermaid
flowchart LR
    StageA["Canonical Stage A wire"] --> Agreement["Validated agreement"]
    StageB["Canonical Stage B wire"] --> Activation["Validated activation"]
    ViewKey["Owner private view key"] --> Activation
    Agreement --> Activation
    Agreement --> Claim["Checked claim descriptor"]
    Agreement --> Refund["Checked refund descriptor"]
    Claim --> ClaimActor["Role owned claim session"]
    Refund --> RefundActor["Role owned refund session"]
    Activation --> Plan["Validated LEZ initialize plan"]
    Plan --> Taker["Taker tag 13 actor"]
    Runtime["Loopback RPCs and deployment"] --> Taker
```

## Validation and execution flow

```mermaid
sequenceDiagram
    participant TakerProc as Taker actor
    participant SDK as XMR SDK
    participant Indexer as Finalized LEZ indexer
    participant Sequencer as LEZ sequencer

    TakerProc->>SDK: validate Stage A wire
    SDK-->>TakerProc: validated agreement
    TakerProc->>SDK: validate Stage B with view key
    SDK-->>TakerProc: validated activation and LEZ plan
    TakerProc->>Indexer: read stable finalized nonces
    Indexer-->>TakerProc: canonical accounts and clock
    TakerProc->>TakerProc: recompute messages and compare plan
    TakerProc->>Sequencer: submit exact Initialize once
    TakerProc->>Indexer: require finalized Initialize
    TakerProc->>Sequencer: submit exact Fund once
    TakerProc->>Indexer: require finalized Fund
```

Finality reads are observation, not submission retries. This sequence proves
ordered LEZ funding only; it is not cross-chain atomicity.

## TDD evidence

The descriptor API began with an expected compile failure for eleven missing
API items. The GREEN suite proves exact equality with retained contexts and
rejects wrong purpose, session, message, adaptor, key order, binding, and a
refund-into-claim cross-wire. All seven SDK tests, strict Clippy, warning-fatal
Rustdoc, formatting, and diff checks pass.

## Consequences and remaining work

- Actors do not define a parallel agreement format or session-domain formula.
- The private view key remains owner-only and is absent from public evidence.
- Stable finalized nonces remain live inputs and must be checked before tag 13.
- Descriptors are not signing/release authority or actual-swap evidence.

Add role-correct unsigned-body validation and countersigning packets; refactor
the tag-13 executable to accept canonical wires; then compose actual finalized
tags 13, 14, and 15, adaptor extraction, and the official-wallet Monero spend.
Recovery, chaos, and production/Stagenet hardening follow the happy PoC under
ADR 0027.
