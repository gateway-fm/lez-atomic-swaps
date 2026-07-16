# System architecture and actor flows

Status: Living target architecture — 2026-07-16

This is the canonical whole-system view. ADRs record why individual choices
were made; this document shows how the choices compose into the product that
operators and takers actually run. Dashed components are required final
deliverables that are not implemented yet. Blue components are implemented
boundaries that may have partial live exercise but have not completed the shown
end-to-end boundary. A test
is called end to end only when it crosses the same process, RPC, persistence,
role, and chain boundaries shown here.

## Canonical M2 build, deployment, and actor boundary

```mermaid
flowchart LR
    Source["LEZ v0.2.0 + SPEL PR 238 source"]
    Builder["Pinned Risc0 Docker builder<br/>r0.1.94.1 immutable digest"]
    ELF["ELF c85055...9d2e"]
    Program["ImageID and ProgramId<br/>5cf8c5...29c1"]
    Deploy["ProgramDeployment<br/>tx bd1680...733f"]

    subgraph LezDevnet["Private actual-node LEZ v0.2 devnet"]
        Sequencer["Sequencer JSON-RPC 3040<br/>host proof port 32832"]
        Bedrock["Bedrock HTTP 18080<br/>host proof port 32831"]
        Indexer["Indexer JSON-RPC 8779<br/>host proof port 32833"]
        Finalized["Finalized block 2582<br/>hash d2c494...6860"]
    end

    subgraph RoleProcesses["Independent actor boundaries"]
        Maker["Maker actor + SQLite"]
        MakerSidecar["Maker LEZ sidecar"]
        Taker["Taker actor + SQLite"]
        TakerSidecar["Taker LEZ sidecar"]
    end

    Zebra["Zebra 5.2.0 Regtest JSON-RPC 18232<br/>host proof port 32834"]
    Forward["TakerSellsLez Completed"]
    Reverse["TakerSellsForeign Completed"]

    Source --> Builder
    Builder --> ELF
    ELF --> Program
    Program --> Deploy
    Deploy -->|"sendTransaction"| Sequencer
    Sequencer -->|"publish signed blocks"| Bedrock
    Indexer -->|"poll finalized channel"| Bedrock
    Indexer -->|"getBlockById and getBlockByHash"| Finalized
    Deploy --> Finalized
    Maker --> MakerSidecar
    Taker --> TakerSidecar
    MakerSidecar -->|"official v0.2 RPC"| Sequencer
    TakerSidecar -->|"official v0.2 RPC"| Sequencer
    Maker -->|"typed Regtest RPC"| Zebra
    Taker -->|"typed Regtest RPC"| Zebra
    Finalized --> Forward
    Finalized --> Reverse
    Zebra --> Forward
    Zebra --> Reverse
```

The canonical artifact above is the only current v0.2 deployment target: ELF
SHA-256
`c85055f6fe85b71535a322ba84ffc612f5d093954a721ba3b529428814dc9d2e`
and ImageID and ProgramId
`5cf8c5a4eedb3c2873956cb7898eb33a495407c9746fb1a065c99638159329c1`.
Deployment transaction
`bd16808ee91c9860e860830e7437148b3f4f81c632fc1b6d40350e20cc47733f`
is Finalized in block `2582`, hash
`d2c4944a936347207be7030bb39f6b8f21dfc3dc75e95afedb58e22ed1f96860`.
The exact source, toolchain, actor, LEZ transaction, Zebra transaction, balance,
and timing facts are retained in the
[canonical M2 certification packet](../evidence/m2-canonical-local-certification-20260714.json).
Earlier host-built ELF `40c9d37c...8021` and ProgramId `f8385049...0fbe`
remain solely as immutable historical-evidence identities. They are not
accepted by current manifests, actor configuration, or the corridor runner.
All displayed host ports are retained-proof addresses for one isolated run,
not defaults or reusable discovery values.

## M3 repository-owned actor PoC and retained operator evidence

Run `m3actor-20260716n`, completed at `2026-07-16T01:00:30Z` from repository
commit `6ded2f9b8ba9ec8e0cfbf06287da92d34256f91a`, is the successful
repository-owned end-to-end actor PoC. Fresh one-shot maker and taker actor
processes completed both `TakerSellsForeign` and `TakerSellsLez` against one
run-owned Bitcoin Core 31.1 Regtest and LEZ v0.2 service tuple. Both role stores
reached revision 4 `Completed` in both directions, and the replay pass added
zero submissions. The older 2026-07-15 operator-composed packet remains valid
historical component and protocol evidence, but is no longer substituted for
the repository-owned actor proof.

The successful run also closed two actual-version integration gaps. Core 31.1
requires `gettxspendingprevout` flags in one options object
`{mempool_only:false,return_spending_tx:true}`, rather than the older positional
boolean form. Finalized LEZ scans use bounded windows and each actor-sidecar
request has a finite 30-second timeout. Bounded fresh-process observation
retries may repeat reads only; they never re-arm or repeat a submission.

The current runner allocates every host port dynamically on literal loopback.
Bitcoin publishes no P2P port. Core, Bedrock, the sequencer, the indexer, both
sidecars, state roots, credentials, and Docker resources are owned by one
unique run identifier. The endpoint labels below are protocols and process
roles, not reusable port numbers.

```mermaid
flowchart TB
    Source["Pinned LEZ v0.2 source and witnessed guest"]
    Artifact["Exact guest ELF<br/>SHA-256 a199c5be...e293"]
    Deployment["Exact ProgramDeployment<br/>ProgramId 39b6a4db...4dec"]

    subgraph Bootstrap["Run-owned bootstrap authority"]
        Identity["Fresh maker and taker owner identities"]
        VaultDerive["Official owner-derived Vault account IDs"]
        CoreProvisioner["Core wallet, miner, and funding provisioner"]
        LezBootstrap["Exact guest deploy, finality audit, and Vault Claims"]
        Identity --> VaultDerive
    end

    subgraph MakerHost["Maker role boundary"]
        Maker["Maker operator"]
        MakerRunner["Maker role runner"]
        MakerActor["btc-reference-actor"]
        MakerStore[("Maker lifecycle, effect, BTC, and LEZ signer journals")]
        MakerSidecar["Maker sidecar<br/>capability-authenticated loopback"]
        Maker --> MakerRunner
        MakerRunner --> MakerActor
        MakerActor --> MakerStore
        MakerActor --> MakerSidecar
    end

    subgraph TakerHost["Taker role boundary"]
        Taker["Taker operator"]
        TakerRunner["Taker role runner"]
        TakerActor["btc-reference-actor"]
        TakerStore[("Taker lifecycle, effect, BTC, and LEZ signer journals")]
        TakerSidecar["Taker sidecar<br/>capability-authenticated loopback"]
        Taker --> TakerRunner
        TakerRunner --> TakerActor
        TakerActor --> TakerStore
        TakerActor --> TakerSidecar
    end

    subgraph BitcoinStack["Run-owned Bitcoin Core 31.1 Regtest"]
        Core["JSON-RPC<br/>dynamic literal-loopback port"]
        CoreState[("Run-owned chain, wallet, and deterministic Regtest funds")]
        Core --> CoreState
    end

    subgraph LezStack["Run-owned LEZ v0.2 private devnet"]
        Sequencer["Sequencer JSON-RPC<br/>dynamic literal-loopback port"]
        Bedrock["Bedrock HTTP<br/>dynamic literal-loopback port"]
        Indexer["Indexer JSON-RPC<br/>dynamic literal-loopback port"]
        Vaults[("Fresh derived maker and taker Vault accounts")]
        Escrow["Witnessed escrow program<br/>39b6a4db...4dec"]
        Sequencer -->|"publish signed channel blocks"| Bedrock
        Indexer -->|"read finalized channel"| Bedrock
        Sequencer --> Escrow
    end

    Source --> Artifact
    Artifact --> Deployment
    Deployment --> LezBootstrap
    VaultDerive --> LezBootstrap
    LezBootstrap --> Sequencer
    LezBootstrap --> Indexer
    LezBootstrap --> Vaults
    CoreProvisioner -->|"cookie RPC for setup, mining, and funding only"| Core
    MakerActor -->|"restricted role Basic RPC"| Core
    TakerActor -->|"restricted role Basic RPC"| Core
    MakerSidecar -->|"signed transactions"| Sequencer
    TakerSidecar -->|"signed transactions"| Sequencer
    MakerSidecar -->|"finalized observations"| Indexer
    TakerSidecar -->|"finalized observations"| Indexer

    Terminal["Run m3actor-20260716n<br/>two of two directions revision 4 Completed"]
    MakerActor --> Terminal
    TakerActor --> Terminal
    Core --> Terminal
    Indexer --> Terminal
```

