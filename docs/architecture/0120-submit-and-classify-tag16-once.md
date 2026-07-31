# ADR 0120: Submit and classify tag 16 once

- Status: Accepted for the M5 component checkpoint
- Date: 2026-07-30
- Milestone: M5 progressive local-functional PoC

## Context

ADR 0119 made the exact generated native-XMR tag 16 message durably
preparable and completable under authenticated Taker authority, but deliberately
left submission and finalized observation closed. The next safe boundary must
admit only those already completed bytes, preserve the existing one-attempt
submission policy across restart, and let each role classify the same finalized
refund without turning observation into authority.

This checkpoint is component-tested with authenticated sidecars, a controlled
sequencer, and finalized-indexer fixtures. It is not an actual local-devnet
refund run. Maker ingestion of the finalized aggregate signature, extraction of
the Taker adaptor scalar, reconstruction of the Stage-A Monero spend key, a real
Monero sweep to Maker, cross-chain evidence binding, tag 17, and milestone
certification remain open.

## Decision

1. The authenticated Taker sidecar is the only tag 16 submission owner.
2. Generic submission admits tag 16 only when the exact transaction matches
   both create-new durable preparation and completion state. Runtime, run,
   role, terms, signature, canonical bytes, and transaction identity are
   revalidated before node access.
3. The canonical submission request ID is derived from the completed
   transaction ID. A fresh arbitrary request ID fails closed.
4. Submission performs an exact lookup before at most one sequencer send.
   Accepted replay performs no node I/O. An ambiguous one-attempt result stays
   unknown across restart and is never automatically resent.
5. The Taker may classify its exact owned tag 16 transaction. The Maker may
   independently discover tag 16 by the complete v3 terms. Other role and
   effect combinations fail closed.
6. A `Found` result binds the exact public transaction bytes, refund message
   hash, ordered metadata, custody, depositor, and refund-authority accounts,
   sole aggregate authority signer, aggregate BIP340 signature, `Refunded`
   metadata state, zero custody balance, containing block, and stable finalized
   scan coverage.
7. The containing block timestamp must be in the half-open interval
   `[refund_at, punish_at)`. Both endpoints are checked explicitly.
8. Classification is read-only and never submits. Tag 17 remains unavailable.

## Components and trust boundaries

```mermaid
flowchart LR
    Taker["Authenticated Taker actor"]
    TakerSidecar["Taker sidecar"]
    Planner["Tag 16 planner"]
    Durable["Private prepare and completion state"]
    Journal["Durable one-attempt submission journal"]
    Sequencer["LEZ sequencer RPC"]
    Indexer["LEZ finalized indexer RPC"]
    TakerView["Taker exact classifier"]
    MakerSidecar["Maker sidecar"]
    MakerView["Maker discovery classifier"]
    Facts["Finalized refund facts"]

    Taker -->|Completed exact tag 16| TakerSidecar
    TakerSidecar --> Planner
    Durable --> Planner
    Planner --> Journal
    Journal -->|Exact lookup then at most one send| Sequencer
    TakerSidecar --> TakerView
    MakerSidecar --> MakerView
    TakerView --> Indexer
    MakerView --> Indexer
    Indexer --> Facts
```

The sidecar capability and durable planner state remain role-private. The
sequencer is an effect boundary; the indexer is an observation boundary. Maker
discovery receives no Taker submission capability and does not consume private
Taker state.

## One-attempt submission and finalized observation

```mermaid
sequenceDiagram
    participant T as Taker actor
    participant S as Taker sidecar
    participant D as Durable state
    participant Q as LEZ sequencer
    participant I as Finalized indexer
    participant M as Maker sidecar

    T->>S: Submit completed tag 16
    S->>S: Authenticate Taker and canonical request ID
    S->>D: Reload and revalidate preparation and completion
    S->>D: Reserve one-attempt submission
    S->>Q: Exact transaction lookup
    alt Exact transaction is absent
        S->>Q: Send exact transaction once
    else Exact transaction is already known
        S->>S: Do not send
    end
    S->>D: Persist accepted or unknown outcome
    S-->>T: Stable submission result
    T->>S: Classify exact refund
    M->>I: Discover refund by exact terms
    S->>I: Scan bounded finalized window
    I-->>S: Stable finalized transaction and state
    I-->>M: Stable finalized transaction and state
    Note over S,M: Both views validate the same hash, signature, accounts, state, and refund window
```

An accepted replay is a journal read, not a second submission. If the first
send cannot be classified safely, restart preserves `Unknown` and requires
later reconciliation rather than another send.

## Conditional atomicity boundary

```mermaid
flowchart TD
    Terms["Stage A and B commit tag 16 hash and refund session"]
    Completed["Taker completes exact tag 16 with aggregate signature"]
    Submitted["Taker submits exact transaction at most once"]
    Finalized["Tag 16 is found in the finalized refund window"]
    Revealed["Finalized signature can reveal the Taker adaptor scalar"]
    Extracted["Maker ingests and extracts from its refund presignature"]
    Reconstructed["Maker reconstructs the exact Stage-A Monero spend key"]
    Swept["Neutral shared wallet sweeps Monero to Maker"]
    Bound["LEZ and Monero evidence are cross-bound"]

    Terms --> Completed --> Submitted --> Finalized --> Revealed
    Revealed -.->|Open actual runner work| Extracted
    Extracted -.-> Reconstructed
    Reconstructed -.-> Swept
    Swept -.-> Bound
```

The implemented solid path proves that one exact timeout-refund signature can
be submitted once and recognized only as the finalized tag 16 effect committed
by the agreement. The aggregate signature is the public cryptographic input
needed by Maker, but this checkpoint does not yet ingest or extract it.
Therefore it proves neither the Monero recovery effect nor conditional
cross-chain refund atomicity.

## Consequences

- Tag 16 no longer aliases legacy generic refunds or tag 15 ownership.
- Accepted and ambiguous outcomes retain the same at-most-once behavior across
  restart.
- Exact-owner and counterparty-discovery views agree on the finalized effect,
  while their authorities remain separate.
- The next PoC slice must run the role-correct local-devnet refund tail:
  finalized Maker ingestion, adaptor extraction from the precommitted refund
  presignature, symmetric spend-key reconstruction, neutral shared-wallet
  sweep to Maker, Taker-mined Monero confirmations, and cross-chain binding.
- No M5 tag or milestone certification is justified by this component
  checkpoint.
