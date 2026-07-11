# System architecture and actor flows

Status: Living target architecture — 2026-07-11

This is the canonical whole-system view. ADRs record why individual choices
were made; this document shows how the choices compose into the product that
operators and takers actually run. Dashed components are required final
deliverables that are not implemented yet. A test is called end to end only
when it crosses the same process, RPC, persistence, role, and chain boundaries
shown here.

## Actors, runtime components, and trust boundaries

```mermaid
flowchart TB
    subgraph People["Independent actors"]
        MO["Maker operator"]
        T["Taker"]
    end

    subgraph MakerHost["Maker-controlled host"]
        MC["Maker CLI"]
        MM["Maker mini-app"]
        LC["Logos Core lifecycle adapter"]
        MD["Maker daemon"]
        CO["Durable swap coordinator"]
        DB[("Encrypted SQLite state + outbox")]
        PS["BTC / XMR / ZEC pair SDKs"]
        ZTX["ZEC BIP-199 V5 transaction SDK"]
        CA["Validated chain adapters"]
    end

    subgraph TakerDevice["Taker-controlled device"]
        TC["Taker CLI"]
        TM["Taker mini-app"]
        TS["Taker pair SDK + durable recovery state"]
    end

    subgraph OffChain["Untrusted, removable after lock"]
        DC["Delivery / Chat"]
    end

    subgraph Nodes["Actor-selected node boundary"]
        LEZ["LEZ sequencer<br/>v0.1.2 guest deployment proven / v0.2 testnet target"]
        BTC["Bitcoin Core"]
        XMR["monerod + wallet RPC"]
        ZEC["Minimal Zebra 5.2.0 + local Zcash construction"]
    end

    subgraph Networks["Consensus networks"]
        LN["LEZ"]
        BN["Bitcoin"]
        XN["Monero"]
        ZN["Zcash transparent pool"]
    end

    MO --> MC
    MO --> MM
    MO --> LC
    MC -->|"authenticated local RPC"| MD
    MM -.->|"M6 authenticated local RPC"| MD
    LC -.->|"start / stop / health"| MD
    MD --> CO
    CO --> DB
    CO --> PS
    PS --> ZTX
    PS --> CA
    ZTX --> CA

    T --> TC
    T --> TM
    TC --> TS
    TM -.-> TS

    MD <-->|"discovery + negotiation only"| DC
    TS <-->|"discovery + negotiation only"| DC
    CA --> LEZ
    CA --> BTC
    CA --> XMR
    CA --> ZEC
    TS --> LEZ
    TS --> BTC
    TS --> XMR
    TS --> ZEC
    LEZ --> LN
    BTC --> BN
    XMR --> XN
    ZEC --> ZN

    classDef planned stroke-dasharray: 5 5,fill:#fff7e6,stroke:#9a6700;
    class MM,LC,PS,CA,TC,TM,TS,DB planned;
```

The maker operator owns maker policy, keys, node selection, and the daemon
lifecycle. The taker owns a separate client, keys, node selection, and recovery
state. Logos Core is an optional lifecycle surface, never a protocol authority.
Delivery / Chat is not trusted with secrets or chain truth and may disappear
after the first lock. Chain adapters accept consensus evidence from the selected
LEZ sequencer, Bitcoin Core, `monerod`, or Zebra; peer messages never advance an
on-chain state by themselves.

The dashed state reflects delivery honestly. The deterministic core, SQLite
repository, maker daemon, authenticated maker CLI flow, LEZ semantic
verification, pinned SPEL/LEZ generated-IDL/client fixture, and ZEC exact-script
plus signed V5 spend foundation
exist. Deterministic actor-owned funding/change and pinned, vulnerability-clean
Zebra 5.2.0 Regtest acceptance/rejection/confirmation, concurrent swaps,
confirmation regression, exact rebroadcast, block reconsideration, and a
two-node conflicting four-over-three-block fork replacement now exist as
consensus-node proof. Source-correct authenticated-transfer/ATA custody and
generated owner-role clients now exist as locally composed upstream-program
evidence. The checked Risc0 guest now builds reproducibly, deploys through
public RPC, and executes the complete native initialize/fund/claim/refund
lifecycle with real funded actor keys in an isolated v0.1.2 standalone
sequencer. Wrong-preimage, wrong-role, and early-refund transactions are
excluded from canonical blocks without mutating nonce or custody. Native
recursive cost evidence is machine-checked from production state transitions
without Clock noise. The official ATA lifecycle also passes for two independent
definitions with real owner roles and permissionless fixed refunds. Their
escrow/ATA/Token recursion is also included in the machine-checked cost record.
Public-testnet evidence, composed both-direction maker/taker processes, typed
adapter-grade ZEC observations, cross-chain deadline composition, encrypted
state/outbox, and mini-apps remain milestone work and cannot yet be represented
as production E2E.