Fresh LEZ owners are generated for every actor run. The identity provisioner
derives each Vault account with the official Vault program function and emits
both owner and derived Vault identities. Genesis supplies the owner identity;
the sequencer derives and funds its Vault. Readiness and onboarding then query
that same derived Vault and submit exactly one owner-signed Vault Claim. This
prevents a fresh owner from accidentally being paired with a stale static
Vault identifier.

The runner independently hashes the supplied guest artifact, requires
`a199c5be...e293`, deploys ProgramId `39b6a4db...4dec` exactly once, and uses
sequential bounded indexer reads to prove the deployment and both Vault Claims
finalized. Node acceptance alone is not finality. Run `m3actor-20260716n`
proved this bootstrap in the same fresh composition as both complete actor
directions: deployment finalized once in block 6, the maker Vault Claim in
block 9, and the taker Vault Claim in block 12.

```mermaid
sequenceDiagram
    actor Maker as Maker operator
    participant Provisioner as Run provisioner
    participant Locks as Run-owned lock operator
    participant MakerJournal as Maker BTC and LEZ journals
    participant MakerActor as Maker reference actor
    participant Core as Bitcoin Core
    participant Sequencer as LEZ sequencer
    participant Indexer as LEZ indexer
    participant TakerActor as Taker reference actor
    participant TakerJournal as Taker BTC and LEZ journals
    actor Taker as Taker operator

    Provisioner->>Provisioner: Generate fresh terms and actor identities
    Provisioner->>Provisioner: Prepare exact signed Bitcoin funding offline
    Provisioner->>Core: testmempoolaccept exact funding bytes
    Core-->>Provisioner: Read-only policy result
    Provisioner->>Maker: Finalize countersigned agreement
    Provisioner->>Taker: Finalize countersigned agreement
    Maker->>MakerJournal: Complete both domain signer sessions
    Taker->>TakerJournal: Complete both domain signer sessions
    Note over MakerJournal,TakerJournal: Agreement and both journals are durable before first effect
    Taker->>Locks: Authorize direction-derived first lock
    alt First lock is Bitcoin
        Locks->>Core: Broadcast exact pre-admitted funding bytes
        Provisioner->>Core: Mine exactly the planned next block
        Core-->>MakerActor: Agreement-bound anchor and confirmation
        Core-->>TakerActor: Agreement-bound anchor and confirmation
    else First lock is LEZ
        Locks->>Sequencer: Initialize and fund witnessed escrow
        Indexer-->>MakerActor: Finalized exact funding evidence
        Indexer-->>TakerActor: Finalized exact funding evidence
    end
    MakerActor->>MakerActor: Drive and project matching revision 1
    TakerActor->>TakerActor: Drive and project matching revision 1
    Maker->>Locks: Authorize direction-derived second lock
    alt Second lock is Bitcoin
        Locks->>Core: Broadcast exact pre-admitted funding bytes
        Provisioner->>Core: Mine exactly the planned next block
        Core-->>MakerActor: Agreement-bound anchor and confirmation
        Core-->>TakerActor: Agreement-bound anchor and confirmation
    else Second lock is LEZ
        Locks->>Sequencer: Initialize and fund witnessed escrow
        Indexer-->>MakerActor: Finalized exact funding evidence
        Indexer-->>TakerActor: Finalized exact funding evidence
    end
    MakerActor->>MakerActor: Drive and project matching revision 2
    TakerActor->>TakerActor: Drive and project matching revision 2
    Note over MakerActor,TakerActor: Both lock gates are canonical before scalar use
    Taker->>TakerActor: Drive actor-owned revealing claim
    alt TakerSellsLez reveals on Bitcoin
        TakerActor->>Core: Submit exact Bitcoin claim once
        Provisioner->>Core: Mine one local claim block
        Core-->>TakerActor: Confirmed exact claim bytes
    else TakerSellsForeign reveals on LEZ
        TakerActor->>Sequencer: Submit exact LEZ claim once
        Indexer-->>TakerActor: Finalized exact claim bytes
    end
    TakerActor->>TakerActor: Project revision 3
    MakerActor->>MakerActor: Observe revealing claim and project revision 3
    Maker->>MakerActor: Drive actor-owned follow-up claim
    alt TakerSellsLez follows up on LEZ
        MakerActor->>Sequencer: Submit exact LEZ claim once
        Indexer-->>MakerActor: Finalized exact claim bytes
    else TakerSellsForeign follows up on Bitcoin
        MakerActor->>Core: Submit exact Bitcoin claim once
        Provisioner->>Core: Mine one local claim block
        Core-->>MakerActor: Confirmed exact claim bytes
    end
    MakerActor->>MakerActor: Project revision 4 Completed
    TakerActor->>TakerActor: Observe follow-up and project revision 4 Completed
    Note over Maker,Taker: Delivery and Chat are absent after lock and no cross-chain atomic commit is claimed
```

At the current PoC boundary, the run-owned operator constructs and submits both
locks and owns local mining. Each public actor independently observes those
exact lock facts for revisions one and two. Claim construction, durable
one-attempt authority, submission, canonical observation, and revisions three
and four are actor-owned. Production work may move lock construction behind a
product SDK without changing the agreement or chain-adapter contracts.

