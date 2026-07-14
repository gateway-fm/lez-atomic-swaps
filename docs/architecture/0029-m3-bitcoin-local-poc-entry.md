# ADR 0029: M3 starts with an isolated Bitcoin actual-node PoC

Status: Proposed entry boundary; milestone not active — 2026-07-14

## Context

The live RFP and accepted proposal #112 require a complete LEZ/BTC lifecycle,
BIP-340 Schnorr adaptor signatures, a BIP-341 cooperative key-path claim, a
Bitcoin Core setup guide, both role directions, and reproducible evidence. The
current tree has generic Bitcoin state-machine vocabulary only. It has no
Bitcoin SDK, Core adapter, P2TR transaction builder, adaptor implementation,
Bitcoin actor, actual-node runner, or Bitcoin evidence packet.

The accepted proposal names DLC-specs `AdaptorSignature.md` as a conformance
source. No such file exists in the current DLC repository or its history. The
published DLC adaptor corpus is ECDSA, while M3 requires BIP-340 Schnorr. This
is a Gateway proposal/reference defect, not evidence that may be silently
substituted or marked passing.

## Decision

When the owner explicitly enters M3, the PoC target is a reproducible local
happy path through a Bitcoin Core 31.1 Regtest node built from the signed,
checksum-pinned official binary archive and bound to its exact source revision,
plus the pinned local LEZ v0.2 stack. It uses independent maker and taker
actors, separate keys and stores, actual signed transactions, and the same
public SDK boundary intended for later routes. Public Testnet4 changes node
configuration, credentials, funding, confirmation policy, and deployed LEZ
identity; it does not select a different protocol implementation.

```mermaid
flowchart LR
    Maker["Maker actor"] --> MakerSdk["Role-fixed BTC SDK"]
    Taker["Taker actor"] --> TakerSdk["Role-fixed BTC SDK"]
    MakerSdk --> MakerBtcPort["Maker Bitcoin chain port"]
    TakerSdk --> TakerBtcPort["Taker Bitcoin chain port"]
    MakerSdk --> LezPort["Role-local LEZ chain ports"]
    TakerSdk --> LezPort
    MakerBtcPort --> MakerCoreAdapter["Maker Core adapter instance"]
    TakerBtcPort --> TakerCoreAdapter["Taker Core adapter instance"]
    MakerCoreAdapter --> Core["Bitcoin Core 31.1 Regtest"]
    TakerCoreAdapter --> Core
    Provisioner["Run-owned miner and fund provisioner"] --> Core
    LezPort --> RoleBridge["Role-local LEZ bridge"]
    RoleBridge --> Sequencer["LEZ v0.2 sequencer"]
    Sequencer --> Bedrock["Bedrock settlement"]
    Sequencer --> Indexer["LEZ v0.2 indexer"]
    MakerSdk --> MakerStore["Maker store and keys"]
    TakerSdk --> TakerStore["Taker store and keys"]
    Core --> Evidence["Secret-safe evidence auditor"]
    Indexer --> Evidence
    MakerStore --> Evidence
    TakerStore --> Evidence
```

Bitcoin funding commits a two-party aggregate internal key `P` and the ADR
0009 CSV refund tapleaf. Both the Bitcoin claim authority and the LEZ
witnessed-claim authority are distinct two-party aggregate keys. Neither actor
receives a standalone claim key. Otherwise that actor could bypass the adaptor
transcript, claim one leg without revealing the agreed scalar, and destroy
atomicity.

The Bitcoin signing session is Taproot-tweak aware. It derives the even-Y x-only
internal key `P`, exact tapleaf and leaf version, Merkle root, tweak
`h = TapTweak(P || merkle_root)`, and output key `Q = P + hG`; it binds the
output-key parity carried by the control block. MuSig2/adaptor signing applies
the identical tweak and parity convention and signs the exact BIP-341 key-path
sighash under `Q`, with the annex presence/absence and sighash type fixed in
the agreement. The agreement commits `P`, its x-only/parity convention, the
tapleaf/script/version, Merkle root, `Q`, control block, input/outpoint/value,
destinations, transaction message, and sighash policy. Library verification
under the raw untweaked `P` is never spend evidence; the completed signature
must verify under `Q` and pass Bitcoin Core consensus.