## LEZ escrow custody components and actor flows

```mermaid
flowchart LR
    Depositor["Depositor owner account<br/>real signer on funding"]
    Claimant["Claimant owner account<br/>real signer on claim"]
    Relayer["Any relayer<br/>refund / ATA creation"]

    subgraph EscrowProgram["SPEL ZEC escrow"]
        Metadata["Metadata PDA<br/>terms, actors, digest, deadline, status"]
        NativeCustody["Native custody PDA"]
        TokenCustody["ATA(metadata, token definition)"]
    end

    Auth["Canonical authenticated-transfer program"]
    ATA["Official associated-token-account program"]
    Token["Official token program"]
    DepositorATA["ATA(depositor, definition)"]
    ClaimantATA["ATA(claimant, definition)"]

    Depositor -->|"signed native funding"| Auth
    Auth --> NativeCustody
    Metadata -->|"escrow PDA delegation on claim/refund"| Auth
    NativeCustody --> Auth

    Relayer -->|"permissionless create"| ATA
    Metadata --> ATA
    ATA -->|"nested initialize"| Token
    Token --> TokenCustody
    Depositor -->|"signed owner"| ATA
    DepositorATA --> ATA
    ATA -->|"nested token transfer"| TokenCustody
    Metadata -->|"PDA delegation on claim/refund"| ATA
    TokenCustody --> ATA
    ATA -->|"fixed destination"| DepositorATA
    ATA -->|"fixed destination"| ClaimantATA
```

```mermaid
sequenceDiagram
    actor Depositor
    actor Claimant
    actor Relayer
    participant Escrow as SPEL escrow
    participant ATA as ATA program
    participant Token as Token program

    Depositor->>Escrow: initialize_token(signed owner, claimant, definition, terms)
    Escrow-->>Escrow: bind exact actor ATAs and ATA(metadata, definition)
    Relayer->>Escrow: create_token_custody()
    Escrow->>ATA: Create(metadata, definition, custody ATA)
    ATA->>Token: InitializeAccount(custody ATA) with ATA PDA seed
    Depositor->>Escrow: fund_token(signed owner, depositor ATA, custody ATA)
    Escrow->>ATA: Transfer(owner, depositor ATA, custody ATA)
    ATA->>Token: Transfer with depositor-ATA PDA seed
    alt claim before deadline
        Claimant->>Escrow: claim_token(signed owner, preimage)
        Escrow->>ATA: Transfer(metadata PDA, custody ATA, claimant ATA)
        ATA->>Token: Transfer with custody-ATA PDA seed
    else refund at or after deadline
        Relayer->>Escrow: refund_token(custody ATA, fixed depositor ATA)
        Escrow->>ATA: Transfer(metadata PDA, custody ATA, depositor ATA)
        ATA->>Token: Transfer with custody-ATA PDA seed
    end
```

## Deployable guest and standalone block flow

```mermaid
flowchart LR
    Source["SPEL escrow source"] --> Guest["Risc0 3.0.5 guest wrapper"]
    Builder["Pinned guest-builder image digest"] --> ELF["Checked ELF<br/>SHA-256 + ImageID"]
    Guest --> Builder
    Manifest["Tracked artifact manifest"] --> ELF
    R0VM["Exact r0vm 3.0.5"] --> Clock["Mandatory clock execution"]
    Clock --> Ready["Persisted readiness block"]
    ELF --> RPC["sendTransaction ProgramDeployment"]
    RPC --> Mempool["Standalone mempool"]
    Ready --> Mempool
    Mempool --> Validate["Block-time deployment validation"]
    R0VM --> Validate
    Validate --> Block["Canonical persisted block"]
    Block --> Query["getTransaction + getLastBlockId"]
    Actors["Funded depositor + claimant keys"] --> NativeLifecycle["Signed native initialise / fund / claim"]
    Relayer["Permissionless refund relayer"] --> NativeLifecycle
    NativeLifecycle --> RPC
    Block --> NativeState["Metadata status + exact custody/actor balances"]
    NativeState --> CostReplay["Deterministic production-state replay<br/>Clock excluded"]
    CostReplay --> CostEvidence["12 attributed Risc0 sessions<br/>cycle invariants + budgets + JSON"]
    TokenActors["Two definitions + owner keys + actor ATAs"] --> TokenLifecycle["Initialize / custody / fund / claim / refund"]
    TokenLifecycle --> RPC
    Block --> TokenState["Definition-bound holdings + exact supply conservation"]
    TokenState --> TokenCostReplay["Deterministic token replay<br/>Clock/setup excluded"]
    TokenCostReplay --> TokenCostEvidence["Escrow + ATA + Token sessions<br/>invariants + budgets + JSON"]
    Testnet["Rebuilt v0.2 guest + public testnet"] -.-> RPC

    classDef planned stroke-dasharray: 5 5,fill:#fff7e6,stroke:#9a6700;
    class Testnet planned;
```