The older operator facts remain in the
[M3 local evidence packet](../evidence/m3-local-two-direction-poc-20260715.json).
The repository-owned proof is the secret-safe packet rooted at
`.e2e/m3actor-20260716n/m3-actor-poc/evidence/`, whose summary binds the exact
commit and executable hashes. It retains dynamic local endpoints, fresh
owner-derived Vault onboarding, the checked guest deployment, both role
journals, five unique effects per direction, four terminal role states, replay
submission counts, and exact cleanup attestation. It records no public RPC,
faucet, public funds, or private-material disclosure. Refund, concurrent,
process-kill, reorg, chaos, production key custody, and public deployment
journeys remain outside this happy-path PoC proof.

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
        DB[("Maker SQLite schema v10<br/>lock + claim + refund journals")]
        PS["BTC / XMR / ZEC pair SDKs"]
        ZA["Canonical dual-signed LEZ/ZEC agreement validator"]
        ZTX["ZEC BIP-199 V5 transaction SDK"]
        CA["Validated chain adapters"]
        MOA["Maker-only taker-lock observation"]
        OJ["Contiguous exact-tracker journal<br/>canonical / depth / same-tip replacement / removal"]
        ME["Fresh-gated durable maker second lock"]
        MLB["Context-owning LEZ SDK ports + adapter"]
    end

    subgraph TakerDevice["Taker-controlled device"]
        TC["Taker CLI"]
        TM["Taker mini-app"]
        TS["Taker pair SDK + durable recovery state"]
        TA["Taker-side concrete agreement validator"]
        TMO["Taker-only maker-lock observation"]
        TDB[("Taker SQLite schema v10<br/>role-local recovery")]
        TLB["Context-owning LEZ SDK ports + adapter"]
    end

    subgraph SharedSecurity["Shared SDK security boundary"]
        PCM["Protected preimage + exact claim payload<br/>XChaCha20-Poly1305 + HKDF<br/>schema-v10 envelope journal"]
        M3AJ[("M3 role-local adaptor journal<br/>reserve before commitment<br/>consume nonce with exact partial GREEN")]
        M3AS[("M3 taker-only adaptor scalar<br/>owner-private file; point check only at activation<br/>maker authority forbidden")]
        M3PE[("M3 role-local public-effect journal<br/>complete public bytes before one send CAS<br/>actor claim integration GREEN")]
        M3BR[("M3 BTC lifecycle recovery store<br/>four evidence revisions + hash chain<br/>offline Completed status GREEN")]
        M3BC["M3 typed Core 31.1 adapter<br/>stable-tip evidence + durable one-attempt submit<br/>GREEN component"]
        M3RA["btc-reference-actor<br/>schema 2 claim authority<br/>revisions zero through four GREEN in source"]
    end

    subgraph LezSidecars["Role-isolated official LEZ v0.1.2 processes"]
        MSL["Maker sidecar<br/>official wire + signer + durable cache"]
        TLS["Taker sidecar<br/>official wire + signer + durable cache"]
    end

    subgraph LocalLezFixture["Run-scoped exact-v0.1.2 node fixture"]
        ELN["Reusable external standalone process<br/>checked guest + fresh mode-0700 home"]
        LRM[("Private mode-0600 readiness<br/>endpoint + deployment tx/block + ProgramId<br/>built-in owner + funded actor keys")]
        LRR["Future reference-actor runner<br/>splits role-local endpoint and key files"]
    end

    subgraph LezV02Sidecars["Official LEZ v0.2 sidecar boundary"]
        MSL2["Maker v0.2 PoC process<br/>Vault Claim + native deposit GREEN"]
        TLS2["Taker v0.2 PoC process<br/>Vault Claim + native claim GREEN"]
        V02J[("Separate role-bound state<br/>exact reservations + Vault attempt journals GREEN")]
        MBR2["Maker lez-v02-bridge-poc<br/>canonical forward and reverse complete"]
        TBR2["Taker lez-v02-bridge-poc<br/>canonical forward and reverse complete"]
        M3WB["M3 witnessed prepare, complete, and submit<br/>both local happy directions GREEN"]
        M3FF["M3 finalized witnessed-funding observer<br/>parent-linked stable-tip ancestry<br/>historical Funded state GREEN"]
        M3FO["M3 finalized witnessed-claim observer<br/>parent-linked stable-tip ancestry<br/>dual role + BIP340 GREEN"]
        MBRJ[("Maker-only request store<br/>PREPARE replay + submit unknown-before-I/O GREEN")]
        TBRJ[("Taker-only request store<br/>PREPARE replay + submit unknown-before-I/O GREEN")]
        MSL2 --> V02J
        TLS2 --> V02J
        MBR2 --> MBRJ
        TBR2 --> TBRJ
        MBR2 --> M3WB
        TBR2 --> M3WB
        M3WB --> V02J
        MBR2 --> M3FF
        TBR2 --> M3FF
        MBR2 --> M3FO
        TBR2 --> M3FO
    end

    subgraph LocalLezV02["Required public-compatible local LEZ v0.2 devnet"]
        BR["Bedrock HTTP 18080<br/>retained proof host 32831"]
        IX["LEZ v0.2 indexer RPC 8779<br/>retained proof host 32833"]
        SQ["LEZ v0.2 sequencer RPC 3040<br/>retained proof host 32832"]
        V02R["Host orchestrator<br/>exact-ID lifecycle and RPC probes"]
        V02Net["Unique no-masquerade Docker bridge<br/>dynamic loopback ports"]
        V02Ready[("v0.2 services + Vault Claims + canonical deploy GREEN<br/>ProgramId 5cf8c5...29c1")]
        V02Native[("Native init + fund + claim GREEN<br/>finalized blocks 219 220 223")]
        V02Fixture[("Fixture readiness GREEN<br/>isolated configs; saved window stale")]
        V02Partial[("Historical host-built evidence retained<br/>14d through 14o and reverse 14c")]
        V02Full[("Canonical ZEC corridor directions GREEN<br/>2 of 2 happy directions")]
        V02M3[("M3 aggregate-witness guest deployed<br/>ELF a199c5be...e293<br/>ProgramId 39b6a4db...4dec; two claims finalized")]
        V02State[(".e2e/run_id/lez-v02")]
    end

    subgraph OffChain["Untrusted, removable after lock"]
        DC["Delivery / Chat"]
    end

    subgraph Nodes["Actor-selected node boundary"]
        LEZ["LEZ sequencer<br/>dynamic local port<br/>private v0.2 execution GREEN<br/>public live execution deferred"]
        BTC["Bitcoin Core 31.1 Regtest<br/>both adaptor claim directions GREEN<br/>service mode and durable SDK/journal GREEN"]
        XMR["monerod + wallet RPC"]
        ZEC["Zebra 5.2.0 Regtest JSON-RPC<br/>retained proof host 32834"]
    end

    subgraph DormantRoutes["Dormant public route contracts; no public I/O evidence"]
        RouteGate["Schema-v3 route + signed runtime<br/>pre-persistence validation"]
        LezProfile{"Sidecar outbound LEZ profile"}
        PublicLez["Exact HTTPS<br/>testnet.lez.logos.co"]
        PublicLezRisk["Finalized-tip method<br/>availability unknown"]
        ZebraProfile{"Actor Zebra route"}
        SelfHostedZebra["Self-hosted loopback<br/>cookie authentication"]
        TatumZebra["Exact Tatum Testnet HTTPS<br/>x-api-key authentication"]
    end

    subgraph PublicIdentity["Dormant public deployment identity handoff"]
        V02Deploy["Exact-once v0.2 deployment client<br/>fixed official RPC"]
        V02AuthKey[("Separate owner-only 32-byte<br/>evidence authentication key")]
        V02Evidence[("Bounded HMAC-authenticated evidence<br/>channel + genesis + program + tx + block")]
        V02Target["Canonical Docker target<br/>ELF c85055...9d2e<br/>ProgramId 5cf8c5...29c1"]
        V02Provision["Offline provision-identity<br/>trusted target + no-clobber"]
        V02Runtime[("Exact public runtime identity")]
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
    PS --> ZA
    PS --> ZTX
    PS --> CA
    PS --> MOA
    PS --> ME
    PS --> MLB
    ZTX --> CA

    T --> TC
    T --> TM
    TC --> TS
    TS --> TA
    TS --> TMO
    TS --> TDB
    TS --> TLB
    TMO --> TDB
    TMO -->|"signed direction selects one node"| LEZ
    TMO -->|"signed direction selects one node"| ZEC
    PS --> PCM
    TS --> PCM
    PS --> M3AJ
    TS --> M3AJ
    TS -->|"taker only owner-private authority"| M3AS
    PS -->|"maker private config"| M3RA
    TS -->|"taker private config"| M3RA
    M3AJ -->|"existing exact-role Bitcoin and LEZ journals"| M3RA
    M3AS -->|"stable read and agreement point check"| M3RA
    M3WB -->|"full prepared claim result"| M3RA
    M3RA -->|"predecessor CAS projections one through four"| M3BR
    M3RA -->|"agreement-derived Bitcoin funding and claim"| M3BC
    M3RA -->|"signed-account finalized LEZ funding read"| M3FF
    M3RA -->|"signed transcript finalized LEZ claim read"| M3FO
    M3RA -->|"persist exact actor-owned claim before presence"| M3PE
    PCM -->|"encrypted envelope + journal"| DB
    PCM -->|"encrypted envelope + journal"| TDB
    TM -.-> TS

    MLB <-->|"bounded authenticated lez_bridge.v1"| MSL
    TLB <-->|"bounded authenticated lez_bridge.v1"| TLS
    MSL -->|"pinned generated JSON-RPC"| LEZ
    TLS -->|"pinned generated JSON-RPC"| LEZ
    ELN -->|"start exact upstream service"| LEZ
    LEZ -->|"official health, tx/block, static built-in, account RPC"| ELN
    ELN -->|"atomic no-clobber publish after verification"| LRM
    LRM -.->|"future private handoff"| LRR
    LRR -.->|"maker-only provisioning"| MSL
    LRR -.->|"taker-only provisioning"| TLS
    MLB <-->|"live bounded bridge; canonical runs Completed"| MBR2
    TLB <-->|"live bounded bridge; canonical runs Completed"| TBR2
    MSL2 -->|"official v0.2 JSON-RPC"| SQ
    TLS2 -->|"official v0.2 JSON-RPC"| SQ
    MBR2 -->|"reveal forward; initialize and fund reverse"| SQ
    TBR2 -->|"initialize and fund forward; reveal reverse"| SQ
    MBR2 -->|"non-genesis finalized-tip readiness"| IX
    TBR2 -->|"non-genesis finalized-tip readiness"| IX
    M3FO -->|"bounded ID and hash blocks parent-linked through stable tip<br/>unique transcript + accounts at containing BlockId"| IX
    M3FF -->|"bounded ID and hash blocks parent-linked through stable tip<br/>canonical FundNative + historical Funded accounts"| IX
    MBR2 -->|"typed outbound profile"| LezProfile
    TBR2 -->|"typed outbound profile"| LezProfile
    LezProfile -->|"local explicit loopback"| SQ
    LezProfile -->|"local explicit loopback"| IX
    LezProfile -.->|"official_public"| PublicLez
    PublicLez -.-> PublicLezRisk
    V02Deploy -.->|"future owner-authorized deployment"| PublicLez
    V02AuthKey -->|"authenticate observed facts"| V02Deploy
    V02Deploy -->|"retain exact observed result"| V02Evidence
    V02Evidence --> V02Provision
    V02AuthKey -->|"verify before trust"| V02Provision
    V02Target --> V02Provision
    V02Provision --> V02Runtime
    V02Runtime -.->|"future signing and role provisioning"| RouteGate
    V02R -->|"start first; cryptarchia and channel HTTP"| BR
    V02R -->|"start after channel; finalized ID and hash RPC"| IX
    V02R -->|"start after exact missing proof; service RPC"| SQ
    V02R --> V02Net
    V02R --> V02State
    V02Net --> BR
    V02Net --> IX
    V02Net --> SQ
    SQ -->|"Zone SDK signed publish"| BR
    IX -->|"poll finalized LEZ channel"| BR
    V02R -->|"write run-scoped evidence"| V02Ready
    V02Ready --> V02Native
    V02Ready --> V02Fixture
    ZEC -->|"stable mature Regtest UTXO query"| V02Fixture
    V02Native --> V02Partial
    V02Fixture --> V02Partial
    V02Ready -->|"canonical deployment finalized"| V02Full
    V02R -->|"digest-pinned build, recursive execution, official-wire completion"| V02M3
    M3WB --> V02M3
    V02M3 -->|"deployment and witnessed claims finalized"| SQ
    V02Fixture -->|"fresh role provisioning"| V02Full
    V02Full -->|"direction-derived funding and exact spend on Zebra"| ZEC
    V02Full -.->|"runtime and funding handoff"| LRR
    ZA --> RouteGate
    TA --> RouteGate
    RouteGate --> LezProfile
    RouteGate --> ZebraProfile
    PS --> ZebraProfile
    TS --> ZebraProfile
    ZebraProfile -->|"deterministic_local"| ZEC
    ZebraProfile -.->|"self_hosted_cookie"| SelfHostedZebra
    ZebraProfile -.->|"tatum_testnet_x_api_key"| TatumZebra

    MD <-->|"discovery + negotiation only"| DC
    TS <-->|"discovery + negotiation only"| DC
    CA --> LEZ
    CA --> BTC
    M3BC --> BTC
    CA --> XMR
    CA --> ZEC
    MOA -->|"signed direction selects one node"| LEZ
    MOA -->|"signed direction selects one node"| ZEC
    MOA -->|"validated event"| OJ
    OJ -->|"atomic role-local projection"| DB
    OJ -->|"full-history replay"| CO
    ME -->|"fresh exact-head query"| MOA
    ME -->|"intent and confirmed transition"| DB
    ME -.->|"typed production action pending"| CA
    TS --> LEZ
    TS --> BTC
    TS --> XMR
    TS --> ZEC
    LEZ --> LN
    SQ -.-> LN
    PublicLez -.-> LN
    BTC --> BN
    XMR --> XN
    ZEC --> ZN
    SelfHostedZebra -.-> ZN
    TatumZebra -.-> ZN

    classDef planned stroke-dasharray: 5 5,fill:#fff7e6,stroke:#9a6700;
    classDef implemented fill:#ddf4ff,stroke:#0969da;
    classDef running fill:#e6ffec,stroke:#1a7f37;
    class MM,LC,CA,TM,LRR,PublicLezRisk planned;
    class TC,M3RA,M3AS,M3PE,MBRJ,TBRJ,V02Partial,RouteGate,LezProfile,PublicLez,ZebraProfile,SelfHostedZebra,TatumZebra,V02Deploy,V02AuthKey,V02Evidence,V02Target,V02Provision,V02Runtime implemented;
    class BR,IX,SQ,V02R,V02Net,V02Ready,V02Native,V02Fixture,V02Full,V02State,MSL2,TLS2,V02J,MBR2,TBR2,M3FF,M3FO running;