No project-owned elliptic-curve arithmetic is permitted. Candidate libraries
must be exact-pinned, source-reviewed, license/advisory/source gated, and proved
interoperable with Bitcoin Core consensus before acceptance. The entry audit
selects Bitcoin Core 31.1 as the node candidate and `rust-bitcoin`, `miniscript`,
`corepc`, and `musig2` as candidates, not accepted dependencies. The final
feature graph and cryptographic evidence decide adoption.

## Candidate provenance

The 2026-07-14 entry audit selected Bitcoin Core 31.1 source commit
`9be056a8a72b624dae9623b2f7bded92c2a21c91`. The official x86_64 Linux
archive candidate has SHA-256
`b80d9c3e04da78fb6f0569685673418cf686fadba9042d926d13fb87ff503f9e`.
Bitcoin Core publishes no endorsed container image. The PoC therefore builds a
repository-owned minimal image from the official archive only after its checksum
and release signatures/attestations verify, then records and vulnerability-scans
the resulting immutable image digest. The existing unofficial Docker Hub image
is not a supply-chain authority.

This provenance is an audited candidate, not retained executable evidence. The
runner must reproduce every verification before the dependency and image become
accepted pins.

## Pre-lock signing ceremony

Both claim transactions and all recovery material are complete before the first
lock. Each chain/message uses a distinct domain-separated MuSig2 session and
fresh nonce. A crash-safe journal reserves the secret nonce before its public
commitment is exposed and forbids reuse across messages/swaps/chains. Before any
partial-signature network send, one atomic local transaction stores the exact
outbox bytes and marks that nonce consumed. The secret nonce is then zeroized;
delivery may only send or retransmit the already-persisted bytes. Only the
verified aggregate adaptor pre-signatures, public transcripts, exact messages,
refund material, and recovery state remain after the ceremony.

```mermaid
sequenceDiagram
    participant T as Taker SDK and store
    participant M as Maker SDK and store
    T->>M: Dual-signed terms and exact BTC/LEZ claim messages
    M->>T: Confirm tweaked Q, LEZ authority, adaptor point and refunds
    T->>T: Reserve fresh BTC and LEZ secret nonces durably
    M->>M: Reserve fresh BTC and LEZ secret nonces durably
    T->>M: Nonce commitments then public nonces
    M->>T: Nonce commitments then public nonces
    T->>T: Atomically persist exact partial outbox and consume nonces
    M->>M: Atomically persist exact partial outbox and consume nonces
    T->>T: Zeroize secret nonces
    M->>M: Zeroize secret nonces
    T->>M: Send persisted message-bound partial/adaptor signatures
    M->>T: Send persisted message-bound partial/adaptor signatures
    T->>T: Verify both aggregate pre-signatures and persist recovery
    M->>M: Verify both aggregate pre-signatures and persist recovery
    Note over T,M: No counterparty signing interaction is needed after first lock
```

Funding is forbidden unless both roles independently verify and durably retain
the exact presignatures they need for either direction. Discovery/Chat may vanish
after the taker submits the first lock without affecting claim or recovery.

## Actor flows

In `TakerSellsForeign`, the taker locks BTC first. The maker locks LEZ only
after the negotiated Bitcoin confirmation depth. The taker adapts and submits
the aggregate LEZ witnessed claim, revealing the agreed scalar in its finalized
signature. The maker extracts that scalar from those exact finalized LEZ bytes
and adapts the Bitcoin key-path claim.

```mermaid
sequenceDiagram
    participant T as Taker sells BTC
    participant B as Bitcoin Core
    participant M as Maker
    participant L as LEZ v0.2
    T->>B: Fund exact P2TR output
    B-->>M: Canonical confirmed outpoint
    M->>L: Fund witnessed BTC escrow
    L-->>T: Canonical finalized escrow
    T->>L: Adapt and submit aggregate claim
    L-->>M: Exact finalized revealing signature
    M->>B: Extract scalar and key-path claim
    B-->>T: Canonical confirmed spend
```

In `TakerSellsLez`, the taker locks LEZ first. The maker locks BTC only after
LEZ finality. The taker adapts the Bitcoin key-path claim; the maker extracts
the scalar from the exact Bitcoin witness and adapts the LEZ witnessed claim.

