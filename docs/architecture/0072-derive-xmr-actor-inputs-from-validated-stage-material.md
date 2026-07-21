# ADR 0072: Derive XMR actor inputs from validated stage material

Status: Accepted as an M4 composition rule; SDK boundaries, the role-fixed
tag-13 actor, independent material provisioning, the completed Taker-journal
handoff, private Stage-A signing/assembly/session roots, one-journal-per-role
adaptor rounds, canonical countersigned Stage B, and actual-local tag-13
Initialize/Fund execution are implemented. Tag 14 onward remains pending.

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

The SDK now validates each unsigned Stage-A/Stage-B body before either role
signs and attaches only correctly indexed Maker/Taker signatures. Independent
role provisioning now binds each private bundle to its public packet. The
authenticated bridge handoff opens the existing Taker claim journal, requires
its completed signing phase, rebinds its exact transcript and withheld partial
to Stage B, and creates no plaintext side store. The composed processes must
still preserve the rule that Maker never receives the Taker claim partial
before finalized tag 14.

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
    ClaimActor --> TakerJournal["Completed Taker claim journal"]
    TakerJournal --> Handoff["Authenticated Stage B handoff"]
    Activation --> Handoff
    Handoff --> Tag14["Exact tag 14 preparation"]
    Activation --> Plan["Validated LEZ initialize plan"]
    Plan --> Taker["Role fixed Taker tag 13 actor"]
    Signer["Owner private Taker LEZ key"] --> Taker
    Indexer["Official finalized indexer RPC"] --> Taker
    Taker --> Sequencer["Official sequencer RPC"]
    Deployment["Checked M4 ProgramID and local chain identity"] --> Taker
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
    TakerProc->>TakerProc: require finalized clock at or before signed funding cutoff
    TakerProc->>Sequencer: submit exact Initialize once
    TakerProc->>Indexer: require finalized Initialize
    TakerProc->>TakerProc: recheck Initialize clock at or before cutoff
    TakerProc->>Sequencer: submit exact Fund once
    TakerProc->>Indexer: require finalized Fund at or before cutoff
```

Finality reads are observation, not submission retries. This sequence proves
ordered LEZ funding only; it is not cross-chain atomicity.

The claim-publication handoff is independently constrained as follows:

```mermaid
sequenceDiagram
    participant TakerJournal as Taker claim journal
    participant Adapter as Typed bridge adapter
    participant StageB as Validated Stage B
    participant Sidecar as Authenticated sidecar

    Adapter->>TakerJournal: open existing exact session
    TakerJournal-->>Adapter: completed transcript and withheld partial
    Adapter->>StageB: rebind identity, transcript, Maker partial, and commitment
    StageB-->>Adapter: exact published-partial validation
    Adapter->>Sidecar: prepare exact tag 14 once
```

This flow does not reveal the partial to the Maker or submit it by itself; it
only removes the unsafe manual/plaintext handoff from the later actor route.

## TDD evidence

The descriptor API began with an expected compile failure for eleven missing
API items. The GREEN suite proves exact equality with retained contexts and
rejects wrong purpose, session, message, adaptor, key order, binding, and a
refund-into-claim cross-wire. The unsigned-body API separately began with eight
expected missing-type errors and proves semantic rejection before signature
attachment, wrong view-key/cross-agreement rejection, role-indexed signatures,
and byte-identical canonical wires. All 16 SDK tests, strict Clippy,
warning-fatal Rustdoc, formatting, and diff checks pass.

The tag-13 actor retains 12 GREEN unit tests for the checked deployment identity,
role-restricted finalized nonce source, owner-only inputs/evidence, no-clobber
before node access, strict CLI schema, stable-anchor policy, and effect-specific
classification requests. It also rejects a stale finalized preflight before any
submission, rechecks the signed Maker funding cutoff after finalized Initialize
before Fund, and refuses success evidence when Fund finalizes after the cutoff.
Five additional focused reusable-finality tests cover stable advancement, pinned-view mutation, finalized-height regression, wrong genesis, and ProgramID mutation. Only finalized LEZ consensus timestamps carry cutoff authority. Locked/offline actor tests and strict Clippy pass with
the pinned local rapidsnark libraries. This is component evidence only; no node
effect was executed by that test gate.

## Consequences and remaining work

- Actors do not define a parallel agreement format or session-domain formula.
- The private view key remains owner-only and is absent from public evidence.
- Stable finalized nonces remain live inputs and must be checked before tag 13.
- Fund cannot be submitted until exact Initialize bytes are classified `Found`
  in stable finalized history; polling never grants another submission attempt.
- The current PoC actor is deliberately one-shot after submission: crash restart
  and ambiguous-outcome reconciliation remain post-PoC hardening. An operator
  must never delete its empty evidence reservation without reconciling chain
  state first.
- The PoC must use dedicated per-swap owner accounts with no unrelated nonce
  consumers; durable nonce leasing/exclusivity remains production hardening.
- Descriptors are not signing/release authority or actual-swap evidence.

Feed the GREEN canonical Stage A/B and completed Taker journal into the
canonical-wire tag-13 actor, then compose finalized tags 14 and 15,
adaptor extraction, and the official-wallet Monero spend.
Recovery, chaos, and production/Stagenet hardening follow the happy PoC under
ADR 0027.