```

The maker operator owns maker policy, keys, node selection, and the daemon
lifecycle. The taker owns a separate client, keys, node selection, and recovery
state. Logos Core is an optional lifecycle surface, never a protocol authority.
Delivery / Chat is not trusted with secrets or chain truth and may disappear
after the first lock. Chain adapters accept consensus evidence from the selected
LEZ sequencer, Bitcoin Core, `monerod`, or Zebra; peer messages never advance an
on-chain state by themselves.

The concrete LEZ/ZEC agreement validator is integrated on both actor sides as
one bounded canonical wire contract. Negotiation yields untrusted bytes;
role-fixed SDK instances validate and persist an accepted envelope before
activation, then revalidate its exact durable wire on resume without retaining
transport or raw adapter handles. The current executable SDK store is a
role-fixed production SQLite adapter for accepted agreement, both lock intents,
taker projection, maker-independent observation replay, confirmed maker funding,
and the taker-local observed-maker transition. Schema-v10 claim and refund
journals protect exact owner material, keep observer paths secret-free, and
replay separate role stores to `Completed` or `Refunded` in both directions.
The official native-refund sidecar, main revealing-claim/refund validation
adapters, both-direction agreement-bound Zebra funding discovery, and
context-owning LEZ SDK ports are now GREEN. The production fresh-client
factory, actor-owned random request/window allocator, and cloneable shared
role-local operation journal close the corresponding process-composition
prerequisites. Reopened role-local SQLite is now the canonical LEZ claim-funding
authority in both directions and reverse claims require the durable opposite
Zcash lock. The role-keyed Zcash funding/claim/refund signer retains only
zeroizing key bytes and uses the canonical SDK builders. The
agreement-committed exact-outpoint planner, checked all-trait Zebra composite,
refund-aware full-history resume, and the mode-0600, path-isolated one-shot
maker/taker CLI boundary are GREEN. Offline `status` now opens only a
pre-existing hardened role store, replays all durable lock/claim/refund state
with chain ports impossible by type, and leaves missing state uncreated. The
v0.2 sidecar foundation now binds the exact official LEE account and
transaction types, canonical signed-transaction decoder, sequencer
health/channel RPC, and an authenticated role/run-bound describe server. Its
native escrow planner prepares and validates an exact signed initialize/fund
pair using node-observed consecutive nonces. Its Vault planner prepares and
validates the exact official maker/taker Claim transactions with role-specific
allocations and an independently confirmed owner nonce. Both optionally persist
their exact reservation in a Linux owner-only no-symlink directory and recover
the same validated signed bytes after restart without another nonce lookup.
The native refund planner separately derives the official metadata, custody, and
immutable depositor account order, constructs an unsigned `RefundNative` public
transaction with zero nonces and witnesses, and durably reserves the exact bytes
before exposure. It accepts strict legacy and witnessed terms, revalidates the
aggregate authority binding for the witnessed shape, restores byte-identically
after restart without a nonce lookup, and admits only the retained transaction
to the generic submission boundary. This planner performs no RPC and does not
claim deadline eligibility or finality.
Their role-bound SQLite effect journal now exposes the narrow Vault Claim
one-attempt state machine: it commits exact typed preparation and
`AttemptStarted` before the only official-transaction call and makes every
reopen observe-only. Forced races, crash windows, ambiguity, error
classification, schema tampering, and filesystem substitution are GREEN in
library tests. The retained local v0.2 stack then proved both owner-authorized
Vault Claims, checked deployment, and separate maker/taker PoC processes driving
native initialize/fund/claim into finalized blocks 219/220/223. The native CLI
itself proves sequencer inclusion and same-tip state; sequential indexer reads
provide the distinct finality proof. It reports `crash_atomic_submission=false`.
The exact `lez-v02-bridge-poc` source now provides the role/run/runtime/signer-
bound process boundary for prepare, observe, revealing claim, and submit. It
requires an explicit nonzero loopback actor listener and private file inputs.
Its outbound node profile accepts either explicit loopback HTTP sequencer and
indexer URLs or the exact `https://testnet.lez.logos.co/` origin for both;
mixed or generic remote routes fail before client construction. It gates
startup on the official sequencer and indexer health calls, replays successful
PREPARE results, re-executes observations and transient PREPARE failures, and
persists submit as unknown before node I/O. The authenticated bridge
prepare-refund method reaches the internal durable planner, stores the canonical request/result, and reconstructs and compares it
before a restarted server binds. The repeatable observe-refund method now uses
fully covered finalized indexer ancestry, equal by-ID/by-hash blocks, historical
and tip accounts, and the containing-block deadline. It never submits or caches
chain truth. Actor recovery and actual-node timeout evidence remain open.
Sequencer observation remains bounded inclusion plus same-tip accounts. The
new witnessed-claim path separately asserts indexer finality through bounded
fully covered scans, equal by-ID/by-hash finalized blocks, exact aggregate
witness validation, and terminal accounts read at the containing block. Either
role-bound participant can observe without submitting, and the client
independently verifies BIP-340. Historical runs 14d through 14n remain failure
and invariant evidence. Fresh
pre-canonical run `m2poc-corridor-fresh-20260714o` completed `TakerSellsLez`: the taker
initialized and funded LEZ, the maker funded Zcash, waited for two
confirmations, claimed LEZ and revealed the preimage, and the taker spent the
Zcash HTLC. Both independent actors reached revision 4 `Completed` in 25.370
seconds. One payload-free `moving_tip` observation was retried once. A separate
indexer audit found the LEZ effects in finalized blocks 264/265/266 and proved
terminal `Claimed` metadata and zero custody; Zebra funding at height 106 was
spent at height 108.