```mermaid
flowchart TD
    Agreement["Dual-signed immutable agreement"] --> First{"Direction-derived taker lock"}
    First --> TakerLez["Taker locks LEZ"]
    First --> TakerBtc["Taker locks BTC"]
    TakerLez --> MakerBtc["Maker locks BTC after LEZ finality"]
    TakerBtc --> MakerLez["Maker locks LEZ after BTC depth"]
    MakerBtc --> RevealBtc["Taker key-path claim reveals scalar"]
    MakerLez --> RevealLez["Taker witnessed LEZ claim reveals scalar"]
    RevealBtc --> FinishLez["Maker adapts LEZ claim"]
    RevealLez --> FinishBtc["Maker adapts BTC claim"]
    FinishLez --> Complete["Both actors Completed"]
    FinishBtc --> Complete
    MakerBtc -. both locked then abandoned .-> MakerRefund["Maker recovers shorter maker-funded leg"]
    MakerLez -. both locked then abandoned .-> MakerRefund
    MakerRefund --> SafetyMargin["Wait typed cross-chain safety margin"]
    SafetyMargin --> TakerRefund["Taker recovers longer taker-funded leg"]
    TakerRefund --> Recovered["Both actors Recovered"]
```

If the maker never submits the second lock, the taker eventually recovers the
first leg directly. If both legs are locked and claims do not complete, the
maker-funded shorter recovery happens first and the taker-funded longer recovery
happens only after the conservative cross-chain margin. For
`TakerSellsForeign` this maps earlier recovery to maker-funded LEZ and later
recovery to taker-funded BTC; for `TakerSellsLez` it maps earlier recovery to
maker-funded BTC and later recovery to taker-funded LEZ. Typed chain clocks are
not compared as raw numbers.

## Reproducibility and isolation

- Every run owns a unique `RUN_ID`, Compose project, network, volumes, temporary
  root, cookie file, wallets, actors, and evidence output.
- Host listeners bind allocated loopback ports; no conventional host port is
  assumed. The provisioner alone holds the full node cookie and mining/wallet
  methods. Maker and taker adapters are separate instances with distinct
  `rpcauth` credentials and method allowlists for required read/broadcast calls;
  keys remain client-side and neither actor can access the other's wallet or
  provisioner methods.
- Cleanup removes only resources carrying the exact run label. Global Docker
  prune, shared volume removal, broad process kills, and foreign-container
  ownership are prohibited.
- Regtest funds come from a run-local deterministic mining/provisioning actor.
  Reproducibility binds the fixed descriptor/key derivation, Regtest genesis,
  block-time policy, maturity, transaction policy, exact values, and observed
  outpoints/confirmations. It is a semantic contract; independently named runs
  need not produce byte-identical block hashes or transaction IDs. No public
  RPC, faucet, public funds, or public chain is used by the PoC.
- Cold setup still depends on verified Bitcoin Core release assets and Rust
  registries; their checksums, availability, cache policy, and scan results are
  evidence, not hidden assumptions.

## PoC gate

The PoC is not complete until both directions run through actual local Core and
LEZ nodes, with taker-first confirmation ordering, distinct actors and stores,
the complete pre-lock signing ceremony durably proven, no off-chain dependency
after the first lock, tweak-aware aggregate-signature/adaptor verification, a
one-item BIP-341 key-path witness under `Q`, correct recipients, the BTC
outpoint spent once, zero terminal LEZ custody, both actors `Completed`, and a
secret-safe immutable evidence packet. The agreement must also commit the exact
refund tapleaf/control block and direction-correct two-stage recovery schedule,
even though before/at/after CSV execution belongs to the owner-selected QA
phase.

Literal DLC Schnorr-vector conformance remains unclaimable because the named
file does not exist. The honest proposed replacement gate is official BIP-340
and BIP-327 vectors, project-owned adaptor positive/negative fixtures, an
independent implementation cross-check, and completed-signature verification
through the Bitcoin library plus Bitcoin Core consensus. Gateway erratum
[GW-M3-001](../proposal-acceptance-errata.md) records the mismatch; an accepted
clarification has not yet been posted. It does not permit ECDSA evidence to be
mislabeled as Schnorr evidence.

This ADR records a proposed entry boundary only. It does not activate M3, accept
the candidate dependency graph, claim either direction works, or authorize an
`m3-complete` tag.
