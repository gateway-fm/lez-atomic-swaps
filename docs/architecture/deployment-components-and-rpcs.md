# Deployment components, RPCs, and local nodes

Status: Living executable inventory — 2026-07-13

This document is the concrete deployment companion to the
[system architecture](system-architecture.md). It distinguishes processes that
actually run today from target components that do not yet have an implementation,
port, credential scheme, or selected provider. A dashed component or edge is
planned. No port is invented for an unimplemented integration.

## Current executable local topology

```mermaid
flowchart TB
    Operator["Maker operator"]

    subgraph MakerHost["Maker host"]
        CLI["lez-maker CLI"]
        Daemon["lez-maker-daemon"]
        Store[("SQLite schema v9")]
        RuntimeTest["maker runtime restart fixture"]
        SdkJournal["SDK exact-tracker canonical / depth / same-tip replacement / removal journal"]
        SdkMaker["SDK fresh-gated maker-lock fixture"]
    end

    subgraph DeterministicCorridor["Deterministic SDK claim corridor; no node or RPC"]
        MakerActor["Role-fixed maker SDK actor"]
        TakerActor["Role-fixed taker SDK actor"]
        MakerClaimState[("Maker schema-v9 state")]
        TakerClaimState[("Taker schema-v9 state")]
        ClaimDouble["Deterministic LEZ/Zcash claim-port doubles"]
        Completed["Both actors Completed at revision 4"]
    end

    subgraph ZebraProject["Isolated Docker Compose project per RUN_ID"]
        ZebraPrimary["Primary Zebra 5.2.0 Regtest"]
        ZebraFork["Fork Zebra 5.2.0 Regtest"]
    end

    ZebraTest["Zebra actor acceptance fixture"]

    subgraph LezProcess["Pinned LEZ v0.1.2 test process"]
        LezTest["LEZ actor acceptance fixture"]
        LezNode["Standalone sequencer"]
    end

    Operator --> CLI
    CLI -->|"Bearer HTTP JSON-RPC; create, status, alert list, alert acknowledge"| Daemon
    Daemon -->|"rusqlite; caller-selected local file; Mutex-serialized"| Store
    RuntimeTest -->|"Direct maker runtime API"| Store
    RuntimeTest --> SdkJournal
    SdkJournal -->|"Immediate transaction and full-history replay"| Store
    SdkMaker -->|"Both directions; intent and confirmed transition"| Store
    MakerActor --> MakerClaimState
    TakerActor --> TakerClaimState
    MakerActor -->|"LEZ reveal or Zcash follow-up"| ClaimDouble
    TakerActor -->|"Observe reveal and continue from protected preimage"| ClaimDouble
    ClaimDouble --> Completed
    Completed --> MakerClaimState
    Completed --> TakerClaimState
    RuntimeTest -->|"Stable JSON-RPC query and block relay"| ZebraPrimary
    RuntimeTest -->|"Independent fork mining JSON-RPC"| ZebraFork
    ZebraTest -->|"Unauthenticated JSON-RPC on ephemeral host-loopback port"| ZebraPrimary
    ZebraTest -->|"Unauthenticated JSON-RPC on different ephemeral host-loopback port"| ZebraFork
    ZebraFork -->|"submitblock relay performed by fixture"| ZebraPrimary
    LezTest -->|"Unauthenticated HTTP JSON-RPC through 127.0.0.1 client URL"| LezNode
```

The maker daemon hard-refuses a non-loopback bind and defaults to
`127.0.0.1:0`. Its ready file contains only the selected URL; the Bearer token
comes from `LEZ_MAKER_RPC_TOKEN` and is never written there. The CLI default is
`http://127.0.0.1:9944`, so an ephemeral daemon must be called with the ready URL.
Registered methods are `swap_create`, `swap_status`, `swap_alerts`, and
`swap_alert_acknowledge`. Status includes pending count/highest severity; list
supports a cursor and acknowledged-history flag. Acknowledgment never changes
protocol phase. There is no daemon-integrated chain watcher, chain-key owner,
health method, or production ZEC ingestion RPC yet.