The pre-canonical reverse run `m2poc-corridor-reverse-fresh-20260714c` completed
`TakerSellsForeign`: the taker funded Zcash, the maker initialized and funded
LEZ after the two confirmations, the taker claimed LEZ and revealed the
preimage, and the maker spent the Zcash HTLC. Both independent actors again
reached revision 4 `Completed`, this time in 26.960 seconds without a drive
retry. LEZ initialize/fund/claim finalized in blocks 641/642/643, and Zebra
funding at height 113 was spent at height 115. Two earlier effect-bearing
reverse attempts are retained and never reused; they exposed a canonical LEZ
validator that was hard-coded to the forward taker depositor. The correction
now validates the agreement-derived LEZ depositor and signer in both
directions. Those successful 14o and reverse 14c records used the host-built
`f8385049...0fbe` program and remain immutable historical evidence, not current
deployment authority.

Canonical certification rebuilt the guest in the pinned Docker builder as ELF
`c85055f6...9d2e`, ProgramId `5cf8c5a4...29c1`, deployed it in transaction
`bd16808e...733f`, and proved Finalized inclusion in LEZ block `2582`. Run
`m2cert-canonical-forward-bb53daf-20260714a` then completed
`TakerSellsLez` in 25.580 seconds over 38 drive rounds with two bounded retries;
Zebra advanced from height 121 to 124. Run
`m2cert-canonical-reverse-bb53daf-20260714a` completed
`TakerSellsForeign` in 28.790 seconds over 47 drive rounds without a retry;
Zebra advanced from height 124 to 127. Both independent actor stores reached
revision 4 `Completed`, both configs bound only the canonical ProgramId, and no
public RPC or faucet was used.

The development runner provisions fresh role inputs and executes independent
`activate`/`drive` processes. Before effects it acquires a nonblocking advisory
`flock` keyed by the SHA-256 of the configured sequencer, indexer, and Zebra
endpoint tuple. That lock serializes only users of the same retained nodes and
does not inspect, stop, or prune unrelated Docker resources. Its live atomic
guard permits the revealing LEZ claim only after the Zcash funding has two
confirmations, and permits the Zcash follow-up spend only after that LEZ reveal;
it also rejects a wrong role or duplicate chain effect. The saved early fixture
window 1..256 remains stale and is not reused. Both required local-devnet happy
directions are now GREEN. The dormant LEZ and Zebra public-route construction
contracts are also GREEN without public I/O. Restart, refund, reorg, chaos,
live-public behavior, and production hardening remain explicit later gates. Chain
adapters must
independently recompute every chain-derived account, input, and deadline. Maker
observation alone is non-authorizing: forward Zcash persists and revalidates
the complete canonical output type plus ordered canonical, depth, atomic
same-tip replacement, and affirmative exact-head removal events. The SDK and
store fold `ZcashObservationTracker`, so duplicate polls write nothing and
changed inclusion without replacement fails. Schema v10 rejects orphan, holey,
or history-incompatible rows and catches stale instances up before returning.
The maker effect invokes the distinct fresh pre-second-lock call internally,
then persists the exact direction-fixed opposite-chain plan before submission.
Confirmed Maker evidence commits atomically and replays to `BothLegsLocked` in
both directions without caching authority in `next_action`. Reverse LEZ rejects
primitive ID/depth assertions and
persists a stable primitive snapshot bound to the signed channel/genesis,
public fund transaction, canonical block/tip, complete SPEL metadata, exact
custody, depth, and finality policy. SDK and SQLite replay rerun the same
validator. The dependency-free exact-head tracker is now folded by the active
SDK and the schema-v10 journal. Exact duplicates write no row, while a
same-inclusion Pending-to-Finalized update advances one contiguous revision and
survives close/reopen. The pure tracker also proves affirmative same-tip
replacement, stale-evidence rejection, and fatal finalized-history changes.
Complete primitive removal/replacement records now carry nonfinal reorgs
through the active SDK and SQLite, while stale old-head evidence fails without
a row or revision change. Deterministic-local reverse fresh eligibility now
replays and re-queries the exact head and checks signed depth; local Pending
remains eligible when depth is sufficient, and no result is cached as
authority. The public Finalized/typed-finality policy and exact route can now be
constructed, but neither has been exercised against a public node. The official
LEZ origin's availability of the required finalized-tip/indexer method remains
an upstream release risk. The official
v0.1.2 node/escrow, revealing-claim, and native-refund owner/discovery ports,
main escrow/claim/refund agreement conversion, and crash-safe context-owning
SDK-port wiring are GREEN lower compatibility evidence. Public deployment is
deferred under ADR 0023. The full local v0.2 runtime tuple and independent actor
processes are GREEN in both happy directions. Dormant public configuration
contracts are GREEN; public live execution, actual-node restart/refund/reorg
and maker-fault evidence, and production adapter composition remain open.

## Dormant route admission and actor flow

```mermaid
flowchart LR
    Evidence["HMAC-authenticated deployment evidence"]
    AuthKey["Separate owner-only authentication key"]
    TrustedTarget["Immutable manifest + compiled target"]
    Provision["Offline provision-identity<br/>no RPC + no-clobber"]
    RuntimeIdentity["Exact public runtime identity"]
    Config["Untrusted schema-v3 actor config"]
    Terms["Dual-signed agreement + runtime descriptor"]
    Validate["Validate route, credentials, role, signer,<br/>network, branch, genesis, channel, and program"]
    Reject["Reject before persistence or effects"]
    Accept["Accepted role-local runtime"]

    Evidence -.->|"future public provisioning"| Provision
    AuthKey --> Provision
    TrustedTarget --> Provision
    Provision -.-> RuntimeIdentity
    RuntimeIdentity -.->|"sign into both inputs"| Config
    RuntimeIdentity -.->|"sign into both inputs"| Terms
    Config --> Validate
    Terms --> Validate
    Validate -->|"invalid"| Reject
    Validate -->|"valid"| Accept

    Accept --> ZebraRoute{"Actor Zebra route"}
    ZebraRoute -->|"deterministic_local"| Regtest["Loopback Zebra Regtest"]
    ZebraRoute -.->|"self_hosted_cookie"| SelfHosted["Loopback Zebra + cookie"]
    ZebraRoute -.->|"tatum_testnet_x_api_key"| Tatum["Exact Tatum HTTPS + x-api-key"]

    Accept --> Bridge["Loopback bridge client + capability"]
    Bridge --> Sidecar["Role-isolated official-wire sidecar"]
    Sidecar --> LezRoute{"Outbound LEZ profile"}
    LezRoute -->|"local"| LocalLez["Loopback sequencer + indexer"]
    LezRoute -.->|"official_public"| PublicLez["Exact official LEZ HTTPS"]
    PublicLez -.-> Risk["Finalized-tip method availability unknown"]
```

Solid route edges are the actual local evidence paths. Dashed edges are dormant
configuration and client-construction contracts proven without public network
calls. They do not claim endpoint availability, authentication success,
funding, deployment, transaction propagation, finality, or provider behavior.

The protected-claim module derives per-context keys with HKDF-SHA256 and encrypts
preimages and bounded exact claim-submission bytes with XChaCha20-Poly1305 while
binding schema, swap, pair, direction, agreement, role, purpose, and key ID.
Schema-v10 SQLite claim/refund intents and owner/observer transitions are
implemented with atomic revision commits, close/reopen replay, rollback,
conflict, corruption, and legacy-secret scrub coverage.

