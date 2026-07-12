# Deployment components, RPCs, and local nodes

Status: Living executable inventory — 2026-07-12

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
        Store[("SQLite schema v4")]
        RuntimeTest["maker runtime restart fixture"]
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
    Zebra["Selected Zebra route"]
    Lez["Selected LEZ route"]

    Maker --> MakerValidator
    Taker --> TakerValidator
    MakerValidator -.->|"Persist accepted time, role, record, commitment, revision"| MakerState
    TakerValidator -.->|"Persist accepted time, role, record, commitment, revision"| TakerState
    Maker -.->|"Publish and countersign before first lock"| Mailbox
    Taker -.->|"Discover and countersign before first lock"| Mailbox
    Mailbox -->|"Bounded dual-signed Borsh v1 record"| MakerValidator
    Mailbox -->|"Bounded dual-signed Borsh v1 record"| TakerValidator
    Maker -.->|"Typed funding, observation, claim, refund"| Zebra
    Taker -.->|"Typed funding, observation, claim, refund"| Zebra
    Maker -.->|"Generated escrow client actions"| Lez
    Taker -.->|"Generated escrow client actions"| Lez
    Mailbox -.->|"Destroyed after immutable terms persist"| Maker
    Mailbox -.->|"Destroyed after immutable terms persist"| Taker

    classDef planned stroke-dasharray: 5 5,fill:#fff7e6,stroke:#9a6700;
    class Maker,Taker,MakerState,TakerState,Mailbox,Zebra,Lez planned;
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
| SQLite | Running | Local file; no RPC or port | Daemon/runtime process filesystem authority | Aggregate, revision, ZEC journal, immutable binding, operator-alert list/ack APIs | WAL, `FULL` synchronous, schema v4; actual two-Zebra test closes/reopens twice; one process mutex today |
| Concrete LEZ/ZEC agreement and lifecycle boundary | Running library boundary | No socket or RPC; bounded Borsh v1 bytes enter from an untrusted negotiation adapter; typed first-lock ports have no selected production endpoint | Maker and taker transparent keys provide dual low-S signatures; each SDK fixes its local role; chain adapters receive the agreement and exact durable bytes and remain authoritative for chain-derived facts | Exact decode, cross-binding, persistence-before-activation, adversarial resume, durable exact first-lock intent, observe-before-rebroadcast, ordered LEZ initialize/fund | 16 KiB agreement and 2,000,000-byte per-submission caps; in-memory contract passes, while production SQLite, evidence projection, and RPC adapters remain pending |
| Primary Zebra | Running in ignored E2E | Container `0.0.0.0:18232`; ephemeral host `127.0.0.1` mapping | Regtest fixture has no cookie auth; signed transactions and consensus remain authoritative | `getblockcount`, `generate`, `getblockhash`, `getblock`, `getblockheader`, `submitblock`, `getaddressutxos`, `getrawtransaction`, `sendrawtransaction`, `getblockchaininfo` | Unique Compose project and tmpfs state per `RUN_ID` |
| Fork Zebra | Running in ignored E2E | Same container port; distinct ephemeral host-loopback mapping | Same Regtest-only policy | Same RPC set; produces independent higher-work branch | Separate tmpfs state; no initial peer; fixture-controlled block relay |
| LEZ standalone v0.1.2 | Running in ignored E2E | Upstream server `0.0.0.0:0`; client uses `127.0.0.1:<assigned>` | No transport credential; actor signatures authorize transactions | `checkHealth`, `sendTransaction`, `getLastBlockId`, `getTransaction`, `getAccountsNonces`, `getAccount`, `getBlock` | In-process handle, tempfile state, deterministic genesis actors; not public v0.2 |
| Logos Core adapter | Planned | No transport/port selected beyond the daemon control endpoint | Protected OS credential handle | `start`, `endpoint`, `health`, `stop` | Optional supervisor of the same daemon binary |
| Delivery / Chat | Planned | No protocol, endpoint, or port selected | Authenticated offers and both-role signed transcript | `OfferDiscovery`; `NegotiationChannel` | Untrusted/removable after first lock |
| Production Zebra watcher route | Planned | Self-hosted and public-testnet routes unselected | Provider credentials/rate limits unselected | Stable-tip observation, broadcast, reorg reconciliation | Actor-selected node; fallback and health policy required |
| Official LEZ testnet v0.2 node | Live node; adapter/deployment planned | HTTPS JSON-RPC `https://testnet.lez.logos.co` | Public reads; actor wallet/signature authorizes transactions; rate limits unspecified | Verified `checkHealth`, `getLastBlockId`, `getProgramIds`; escrow submit/observation still pending | Official LEZ v0.2.0; must use v0.2-compatible guest/client and `/LEE/` PDA domain |
| Bitcoin Core | M3 planned | No port/image/provider selected | Actor-owned node and wallet credentials | Typed `BitcoinChain` port | Do not infer conventional ports before selection |
| `monerod` plus wallet RPC | M4 planned | No ports/images/providers selected | Actor-owned daemon/wallet credentials | Typed `MoneroChain` port | Wallet/key state remains actor-owned |

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
