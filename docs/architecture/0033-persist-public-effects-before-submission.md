# ADR 0033: Persist exact public effects before one-attempt submission

Status: Accepted for the M3 public-effect persistence boundary. Reference-actor
claim integration and actual-node crash evidence remain pending.

## Context

Bitcoin Core, the LEZ sequencer, and SQLite cannot participate in one atomic
commit. A process can stop after a node accepts a transaction but before the
local lifecycle advances. Retrying blindly can duplicate a LEZ admission or
reuse a signing-side effect; treating a transport timeout as rejection can
release a second send authority. Bitcoin transaction IDs also omit witness
bytes, so a txid alone is not an exact claim replay commitment.

The adaptor scalar, nonces, shares, keys, seeds, and any unrevealed Zcash
preimage are secret material. They do not belong in this plaintext journal.
The journal receives only a transaction that is already complete and public.

## Decision

`SqlitePublicEffectJournal` is an additive owner-private SQLite boundary keyed
by swap, local role, chain, operation, and aggregate predecessor. Before any
RPC submission it records:

- the nonzero countersigned-agreement commitment;
- the chain-native expected effect identifier;
- the complete exact Bitcoin or LEZ transaction bytes; and
- a SHA-256 commitment to those bytes.

The same chain-native identifier cannot be attached to another authority key.
Bitcoin bytes include the witness serialization. A caller first observes the
chain, then reconciles that result with the durable row. Only a successful
`Prepared` to `Started` compare-and-swap returns `SubmitOnce`. `Started` and
`Unknown` are never re-armed. A definitive exact-byte observation can advance
`Prepared`, `Started`, or `Unknown` to `Accepted`; an uncertain observation is
always observe-only. A fresh accepted RPC result must return the exact expected
identifier.

```mermaid
stateDiagram-v2
    [*] --> Prepared: exact public bytes durable
    Prepared --> Accepted: exact bytes already observed
    Prepared --> Started: absent and single CAS wins
    Started --> Accepted: exact accepted ID or exact observation
    Started --> Rejected: definitive pre-admission rejection
    Started --> Unknown: ambiguous RPC outcome
    Unknown --> Accepted: exact bytes later observed
    Prepared --> Prepared: uncertain or replay
    Started --> Started: restart is observe-only
    Unknown --> Unknown: restart is observe-only
    Accepted --> Accepted: exact replay
    Rejected --> Rejected: exact replay
```

The signer journal gains an existing-only opener: a miss cannot create an empty
signer database. The LEZ bridge client exposes the same pure prepared witnessed-
claim validator already used internally, so an actor can validate exact public
message bytes and their official domain-separated hash without copying a
private protocol constant.

## Atomicity and crash boundaries

```mermaid
sequenceDiagram
    participant Actor as Role fixed actor
    participant Signer as Existing signer journal
    participant Effects as Public effect journal
    participant Node as Core or LEZ RPC
    participant Lifecycle as BTC recovery store

    Actor->>Signer: Reopen exact completed session
    Actor->>Actor: Construct complete public transaction
    Actor->>Effects: Persist agreement, ID, and exact bytes
    Actor->>Node: Observe exact effect
    Node-->>Actor: Present, absent, or uncertain
    Actor->>Effects: Reconcile observation
    alt sole SubmitOnce authority
        Effects-->>Actor: Started is durable
        Actor->>Node: Submit exact bytes once
        Node-->>Actor: Accepted, rejected, or unknown
        Actor->>Effects: Persist fresh outcome
    else observation only
        Effects-->>Actor: No submission authority
    end
    Actor->>Node: Require finalized or confirmed exact evidence
    Actor->>Lifecycle: Project next predecessor CAS
```

There is deliberately no cross-system atomicity claim. The safety property is
that the exact public effect and the consumed single-attempt authority survive
every local crash boundary. After ambiguity, recovery observes; it does not
send again. Lifecycle projection occurs only after canonical chain evidence.

## Consequences

- Twelve focused tests cover immutable replay, concurrent single-winner CAS,
  rollback, exact accepted identifiers, effect-ID isolation, corruption, and
  ambiguous recovery. The complete swap-store suite is 84/84.
- Public material is restartable without persisting the adaptor scalar. Secret-
  bearing Zcash claims remain in the protected claim journal.
- “Public” means node-disclosable bytes, not a public endpoint. The component
  tests use temporary SQLite only: no RPC, Docker, faucet, peer, or network.
- Zcash is excluded by the typed chain enum, but arbitrary bytes are not secret-
  scanned; callers still must keep secret-bearing material out of this journal.
- The boundary is stricter than node-level transaction idempotence and may
  require operator investigation when a node cannot prove exact absence after
  an ambiguous call.
- M3 is not complete until the reference actor composes this boundary through
  claim revisions three and four in both actual local directions and retains
  the crash/restart evidence.