Each Zebra container listens on `0.0.0.0:18232` inside its isolated project
network. Compose publishes it as a different ephemeral `127.0.0.1` host port.
Cookie authentication is disabled for this Regtest-only fixture. The nodes have
separate tmpfs state, no initial peers, and no fixed host ports; the fixture
relays blocks explicitly. Both containers are non-root, read-only,
capability-free, resource-capped, and removed only through their exact Compose
project name. The runner creates an absolute, run-scoped SQLite path, refuses a
pre-existing manifest, database, WAL, or SHM before Compose starts, and records
the selected database and both ephemeral RPC URLs in `run.env`. The maker
runtime fixture commits real canonical funding, closes/reopens SQLite, suppresses
an unchanged fresh RPC requery, applies an affirmative deeper-fork removal,
closes/reopens again, and proves exact retry without a third journal row.

The pinned LEZ standalone server is short-lived, uses port zero and temporary
state, and the test client connects through `127.0.0.1`. However, the exact
upstream v0.1.2 server binds `0.0.0.0` on that ephemeral port. It is collision
isolated but not loopback/network-namespace isolated. This must remain visible
until upstream accepts a bind address or the lane is placed in an isolated
network namespace/container.

The deterministic claim corridor is deliberately separate from both local-node
lanes. It uses two role-fixed SDK actors, two different temporary schema-v9
SQLite databases, and an externally supplied test claim key per role. The key is
not stored in SQLite. In both signed directions it persists exact protected
claim submissions, observes the LEZ reveal, protects the extracted preimage,
submits the Zcash follow-up, and reopens both actors at revision 4 and
`Completed`. Its LEZ and Zcash ports are deterministic test doubles: it starts no
service, opens no RPC connection, and proves neither `LezClaimPort` nor
`ZcashClaimPort` against a node.

## M2 SDK/reference-demo target topology

This is the accepted M2 demo boundary. It is independent of the M5 production
daemon/CLI/Delivery/Chat delivery below.

```mermaid
flowchart LR
    Maker["Maker reference actor process"]
    Taker["Taker reference actor process"]
    MakerState[("Maker-only recovery state")]
    TakerState[("Taker-only recovery state")]
    Mailbox["Test-only pre-lock agreement mailbox"]
    MakerValidator["Maker concrete agreement validator"]
    TakerValidator["Taker concrete agreement validator"]
    MakerObserver["Maker-only taker-lock observer"]
    MakerEffect["Fresh-gated durable maker effect"]
    ZcashClaimPort["Production ZcashClaimPort pending"]
    LezClaimPort["Production LezClaimPort pending"]
    Zebra["Selected Zebra route"]
    Lez["Selected LEZ route"]

    Maker --> MakerValidator
    Taker --> TakerValidator
    MakerValidator -.->|"Persist accepted time, role, record, commitment, revision"| MakerState
    TakerValidator -.->|"Persist accepted time, role, record, commitment, revision"| TakerState
    Maker -.->|"Publish and countersign before first lock"| Mailbox
    Taker -.->|"Discover and countersign before first lock"| Mailbox
    Mailbox -->|"Bounded dual-signed Borsh schema-2 record"| MakerValidator
    Mailbox -->|"Bounded dual-signed Borsh schema-2 record"| TakerValidator
    Maker -.->|"Prepared exact follow-up claim"| ZcashClaimPort
    Taker -.->|"Prepared exact follow-up claim"| ZcashClaimPort
    Maker -.->|"Prepared exact revealing claim"| LezClaimPort
    Taker -.->|"Prepared exact revealing claim"| LezClaimPort
    ZcashClaimPort -.->|"Observe, broadcast, and return canonical spend evidence"| Zebra
    LezClaimPort -.->|"Official-wire submit, observe, and extract preimage"| Lez
    Zebra -->|"Forward canonical evidence adapter pending"| MakerObserver
    Lez -->|"Reverse stable snapshot<br/>channel, tx, block, metadata, custody"| MakerObserver
    MakerObserver -->|"Atomic non-authorizing maker projection"| MakerState
    MakerState -->|"Fresh reorg-safe eligibility"| MakerEffect
    MakerEffect -.->|"Direction-fixed action"| Zebra
    MakerEffect -.->|"Direction-fixed action"| Lez
    MakerEffect -->|"Intent, transition, and revision"| MakerState
    Mailbox -.->|"Destroyed after immutable terms persist"| Maker
    Mailbox -.->|"Destroyed after immutable terms persist"| Taker

    classDef planned stroke-dasharray: 5 5,fill:#fff7e6,stroke:#9a6700;
    class Maker,Taker,MakerState,TakerState,Mailbox,ZcashClaimPort,LezClaimPort,Zebra,Lez planned;
```