The deployment proof uses port `0`, a temporary sequencer home, deterministic
genesis inputs, an exact `r0vm` path, and no shared Docker project or chain
state. Deployment admission alone is deliberately insufficient: the readiness
block proves the mandatory clock/executor/store loop first, and transaction
lookup proves the deployment reached the block store rather than only the
mempool. The solid native lifecycle uses the actual funded genesis roles,
validates signer-bound claim and permissionless-refund boundaries against
canonical block time, and asserts exact balances. The solid token lifecycle
uses two definitions, owner-signed ATA funding/claim, permissionless custody and
refund, and cross-definition substitution negatives. Token replay attributes
the escrow/ATA/nested-Token recursion while excluding setup and Clock noise. The
dashed v0.2 testnet edge remains M2 exit work. Cost replay executes the same
guest instructions through LEZ production state transitions, counts the escrow
root and authenticated-transfer child, and compares generated JSON with the
checked evidence artifact.

## Zcash competing-fork consensus flow

```mermaid
sequenceDiagram
    actor Claimant
    actor Funder
    participant Primary as Primary Zebra 5.2.0
    participant Fork as Disconnected fork Zebra 5.2.0

    Primary->>Fork: Relay identical canonical prefix (getblock + submitblock)
    Claimant->>Primary: Signed BIP-199 claim
    Funder->>Primary: Independent signed BIP-199 refund
    Primary-->>Primary: Mine claim/refund branch to depth 3
    Funder->>Fork: Conflicting signed refund of claimant output
    Funder->>Fork: Same independent signed refund
    Fork-->>Fork: Mine conflicting branch to depth 4
    Fork->>Primary: Relay four raw consensus blocks via submitblock
    Primary-->>Primary: Accept higher-work branch; detach three blocks
    Primary-->>Claimant: Old claim no longer canonical
    Primary-->>Funder: Conflicting refund active with four confirmations
```

Both nodes use separate ephemeral loopback RPC ports, immutable tmpfs state,
non-root read-only images, and one uniquely named Compose project. This flow
proves Zebra consensus validation and best-chain replacement with actual actor
keys. It deliberately does not claim cross-chain refund-margin proof: that
requires typed ZEC observation commitments, checked LEZ millisecond/core-second
conversion, a named profile, and a composed standalone-LEZ plus Zebra run.

## Happy-path user flow

```mermaid
sequenceDiagram
    actor Maker as Maker operator
    participant Daemon as Maker daemon
    participant Store as Durable store
    participant Chat as Delivery / Chat
    actor Taker
    participant TakerLeg as Taker-funded chain
    participant MakerLeg as Maker-funded chain
    actor LezRecipient as Direction-specific LEZ recipient
    actor ForeignRecipient as Direction-specific foreign recipient
    participant LEZ as LEZ leg
    participant Foreign as Foreign leg

    Maker->>Daemon: Configure pair, price, limits, nodes
    Daemon->>Store: Persist policy and signed offer
    Daemon->>Chat: Publish offer
    Taker->>Chat: Discover and negotiate offer
    Chat->>Daemon: Deliver authenticated request
    Daemon->>Store: Persist immutable direction and safety profile
    Taker->>TakerLeg: Construct and submit first lock
    TakerLeg-->>Daemon: Validated confirmations
    Daemon->>Store: Persist lock evidence before next effect
    Daemon->>MakerLeg: Construct and submit second lock
    MakerLeg-->>Taker: Validated confirmations
    Note over Chat: May disappear permanently now
    alt ZEC pair
        LezRecipient->>LEZ: Claim first and reveal preimage
        LEZ-->>Daemon: Canonical claim evidence contains preimage
        ForeignRecipient->>Foreign: Follow on ZEC with extracted preimage
    else BTC pair
        Taker->>MakerLeg: Taker claims maker-funded leg
        MakerLeg-->>Daemon: Claim witness reveals adaptor material
        Maker->>TakerLeg: Maker claims taker-funded leg
    else XMR pair, LEZ-first direction only
        Maker->>TakerLeg: Maker claims taker-funded LEZ
        TakerLeg-->>Taker: Canonical LEZ claim evidence reveals recovery material
        Taker->>MakerLeg: Taker spends maker-funded XMR output
    end
    Daemon->>Store: Persist terminal chain evidence
    Daemon-->>Maker: Report completed swap
```