The dashed state reflects delivery honestly. The deterministic core, SQLite
repository, maker daemon, authenticated maker CLI flow, LEZ semantic
verification, pinned SPEL/LEZ generated-IDL/client fixture, and ZEC exact-script
plus signed V5 spend foundation exist. Bounded spend recognition now matches
pinned Zebra consensus across all defined sighash modes and alternate valid
stack/signature encodings while reporting stricter SDK construction policy
separately. Deterministic actor-owned funding/change and pinned, vulnerability-clean
Zebra 5.2.0 Regtest acceptance/rejection/confirmation, concurrent swaps,
confirmation regression, exact rebroadcast, block reconsideration, and a
two-node conflicting four-over-three-block fork replacement now exist as
consensus-node proof. Source-correct authenticated-transfer/ATA custody and
generated owner-role clients now exist as locally composed upstream-program
evidence. The checked Risc0 guest now builds reproducibly, deploys through the
isolated standalone sequencer's JSON-RPC, and executes the complete native initialize/fund/claim/refund
lifecycle with real funded actor keys in an isolated v0.1.2 standalone
sequencer. Wrong-preimage, wrong-role, and early-refund transactions are
excluded from canonical blocks without mutating nonce or custody. Native
recursive cost evidence is machine-checked from production state transitions
without Clock noise. The official ATA lifecycle also passes for two independent
definitions with real owner roles and permissionless fixed refunds. Their
escrow/ATA/Token recursion is also included in the machine-checked cost record.
Public-testnet execution evidence is deferred to production readiness. The
full-runtime local v0.2 happy path and composed both-direction maker/taker
processes are evidence-backed PoC boundaries. Production adapter composition,
cross-chain deadline and refund composition, actual-node restart/reorg/chaos,
encrypted state/outbox, and mini-apps remain milestone work and cannot yet be
represented as production E2E.

The reusable external local-node boundary is narrower than that composed E2E.
It recomputes the tracked ELF SHA-256 and Risc0 ImageID before creating any
state, refuses an existing node home or readiness path, creates its own
mode-0700 home, and requests an upstream dynamic port. It publishes the
literal-loopback client endpoint only after official RPC verifies health,
genesis identity, mandatory chain progress, checked-guest deployment and
the exact deployment transaction and containing block, ProgramId, the static
authenticated-transfer built-in identity, plus two key-derived accounts owned
by that built-in with positive balances. Upstream `getProgramIds` is not a
deployment registry; custom guest authority comes from `getTransaction`, exact
`getBlock` membership, and ProgramId derived from the contained ELF. The
mode-0600 no-clobber readiness manifest includes those deterministic private
keys and is therefore a run-local secret. The
future actor edges are dashed because neither SDK actor consumes this handoff
in a composed LEZ/Zebra swap yet. This entire v0.1.2 boundary is retained as a
lower lane and cannot replace ADR 0023's full v0.2 stack. The upstream v0.1.2 server itself still binds
the allocated port on the host wildcard address.

The v0.2 service stack is source- and binary-attested and runs clean LEZ
`v0.2.0` source `a58fbce2...`, Rust 1.94.0 service artifacts, the digest-pinned
Bedrock image, exact Risc0/Rapisnark inputs, and dynamic loopback RPCs on a
unique no-masquerade bridge. Historical pre-canonical run
`m2poc-vertical-20260714a` retained that stack
while Vault Claims finalized in blocks 29/30, the checked escrow deployed in
block 51, and the native lifecycle finalized in blocks 219/220/223. Terminal
custody/maker/taker balances were 0/99300/200700. A keyless process observed the
same terminal state, and the actor-fixture provisioner selected a 625000000-zat
maker-owned Zebra output at 104 confirmations. That retained vertical run was
not itself a cross-chain swap. Subsequent pre-canonical runs 14o and reverse
14c composed the same services but used historical ProgramId `f8385049...0fbe`.
The canonical Docker artifact `5cf8c5a4...29c1` was later deployed in finalized
block 2582 and rerun through both corridors as
`m2cert-canonical-forward-bb53daf-20260714a` and
`m2cert-canonical-reverse-bb53daf-20260714a`. Composed restart, refund, reorg,
and fault recovery remain pending. PoC-to-hardening and milestone transitions
are owner-controlled.

## Verified M2 local actor and RPC flow

```mermaid
sequenceDiagram
    actor Maker
    actor Taker
    participant MakerBridge as Maker LEZ bridge
    participant TakerBridge as Taker LEZ bridge
    participant Sequencer as LEZ sequencer 32832
    participant Bedrock as Bedrock 32831
    participant Indexer as LEZ indexer 32833
    participant Zebra as Zebra Regtest 32834

    Note over Maker,Zebra: Endpoint-tuple flock is acquired before any effect
    alt TakerSellsLez in canonical forward run
        Taker->>TakerBridge: Initialize and fund LEZ escrow
        TakerBridge->>Sequencer: Submit signed native transactions
        Maker->>Zebra: Fund direction-derived BIP-199 HTLC
        Zebra-->>Maker: Two canonical confirmations
        Maker->>MakerBridge: Claim LEZ and reveal preimage
        MakerBridge->>Sequencer: Submit signed revealing claim
        Taker->>Zebra: Spend exact HTLC with observed preimage
    else TakerSellsForeign in canonical reverse run
        Taker->>Zebra: Fund direction-derived BIP-199 HTLC
        Zebra-->>Taker: Two canonical confirmations
        Maker->>MakerBridge: Initialize and fund LEZ escrow
        MakerBridge->>Sequencer: Submit signed native transactions
        Taker->>TakerBridge: Claim LEZ and reveal preimage
        TakerBridge->>Sequencer: Submit signed revealing claim
        Maker->>Zebra: Spend exact HTLC with observed preimage
    end
    Sequencer->>Bedrock: Publish signed LEZ blocks
    Indexer->>Bedrock: Read finalized LEZ channel
    Indexer-->>Maker: Finalized transaction IDs and block hashes
    Indexer-->>Taker: Finalized transaction IDs and block hashes
    Note over Maker,Taker: Both independent stores finish revision 4 Completed
```

The direction changes actor ownership, not the atomic reveal order. In both
canonical runs the Zcash funding is confirmed before the LEZ claim reveals the preimage,
and the exact Zcash follow-up spend occurs only after that reveal. The retained
host ports above are evidence addresses from these isolated runs, not reusable
service discovery values; every new run must receive an explicit fresh local
tuple.

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
    Manifest["Tracked artifact manifest"] --> Preflight["Verify exact manifest bytes<br/>ELF SHA-256 + ImageID"]
    ELF --> Preflight
    Preflight --> Process["External lez-standalone-node"]
    Process --> Home["Fresh mode-0700 node home"]
    R0VM["Exact r0vm 3.0.5"] --> Clock["Mandatory clock execution"]
    Process --> Clock
    Clock --> ReadyBlock["Persisted readiness block"]
    ELF --> RPC["sendTransaction ProgramDeployment"]
    Process --> RPC
    RPC --> Mempool["Standalone mempool"]
    ReadyBlock --> Mempool
    Mempool --> Validate["Block-time deployment validation"]
    R0VM --> Validate
    Validate --> Block["Canonical persisted block"]
    Block --> Query["Exact getTransaction + containing getBlock"]
    Query --> DeploymentCheck["Verify deployment variant, hash,<br/>block identity and ELF-derived ProgramId"]
    Process --> BuiltIn["getProgramIds static built-ins<br/>bind authenticated_transfer only"]
    Actors --> ActorCheck["getAccount ownership + balance checks"]
    BuiltIn --> ActorCheck
    DeploymentCheck --> Readiness["No-clobber mode-0600 schema-v2 readiness<br/>endpoint + tx/block/program + built-in + keys"]
    ActorCheck --> Readiness
    Readiness -.-> ActorRunner["Reference actor runner"]
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
    V02Pins["LEZ v0.2.0 + exact SPEL PR head"] --> V02Guest["Risc0 v0.2 escrow guest"]
    V02Builder["Pinned Docker guest-builder<br/>Rust 1.94.1 + immutable digest"] --> V02Artifact["Canonical v0.2 ELF c85055...9d2e<br/>ImageID and ProgramId 5cf8c5...29c1"]
    V02Guest --> V02Builder
    V02Artifact --> V02Local["Recursive native + two-definition token<br/>claim/refund + rollback tests"]
    V02Artifact --> V02LocalDeployer["Checked local ProgramDeployment<br/>tx bd1680...733f"]
    V02LocalDeployer --> V02DeployProof["Indexer Finalized proof<br/>block 2582"]
    V02DeployProof --> V02FullLocal
    V02Artifact --> V02Deployer["Exact-once fixed-URL<br/>official-RPC deployer"]
    V02Artifact --> V02Sidecar["Official-wire prepare + one-attempt Claim GREEN<br/>actual-node effects GREEN"]
    V02Sidecar --> V02FullLocal["Bedrock + indexer + non-standalone sequencer<br/>both independent actor corridors GREEN"]
    V02Deployer -.-> Testnet["Official v0.2 testnet<br/>deployment + cost evidence"]

    classDef planned stroke-dasharray: 5 5,fill:#fff7e6,stroke:#9a6700;
    classDef running fill:#e6ffec,stroke:#1a7f37;
    class Testnet,ActorRunner planned;
    class V02FullLocal running;