Each process must have a distinct PID, owner-only state/key paths, fixed local
role, and separately persisted agreement. The local mailbox is explicitly a test
adapter, not Logos Delivery or Chat. Once terms persist and the first lock is
submitted, it is destroyed; both actors must complete or refund using only their
own state and selected chain nodes.

## M5 production application target topology

```mermaid
flowchart LR
    Maker["Maker operator"]
    Taker["Taker user"]

    subgraph MakerBoundary["Maker-owned boundary"]
        MakerCLI["Maker CLI"]
        Core["Optional Logos Core lifecycle adapter"]
        MakerDaemon["Maker daemon plus ZEC watcher"]
        MakerStore[("SQLite aggregate, journal, binding, alert outbox")]
        MakerZebra["Selected maker Zebra route"]
        MakerLez["Selected maker LEZ route"]
    end

    subgraph TakerBoundary["Taker-owned boundary"]
        TakerCLI["Taker CLI or SDK"]
        TakerState[("Taker recovery state")]
        TakerZebra["Selected taker Zebra route"]
        TakerLez["Selected taker LEZ route"]
    end

    Delivery["Offer discovery"]
    Chat["Signed negotiation channel"]

    Maker --> MakerCLI
    MakerCLI -.->|"Owner-local authenticated control RPC"| MakerDaemon
    Core -.->|"start, endpoint, health, stop"| MakerDaemon
    MakerDaemon --> MakerStore
    MakerDaemon -.->|"Zebra JSON-RPC; broadcast and stable canonical observations"| MakerZebra
    MakerDaemon -.->|"LEZ JSON-RPC; escrow submit and observation"| MakerLez
    Taker --> TakerCLI
    TakerCLI --> TakerState
    TakerCLI -.->|"Zebra JSON-RPC"| TakerZebra
    TakerCLI -.->|"LEZ JSON-RPC"| TakerLez
    MakerDaemon -.->|"Authenticated expiring offers only"| Delivery
    TakerCLI -.->|"Offer queries only"| Delivery
    MakerDaemon -.->|"Both-role signed terms before first lock"| Chat
    TakerCLI -.->|"Both-role signed terms before first lock"| Chat

    classDef planned stroke-dasharray: 5 5,fill:#fff7e6,stroke:#9a6700;
    class MakerCLI,Core,MakerDaemon,MakerZebra,MakerLez,TakerCLI,TakerState,TakerZebra,TakerLez,Delivery,Chat planned;
```

Delivery and Chat are negotiation transports, never sources of chain truth or
secrets. After the first lock, each actor must recover using only its own durable
state and selected chain nodes. Logos Core is optional lifecycle/presentation;
it never opens SQLite or becomes protocol authority.

## RPC and local-resource inventory