“Taker-funded” and “maker-funded” describe funding order, not a fixed chain.
BTC and ZEC support both product directions; XMR is LEZ-first only. Claim and
refund order is construction-specific, as fixed in ADR 0008 and ADR 0010.

## Abandonment and autonomous recovery flow

```mermaid
sequenceDiagram
    actor Maker as Maker operator
    participant Daemon as Maker daemon + watcher
    participant Store as Durable recovery state
    actor Taker
    participant LEZ as LEZ sequencer
    participant Foreign as Bitcoin Core / monerod / Zebra
    participant TakerLeg as Taker-funded chain
    participant MakerLeg as Maker-funded chain
    participant Chat as Delivery / Chat
    actor LezFunder as Direction-specific LEZ funder
    actor ZecFunder as Direction-specific ZEC funder

    Note over Chat: Offline; neither recovery path calls it
    alt Maker never submits the second lock
        Taker->>Store: Load first-lock recovery data
        Taker->>TakerLeg: Wait for its typed recovery condition
        Taker->>TakerLeg: Submit first-lock refund
    else Both locks exist but the revealing claimant abandons
        Daemon->>Store: Reconcile canonical chain evidence
        alt ZEC
            LezFunder->>LEZ: Submit earlier LEZ refund
            LEZ-->>Daemon: Canonical LEZ refund evidence
            ZecFunder->>Foreign: Submit later ZEC CLTV refund after margin
        else BTC
            Daemon->>MakerLeg: Recover maker-funded leg at its reviewed path
            Taker->>TakerLeg: Recover taker-funded leg at the later safe path
        else XMR
            Taker->>LEZ: Refund taker-funded LEZ
            LEZ-->>Daemon: Canonical LEZ refund event
            Daemon->>Foreign: Recover XMR with reviewed key-share evidence
        end
    end
    Daemon->>Store: Persist refund proofs and terminal state
    Daemon-->>Maker: Report recovered balances
```

Each actor retains enough local data to recover without the counterparty.
Deadline comparisons are typed by chain and clock domain. ZEC always refunds
LEZ first and ZEC later by the configured margin. XMR recovery is gated by a
canonical LEZ event rather than a fictitious Monero deadline.

## Crash, restart, and at-least-once observation flow

```mermaid
sequenceDiagram
    actor User as Maker operator / Taker
    participant Surface as CLI / mini-app
    participant Process as Daemon / taker coordinator
    participant Store as SQLite aggregate + outbox
    participant Node as Selected chain node

    User->>Surface: Request transition with durable request ID
    Surface->>Process: Authenticated command
    Process->>Store: Transactionally persist intent + outbox item
    Process--xProcess: Crash at any instruction boundary
    User->>Process: Restart process
    Process->>Store: Load aggregate, pending effects, observations
    Process->>Node: Reconcile canonical chain state first
    Node-->>Process: Confirmed, replaced, removed, or absent
    Process->>Store: Idempotently apply validated evidence
    alt Effect is still required
        Process->>Node: Submit byte-identical or safely replaced transaction
        Node-->>Process: Transaction identifier / rejection
        Process->>Store: Persist result and close outbox item
    else Effect already exists or became invalid
        Process->>Store: Close, suspend, or replace effect without duplication
    end
    Process-->>Surface: Current durable status
    Surface-->>User: Resume without double spend or state leakage
```

The final coordinator uses one durable aggregate per swap and atomic intent plus
outbox persistence. Chain observations are at least once and therefore
idempotent; conflicting evidence is rejected. Current M1 proves restart and
observation idempotence for the prototype store. Atomic outbox, secret
encryption, process-kill matrices, and pair transaction resubmission are M5 exit
evidence, not present claims.

## Delivery-to-architecture map

| Boundary | Required proof | Milestone |
|---|---|---|
| Deterministic lifecycle and safety profiles | Pair acceptance/property tests | M1, then retained |
| LEZ and foreign-chain construction | Published vectors plus consensus-node acceptance/rejection | M2–M4 |
| Independent maker/taker processes | Black-box role matrix using real credentials, nodes, and funds | M2–M5 |
| Crash-safe coordinator | Kill/restart/outbox/reorg matrix | M5 |
| Maker CLI and Logos Core lifecycle | Authenticated operator journeys | M5 |
| Maker/taker mini-apps | Same role suites through UI process boundaries | M6 |
| Public testnets and review | Happy/refund/concurrency recordings and remediation packet | M7 |

No milestone is complete merely because an internal API test passes. Its tag
must point to the commit whose role-real evidence crosses every applicable
boundary above.