```

The deployment proof uses port `0`, a fresh mode-0700 sequencer home,
deterministic genesis inputs, an exact `r0vm` path, and no shared Docker project
or chain state. The external process rejects the guest before home creation if
its bytes or manifest differ from the embedded tracked identity, and refuses to
reuse another activity's home or readiness path. Deployment admission alone is
deliberately insufficient: the readiness block proves the mandatory
clock/executor/store loop first, transaction lookup proves the deployment
reached the block store rather than only the mempool. The helper locates the
exact transaction in a post-submit block and retains its hash plus containing
block ID/hash. `getProgramIds` binds only the static authenticated-transfer
built-in used as actor owner; official account RPC then revalidates both
deterministic actors before a private schema-v2 mode-0600 handoff is published.
The solid native lifecycle uses the actual funded genesis roles,
validates signer-bound claim and permissionless-refund boundaries against
canonical block time, and asserts exact balances. The solid token lifecycle
uses two definitions, owner-signed ATA funding/claim, permissionless custody and
refund, and cross-definition substitution negatives. Token replay attributes
the escrow/ATA/nested-Token recursion while excluding setup and Clock noise.
The solid v0.2 branch builds an independently locked guest/generated client
through the pinned Risc0 Docker builder, binds ELF SHA-256
`c85055f6...9d2e` and ImageID and ProgramId `5cf8c5a4...29c1`, and runs
recursive native plus two-definition token claim/refund tests. Historical
`40c9d37c...8021` and `f8385049...0fbe` identities remain only with the
immutable pre-canonical evidence that produced them. A child-transfer overflow regression proves the metadata and every
touched account roll back together. The v0.2 deployer validates immutable
endpoint/channel/built-ins/artifact identity, submits once, and accepts only the
exact transaction in its containing block; ambiguity or timeout is never
retried. The solid v0.2 sidecar node represents tested describe/health/decoder,
native initialize/fund preparation, deterministic maker/taker Vault Claim
preparation, hardened durable exact-byte restart, and the role-bound
attempt-before-call Vault Claim submission state machine. The full-local edge
is solid because the canonical forward and reverse runs crossed the official
sidecar, three-service LEZ stack, independent actor state, and Zebra in both
happy directions after the exact deployment was finalized. Actual-node restart, refund, reorg, and fault recovery remain open.
The dashed public-testnet edge and deployed-runtime costs are deferred to production
readiness under ADR 0023. The v0.1.2 cost replay executes the same guest instructions through
LEZ production state transitions, counts the escrow root and
authenticated-transfer child, and compares generated JSON with the checked
evidence artifact.

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
    Primary->>Primary: Mine claim/refund branch to depth 3
    Funder->>Fork: Conflicting signed refund of claimant output
    Funder->>Fork: Same independent signed refund
    Fork->>Fork: Mine conflicting branch to depth 4
    Fork->>Primary: Relay four raw consensus blocks via submitblock
    Primary->>Primary: Accept higher-work branch and detach three blocks
    Primary-->>Claimant: Old claim no longer canonical
    Primary-->>Funder: Conflicting refund active with four confirmations
```

Both nodes use separate ephemeral loopback RPC ports, immutable tmpfs state,
non-root read-only images, and one uniquely named Compose project. This flow
proves Zebra consensus validation and best-chain replacement with actual actor
keys. It deliberately does not claim cross-chain refund-margin proof: that
requires typed ZEC observation commitments, checked LEZ millisecond/core-second
conversion, a named profile, and a composed standalone-LEZ plus Zebra run.

## Zcash profile and deadline flow

```mermaid
flowchart LR
    Terms["Signed profile ID + direction"] --> Select{"Named profile"}
    Node["Zebra network + consensus branch"] --> Validate{"Exact profile match?"}
    Select --> Validate
    Validate -->|"no"| Reject["Reject before funding"]
    Validate -->|"yes"| Depths["LEZ/ZEC confirmation depths"]
    Select --> LezDeadline["Checked LEZ seconds → guest milliseconds"]
    Select --> ZecDeadline["Checked funding height + CLTV blocks"]
    Telemetry["Measured bounds or controlled harness"] --> Safety["LEZ-latest + required margin ≤ ZEC-earliest"]
    Select --> Safety
    Terms --> Binding["Validated ZEC binding record v1"]
    Binding --> Journal
    Safety -->|"missing/short"| Reject
    Safety --> Schedule["Direction-mapped RecoverySchedule<br/>LEZ always earlier than ZEC"]
    Depths --> Validator["Typed observation validator"]
    Depths --> CoreFunding
    ZebraE2E["Stable actual Zebra E2E RPC snapshot"] --> Validator
    Validator --> Watcher["Stable-tip two-phase watcher"]
    Watcher --> Record["Validated primitive event record v1"]
    Record --> Journal["Versioned SQLite ZEC event journal"]
    Journal --> Projection["Runtime event → participant projection"]
    Terms --> CoreFunding["Participant-aware core funding/reorg API"]
    Projection --> CoreFunding
    Projection --> ConflictOutcome["ReplacementConflict classification"]
    Projection --> TerminalOutcome["TerminalReorgDetected classification"]
    ConflictOutcome --> AlertOutbox
    TerminalOutcome --> AlertOutbox["Durable operator/security alert outbox"]
    Validator --> Observe["Bound canonical/removal evidence"]
    LezDeadline --> Schedule
    ZecDeadline --> Schedule
    Observe --> Composed["Composed local LEZ v0.2 + Zebra<br/>both happy directions GREEN"]
    Schedule --> Composed

    classDef running fill:#e6ffec,stroke:#1a7f37;
    class Composed running;
```

The solid profile, validator, stable-tip watcher, and actual Zebra E2E snapshot
paths are implemented. The validator
re-decodes canonical bytes and checks the complete network, branch, block,
outpoint, value, exact script, and derived-depth binding before producing a
lossy coordinator proof. Public-testnet values are acceptance targets, not
mainnet recommendations or proof of worst-case cadence. Durable production
participant-aware core semantics are implemented for taker- and maker-funded
legs: removal pins the exact ID, suspends claims, exact reappearance restores
authority, conflicting replacement fails, and refunds remain available.
Independent leg policies also make maker-depth regression suspend and depth
recovery restore claims. The runtime event-to-participant path is now solid: the
isolated two-Zebra fixture drives real canonical and removal evidence through schema-v10 SQLite
close/reopen and exact replay. The composed local LEZ/ZEC happy-path corridor is solid for both directions in
the canonical forward and reverse certification runs. Its actual-node
restart/refund/reorg and recovery paths remain open. RPC errors or absence never imply removal: a detach event
requires a stable replacement tip and a changed canonical hash at the prior
inclusion height.

The runtime path passes direction-derived canonical/removal projection, atomic
commit, restart reload, exact unknown-outcome replay in both ZEC directions,
authenticated operator alert operations, and actual-node close/reopen/requery.
Restart revalidates primitive records and immutable binding terms,
rejects impossible sequence history, restores the exact historical tracker head,
and still requires a fresh stable Zebra reconciliation before effects.
Completed/refunded removal or replacement is now journaled without erasing the
lifecycle result and is classified as `TerminalReorgDetected`; its critical
alert now commits atomically and survives replay/restart.
Authenticated maker status/list/ack RPC and CLI flows surface that durable alert;
acknowledgment clears attention metadata without changing the protocol phase.
Post-dependent replacement now commits chain truth atomically, retains the
protocol-committed transaction ID in the participant-specific reorg phase, and
returns `ReplacementConflict`; pre-dependent replacement and same-transaction
re-mining remain normal applied outcomes.
The SDK binding record now revalidates primitive profile and BIP-199 terms,
including both derived scripts. Schema v4 persists it atomically with the swap
and rejects immutable rebinding. Runtime requires it before tracker restoration,
replay detection, or projection; both coordinator leg policies and every event
side must match the named profile and expected-output envelope.
The separate concrete agreement record now additionally binds both actor
signatures, exact LEZ chain/custody terms, Zcash transaction policy, refund
calibration, and the authenticated transcript. It remains pre-integration
evidence until activation and resume persist and revalidate that exact record.

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