| Component | Status | Transport and bind | Authentication / authority | Methods exercised or required | Lifecycle and isolation |
|---|---|---|---|---|---|
| `lez-maker-daemon` | Running prototype | HTTP JSON-RPC; default `127.0.0.1:0`; non-loopback rejected | Bearer token from hidden environment; minimum 24 bytes; header checked before JSON parsing | Actual: `swap_create`, `swap_status`, `swap_alerts`, `swap_alert_acknowledge` | Operator/test-owned process; caller-selected SQLite path; Ctrl-C shutdown |
| `lez-maker` | Running prototype | HTTP client; default `127.0.0.1:9944`; explicit ready URL for ephemeral daemon | Authorization header marked sensitive | Actual CLI: `create-swap`, `status`, `alerts`, `acknowledge-alert` | Independent operator process |
| SQLite | Running | Local file; no RPC or port | Daemon/runtime process filesystem authority; SDK adapter fixes one local role per handle; claim key material is supplied externally and never stored | Aggregate, revision, ZEC journal, immutable binding, alerts, separate taker/maker lock and claim intents, protected claim material, owner/observer claim transitions, and canonical observation transitions | WAL, `FULL` synchronous, foreign keys, immediate transactions; schema-v9 replay retains the schema-v8 lock journal, rejects inconsistent history, and closes/reopens both directions at revision 4 and `Completed`; the v8→v9 migration replaces legacy plaintext claim evidence and scrubs SQLite/WAL remnants; 30 committed store tests pass; one process mutex remains |
| Deterministic LEZ/ZEC SDK and claim corridor | Running library/test boundary | No socket, RPC, node, Docker, faucet, or public endpoint; bounded Borsh schema-2 bytes enter from an untrusted negotiation adapter | Fixed maker/taker roles and the signed direction select observations and effects; separate role databases and external claim keys prevent shared recovery authority | Exact agreement validation, protected activation, both lock directions, LEZ reveal, observer preimage extraction, Zcash follow-up, and claim-capable replay at `Completed` in both directions | 16 KiB agreement and 2,000,000-byte submission caps; 120 committed SDK checks including one doctest plus 30 committed store tests pass. Chain evidence comes from deterministic port doubles, so this row is not an actual-node claim |
| Production LEZ/Zcash lifecycle ports | Planned | Required transports are official-wire LEZ JSON-RPC and Zebra JSON-RPC; no production `LezClaimPort` or `ZcashClaimPort` implementation is selected in the SDK today | Each adapter must independently derive deployment, transaction, signer, account, outpoint, and canonical evidence from the accepted agreement and selected node; fixture-supplied identities are not authority | Required: observe before rebroadcast, submit byte-identical protected intent, decode canonical LEZ reveal and extract the bound preimage, submit/observe the Zcash follow-up, handle unknown outcomes and removal/replacement | The real Zebra and local LEZ lifecycles pass separately below; a composed LEZ+Zebra two-actor claim flow, refunds, actor processes, and public-testnet evidence remain M2 work |
| Primary Zebra | Running in ignored E2E | Container `0.0.0.0:18232`; ephemeral host `127.0.0.1` mapping | Regtest fixture has no cookie auth; signed transactions and consensus remain authoritative | `getblockcount`, `generate`, `getblockhash`, `getblock`, `getblockheader`, `submitblock`, `getaddressutxos`, `getrawtransaction`, `sendrawtransaction`, `getblockchaininfo` | Unique Compose project and tmpfs state per `RUN_ID` |
| Fork Zebra | Running in ignored E2E | Same container port; distinct ephemeral host-loopback mapping | Same Regtest-only policy | Same RPC set; produces independent higher-work branch | Separate tmpfs state; no initial peer; fixture-controlled block relay |
| LEZ standalone v0.1.2 | Running in ignored E2E | Upstream server `0.0.0.0:0`; client uses `127.0.0.1:<assigned>` | No transport credential; actor signatures authorize transactions | `checkHealth`, `sendTransaction`, `getLastBlockId`, `getTransaction`, `getAccountsNonces`, `getAccount`, `getBlock` | In-process handle, tempfile state, deterministic genesis actors; not public v0.2 |
| Logos Core adapter | Planned | No transport/port selected beyond the daemon control endpoint | Protected OS credential handle | `start`, `endpoint`, `health`, `stop` | Optional supervisor of the same daemon binary |
| Delivery / Chat | Planned | No protocol, endpoint, or port selected | Authenticated offers and both-role signed transcript | `OfferDiscovery`; `NegotiationChannel` | Untrusted/removable after first lock |
| Production Zebra watcher route | Self-hosted public-Testnet route selected; adapter/evidence planned | Zebra 6.0.0 JSON-RPC on operator-owned loopback with cookie auth; no official public Zebra JSON-RPC route found | Operator owns cookie and node; public peers provide consensus data; never substitute lightwalletd gRPC | Required: sync/branch preflight, stable-tip observation, `gettxout`, raw transaction lookup, broadcast, reorg reconciliation | Initial sync/disk/P2P/epoch flakiness; project transparent signer and live actor suite remain |
| Official LEZ testnet v0.2 node | Live node; narrow adapter/deployment planned | HTTPS JSON-RPC `https://testnet.lez.logos.co` | Public reads; actor wallet/signature authorizes transactions; rate limits unspecified | Verified `checkHealth`, `getLastBlockId`, `getProgramIds`; required deployment evidence adds `getChannelId`, returned `sendTransaction` hash, `getTransaction`, and exact `getBlock` inclusion | Official LEZ v0.2.0 commit `a58fbce...`; guest/client must use `/LEE/` PDA domain; announced resets invalidate evidence when channel changes |
| LEZ v0.2 deployment/query client | Planned, security-constrained | HTTPS JSON-RPC to the official node; no local listener | Official LEZ transaction/RPC types; actor signing material remains process-local | Deploy checked guest, retain transaction hash, query exact transaction and containing block | Thin `jsonrpsee` graph must exclude Logos node auth, libp2p, Hickory 0.25, and pending LGPL exceptions; full released standalone graph is prohibited for public runtime use |
| Bitcoin Core | M3 planned | No port/image/provider selected | Actor-owned node and wallet credentials | Typed `BitcoinChain` port | Do not infer conventional ports before selection |
| `monerod` plus wallet RPC | M4 planned | No ports/images/providers selected | Actor-owned daemon/wallet credentials | Typed `MoneroChain` port | Wallet/key state remains actor-owned |

