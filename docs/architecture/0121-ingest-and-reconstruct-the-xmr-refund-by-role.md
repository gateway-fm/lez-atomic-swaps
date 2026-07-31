# ADR 0121: Ingest and reconstruct the XMR refund by role

- Status: Accepted for the M5 component checkpoint
- Date: 2026-07-30
- Milestone: M5 progressive local-functional PoC

## Context

ADR 0120 closes one-attempt tag-16 submission and finalized classification, but
stops before the public refund signature becomes Maker-owned recovery
authority. The application needs one role-fixed continuation from the Taker
driver through Maker ingestion and adaptor extraction to a Monero sweep. The
existing claim sweep already contains the correct reconstruction and official
wallet RPC boundary, so the refund path must reuse that engine without
preserving claim-only economic roles.

This decision is component-tested. It does not claim an actual LEZ/Monero
refund replay or a cross-chain binding.

## Decision

1. A real Taker process validates Stage A, Stage B, the refund session, and the
   aggregate final signature before it asks the authenticated Taker sidecar to
   prepare, complete, and submit tag 16.
2. Prepare and complete request IDs must be distinct. Submission uses only the
   transaction-derived canonical request ID and inherits the sidecar's
   one-attempt/no-automatic-retry journal.
3. Only a Maker process may ingest the finalized `DiscoverByTerms` tag-16
   result. It re-derives the exact refund session and writes the aggregate
   signature through the existing role-local, journal-linked packet writer.
4. Maker extraction uses its precommitted refund presignature to recover the
   Taker adaptor scalar.
5. One sweep engine serves both reveal branches. Claim reconstructs from the
   Taker share plus Maker scalar and sweeps to Taker; refund reconstructs from
   the Maker share plus Taker scalar and sweeps to Maker.
6. The shared-wallet RPC remains a neutral effect boundary. The opposite role's
   wallet mines confirmations, so the destination wallet does not also provide
   confirmation authority.
7. Legacy claim CLI and evidence remain byte-shape compatible. Refund requires
   explicit `--journey refund`, mutually exclusive refund key inputs, and an
   honest v3 evidence schema.

## Components and RPC boundaries

```mermaid
flowchart LR
    Taker["Taker tag-16 process"]
    TakerSidecar["Authenticated Taker sidecar"]
    Sequencer["LEZ sequencer RPC"]
    Indexer["LEZ finalized indexer RPC"]
    MakerSidecar["Authenticated Maker sidecar"]
    MakerActor["Maker reference actor"]
    MakerJournal["Maker refund session journal"]
    Extractor["Adaptor role runner"]
    Sweep["Role-neutral sweep engine"]
    SharedWallet["Neutral shared-wallet RPC"]
    MakerWallet["Maker destination wallet RPC"]
    TakerWallet["Taker confirmation wallet RPC"]
    Monerod["Monero Regtest daemon RPC"]

    Taker --> TakerSidecar
    TakerSidecar --> Sequencer
    Indexer --> MakerSidecar
    MakerSidecar --> MakerActor
    MakerActor --> MakerJournal
    MakerJournal --> Extractor
    Extractor --> Sweep
    Sweep --> SharedWallet
    Sweep --> MakerWallet
    Sweep --> TakerWallet
    SharedWallet --> Monerod
    MakerWallet --> Monerod
    TakerWallet --> Monerod
```

The sequencer is the LEZ effect boundary and the indexer is the finalized LEZ
observation boundary. The neutral shared wallet owns only the reconstructed
Monero spend effect. Maker receives the refund; Taker supplies confirmation
mining. No component receives both role-private shares.

## Refund continuation

```mermaid
sequenceDiagram
    participant T as Taker process
    participant TS as Taker sidecar
    participant L as LEZ sequencer and indexer
    participant MS as Maker sidecar
    participant M as Maker actor
    participant J as Maker journal
    participant A as Adaptor runner
    participant W as Neutral shared wallet
    participant MW as Maker wallet
    participant TW as Taker wallet

    T->>T: Verify Stage A, Stage B, refund session, and final signature
    T->>TS: Prepare and complete tag 16
    T->>TS: Submit with transaction-derived request ID
    TS->>L: Exact lookup then at most one send
    MS->>L: Discover exact finalized tag 16 by terms
    MS-->>M: Canonical Found result with aggregate signature
    M->>J: Re-derive Maker refund session and persist observed signature
    J-->>A: Maker presignature plus finalized signature
    A-->>M: Extracted Taker adaptor scalar
    M->>W: Reconstruct from Maker share and Taker scalar, then sweep
    W->>MW: Pay the exact Maker destination
    TW->>L: No LEZ authority
    TW->>W: Mine Monero confirmations through its own wallet RPC
    MW-->>M: Independently verified receipt
```

The sequence is the intended composed runner flow. The component boundary
currently ends at validated role selection and serialized evidence; the actual
devnet and cross-chain-binding gate remains open.

## Conditional atomicity argument

```mermaid
flowchart TD
    A["Stage A binds both DLEQ shares and refund session"]
    B["Stage B commits both refund presignatures"]
    R["Finalized tag 16 reveals the Taker adaptor scalar"]
    K["Maker alone reconstructs the shared Monero spend key"]
    S["Neutral wallet sweeps only to the Maker destination"]
    C["Canonical LEZ refund and Monero receipt are cross-bound"]
    P["Punishment branch remains available after punish_at"]

    A --> B --> R --> K --> S
    S -.->|"Open actual replay and binder"| C
    B --> P
```

Before tag 16 finalizes, Maker lacks the Taker scalar and cannot reconstruct the
shared spend key from its retained share. Once the exact signature finalizes in
the signed refund window, the precommitted adaptor relation gives Maker the
missing scalar, while the sweep engine fixes the destination to Maker. This is
conditional atomicity, not a distributed transaction: chain reorganization,
RPC honesty, timelock margins, and the later punishment path remain explicit
assumptions. Until a fresh actual local-devnet run reaches `C`, M5 refund
atomicity is not certified.

## Consequences

- The refund continuation now has real role-fixed process surfaces rather than
  a plan-only handoff.
- Claim compatibility is preserved while refund roles are explicit and tested
  before file or RPC access.
- No public RPC, faucet, peer, or funds participate in this component proof.
- The next slice must wire the runner, add the refund binder, execute the exact
  isolated LEZ/Monero journey, retain cleanup evidence, and then assess tag 17
  and the remaining literal M5 outputs.