ADR 0029 concretizes the BTC branch: distinct two-party aggregate claim
authorities protect the Bitcoin and LEZ legs. The first direction-specific
canonical claim—finalized LEZ bytes or a Bitcoin witness canonical at the
negotiated confirmation policy—reveals the agreed scalar, and the second
claimant adapts the opposite-chain signature. No standalone actor claim key may
bypass that transcript. The exact `f5a9caa` fixture uses public deterministic
maker/taker shares to aggregate and tweak `Q`, computes role-tagged nonce
commitments locally, produces both partials, verifies a 65-byte adaptor
presignature, adapts it with the public fixture scalar, verifies the 64-byte
result under `Q`, passes Core policy and consensus, and extracts the matching
scalar. That single-process fixture is lower-level evidence. The current
source path exchanges commitments through independent role journals, completes
both domain sessions, validates actual Core and official-wire LEZ effects, and
projects both claim revisions without persisting the recovered scalar.

ADR 0031 defines the public role-fixed process boundary. Separate
owner-private maker and taker configs invoke one fresh `btc-reference-actor`
process for `activate`, `drive`, or `status`. Only activation inserts agreement
acceptance. Absent or empty/no-acceptance state remains not activated; corrupt
or conflicting state fails closed. Status may migrate an existing database
schema but creates no acceptance, constructs no chain client, and performs no
RPC. At predecessors zero and one, drive selects respectively the taker- and
maker-funded chain from the validated agreement, receives typed Core funding or
finalized LEZ funding, binds LEZ accounts to the signed terms, and retains the
finalized tip before projecting the next revision. At predecessors two and
three, the local role completes or reproduces the exact revealing or follow-up
claim from the agreement and existing signer journal, persists the public
effect, submits only with single-winner authority, and requires canonical chain
evidence before projection. Normal pre-effect observer errors are retryable
unavailability, not false absence. Exact retries retain their deterministic
identity. A concurrent CAS loser may converge only on a valid matching winner;
other projection failures fail closed. Revisions one through four are GREEN in
source, deterministic adapter tests, and actual-node run `m3actor-20260716n`.
Actual-node refund, process-kill, reorg, concurrency, and chaos evidence remains
pending.

ADR 0033 supplies the reusable effect boundary used by both actor-owned claim
revisions. Both adaptor session databases reopen existing-only; a missing path
cannot create empty signer state. Complete public Bitcoin or LEZ transaction
bytes and signed-agreement authority are durable before a one-winner
`Prepared` to `Started` CAS permits the only fresh RPC submission. `Started` or
`Unknown` recovery observes exact bytes only, and a definitive conflict burns
fresh authority without calling the transport. This is crash-safe local
authority, not a cross-system atomic commit.

```mermaid
flowchart LR
    Agreement["Validated countersigned agreement"]
    Signers[("Existing maker and taker signer journals")]
    Actor["Role-fixed actor<br/>revisions zero through four"]
    Prepared["Complete public Bitcoin or LEZ transaction"]
    Effects[("Role local public effect journal")]
    Observe["Exact chain observation"]
    Core["Bitcoin Core loopback RPC"]
    Sidecar["Role LEZ sidecar loopback RPC"]
    Lifecycle[("BTC recovery lifecycle")]
    Evidence["m3actor-20260716n<br/>2 of 2 actual-node directions Completed"]

    Agreement --> Actor
    Signers --> Actor
    Actor --> Prepared
    Prepared --> Effects
    Effects --> Observe
    Observe --> Core
    Observe --> Sidecar
    Effects -->|"single Started winner"| Core
    Effects -->|"single Started winner"| Sidecar
    Core -->|"confirmed exact bytes"| Lifecycle
    Sidecar -->|"finalized exact bytes"| Lifecycle
    Lifecycle --> Actor
    Actor --> Evidence
```

Pushed `0177151` adds a production-shaped in-memory boundary alongside that
retained Core fixture. Separate maker/taker state objects use fresh OS nonces,
exchange transcript-bound commitments before reveal, verify peer partials, and
bind distinct BTC and LEZ messages to the same adaptor point. Either completed
signature reveals the scalar needed to complete the other. Pushed `e3f2938`
adds the role-local SQLite journal that reserves the nonce before commitment,
gates reveal on a durable peer commitment, and consumes the nonce atomically
with the exact replayable partial. Pushed `8a7ea55` adds SDK generation and
restart reconstruction for the exact journal material, complete-context and
both-commitment checks, partial verification, and aggregate-presignature
verification. Pushed `6935acd` adds the checked LEZ guest:
the aggregate account authorizes the exact transaction while the distinct
claimant receives the escrow. That retained component fixture still shares one
process, but the 2026-07-15 composition crossed both actual local nodes through
independent maker and taker role processes and stores. Both happy directions
completed, and the recovered scalar matched the committed point without being
retained in public evidence. The canonical version-one agreement is now
implemented and reconstructs the exact aggregate key, P2TR/CSV contract,
funding outpoint, cooperative transaction/sighash, Bitcoin chain policy, LEZ
terms, and recovery schedule before accepting both role signatures. The public
actor activates that validated record, observes the agreement-derived taker and
maker locks through typed chain adapters, completes the direction-derived
Bitcoin or LEZ revealing and follow-up claims under the local role, and
persists all four transitions. Run `m3actor-20260716n` executes that
repository-owned workflow against fresh actual Core/LEZ services and proves
both actors terminal `Completed` in both directions. This closes the local
happy-path PoC only; the production-hardening nonclaims above still apply.

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

    Note over Chat: Offline and neither recovery path calls it
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

## Offline actor status flow

```mermaid
flowchart LR
    User["Maker or taker"] --> Command["One-shot status command"]
    Command --> Config["Private role-fixed schema-v3 config"]
    Config --> Inspect["Inspect role-store path"]
    Inspect -->|"missing"| Missing["Versioned not_activated output"]
    Inspect -->|"exists"| Material["Load claim-recovery key only"]
    Material --> Open["Open existing SQLite with NOFOLLOW and identity checks"]
    Open --> SDK["Role-fixed ZEC SDK with unit LEZ and Zcash ports"]
    NoChain["No sidecar, Zebra credential, or chain capability"] --> SDK
    SDK --> Replay["Replay agreement, locks, claims, and refunds"]
    Replay --> Output["Secret-free phase, revision, and next action"]
```

`status` is an implemented store-recovery path, not a configuration-only
placeholder. Missing SQLite returns `not_activated` without creating a file.
Existing state is opened with the existing-only hardened SQLite entry point and
replayed through `resume_all_capable`; the SDK is instantiated with unit LEZ and
Zcash port types, so a chain call is impossible even if a later adapter changes.
It loads neither the sidecar capability nor Zebra cookie, signing key,
agreement file, or preimage. The separate `activate` and `drive` paths load
their scoped effect material and completed both real local v0.2/Zebra directions again in the canonical
forward and reverse certification runs; `status` keeps this
no-chain design.

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
| ZEC private public-compatible local devnets and private recording/evidence | Independent happy/refund/reorg/concurrency actors across full local LEZ v0.2 and Zebra Regtest | M2 |
| ZEC public testnet/deployment and public recordings | Deferred proposal evidence plus final production rehearsal/remediation | Production readiness / M7 under ADR 0023 |
| BTC private public-compatible local devnets and evidence | Independent actors complete both directions across Bitcoin Core 31.1 Regtest and local LEZ v0.2, with exact key-path witness and scalar extraction | M3 |
| BTC public Testnet4 route and final review | Self-host/public node, wallet, funds and flakiness guide in M3; live public execution/remediation may remain private/deferred | M3 documentation, production readiness / M7 live evidence |
| XMR stagenet and final review | Independent actors, COMIT/DLEQ evidence, and remediation packet | M4, M7 |

No milestone is complete merely because an internal API test passes. Its tag
must point to the commit whose role-real evidence crosses every applicable
boundary above.