## External resources and flakiness

The deterministic SDK/schema-v9 corridor uses only local temporary files and
process input. It cannot fail because a public RPC, faucet, chain peer, Docker
registry, or testnet is unavailable. The ignored Zebra and LEZ suites cross real
local consensus/state-transition boundaries, but they also remain independent
suites rather than one composed swap. Regtest coinbase outputs and LEZ genesis
balances provide deterministic local funds.

Cold setup can still depend on crates.io, GitHub, container registries, Risc0
tool distribution, and the checksummed Logos circuits release. CPU, memory,
disk, registry availability, and an uncached dependency graph can therefore
delay or fail a local-node run without changing its chain assertions. Warm,
verified caches reduce availability risk but do not justify bypassing a digest,
lockfile, or vulnerability failure.

Future public evidence adds different failure modes. The official LEZ endpoint
`https://testnet.lez.logos.co` can be rate-limited, unavailable, reset, or move
to another channel; every result must bind the observed channel, block,
transaction, ProgramId, ELF, ImageID, and exact commits. The selected Zcash
route is a self-hosted Zebra 6.0.0 public-Testnet node because no official public
Zebra JSON-RPC service was found; initial sync, disk use, peers, epoch changes,
and organic reorgs can delay it. Community faucets or Discord funding have no
SLA and may be depleted, so CI must not silently treat their outage as success.
No current executable user flow calls those public endpoints or funding routes.

## Local test concurrency

```mermaid
flowchart LR
    Start["Choose unique RUN_ID"] --> Check{"Heavy suite already active?"}
    Check -->|"yes"| Wait["Wait or use isolated checkout and resources"]
    Check -->|"no"| Choose{"Select one heavy lane"}
    Choose --> Zebra["Two-node Zebra Compose project"]
    Choose --> Lez["LEZ standalone and Risc0 lane"]
    Zebra --> ScopedCleanup["Clean exact Compose project only"]
    Lez --> ScopedCleanup
```

Never run the Zebra and LEZ heavy lanes concurrently on the same host. Never use
global Docker prune/stop commands. Every Zebra run owns a unique Compose project,
ephemeral host ports, run manifest, and absolute maker database. Reusing a run
manifest or database is rejected before Compose starts. LEZ runs require unique
tool, target, standalone, and evidence directories when another checkout might be active.
