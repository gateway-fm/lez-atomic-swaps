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
        Store[("SQLite schema v10")]
        RuntimeTest["maker runtime restart fixture"]
        SdkJournal["SDK exact-tracker canonical / depth / same-tip replacement / removal journal"]
        SdkMaker["SDK fresh-gated maker-lock fixture"]
    end

    subgraph DeterministicCorridor["Deterministic SDK claim corridor; no node or RPC"]
        MakerActor["Role-fixed maker SDK actor"]
        TakerActor["Role-fixed taker SDK actor"]
        MakerClaimState[("Maker schema-v10 state")]
        TakerClaimState[("Taker schema-v10 state")]
        ClaimDouble["Deterministic LEZ/Zcash claim-port doubles"]
        Completed["Both actors Completed at revision 4"]
    end

    subgraph ZebraProject["Isolated Docker Compose project per RUN_ID"]
        ZebraPrimary["Primary Zebra 5.2.0 Regtest"]
        ZebraFork["Fork Zebra 5.2.0 Regtest"]
    end

    ZebraTest["Zebra actor acceptance fixture"]

    subgraph LezProcess["Pinned LEZ v0.1.2 local-node lane"]
        LezTest["LEZ actor and process acceptance fixtures"]
        LezExternal["lez-standalone-node external process"]
        LezNode["Standalone sequencer<br/>dynamic upstream port"]
        LezHome["Fresh mode-0700 home"]
        LezReady[("Private mode-0600 schema-v2 readiness<br/>deployment tx/block + program/built-in + actor keys")]
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
    LezTest -->|"Spawn, tamper/reuse rejection, shutdown"| LezExternal
    LezExternal -->|"Start exact upstream service"| LezNode
    LezExternal --> LezHome
    LezExternal -->|"Official health, genesis, tx/block, static built-in, account RPC"| LezNode
    LezTest -->|"Unauthenticated HTTP JSON-RPC through 127.0.0.1 client URL"| LezNode
    LezExternal -->|"Atomic no-clobber publish after verification"| LezReady
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

The reusable external wrapper verifies the tracked manifest bytes, ELF
SHA-256, and Risc0 ImageID before creating state. It refuses a pre-existing
home or readiness path, creates a mode-0700 home, waits for mandatory canonical
progress, deploys the checked guest, and re-reads genesis, the deployed
transaction and containing block, ProgramId derived from the transaction ELF,
the advertised authenticated-transfer built-in, and two key-derived funded
accounts owned by that built-in through official RPC. Upstream `getProgramIds`
is a static built-in map, not a deployed-program registry. Only then does the
process atomically create the mode-0600 schema-v2 readiness file; that file
contains deterministic actor private keys and is never a public ready file.
Tamper/reuse failures leave no readiness and preserve the existing home.

The deterministic claim corridor is deliberately separate from both local-node
lanes. It uses two role-fixed SDK actors, two different temporary schema-v10
SQLite databases, and an externally supplied test claim key per role. The key is
not stored in SQLite. In both signed directions it persists exact protected
claim submissions, observes the LEZ reveal, protects the extracted preimage,
submits the Zcash follow-up, and reopens both actors at revision 4 and
`Completed`. Its LEZ and Zcash ports are deterministic test doubles: it starts no
service, opens no RPC connection, and proves neither `LezClaimPort` nor
`ZcashClaimPort` against a node.

## M2 SDK/reference-demo target topology

This is the accepted M2 demo boundary. It is independent of the M5 production
daemon/CLI/Delivery/Chat delivery below. ADR 0022 records why the LEZ adapter is
a process boundary rather than another dependency in the reference actor.

The Zcash workspace graph pins `crypto-common = 0.2.0-rc.1`. The official LEZ
v0.1.2 graph reaches `chacha20 0.10` and `cipher 0.5.1`, which require stable
`crypto-common ^0.2`; Cargo cannot resolve those constraints in one graph. The
integration must not weaken or patch either cryptographic pin, and must not copy
official LEZ wire or RPC types into the Zcash workspace. A separately built,
exactly locked LEZ sidecar owns those official types. Its typed local bridge is
a bounded serde-only adapter protocol carrying primitive requests and facts; it
is not the official LEZ JSON-RPC protocol and cannot assert canonicality.

```mermaid
flowchart LR
    Mailbox["Test-only pre-lock agreement mailbox"]
    Zebra["Selected Zebra JSON-RPC node"]
    LezNode["Selected LEZ sequencer JSON-RPC node"]
    LezExternal["Reusable checked external v0.1.2 node process"]
    LezReady[("Private schema-v2 node readiness<br/>deployment tx/block + program/built-in + actor keys")]
    LezRunner["Run-scoped reference corridor runner"]

    subgraph MakerProcess["Role-fixed maker reference actor process"]
        MakerSdk["LEZ/ZEC swap SDK and agreement validators"]
        MakerState[("Maker-only schema-v10 state")]
        MakerBridge["Typed local LEZ bridge client"]
        MakerZebra["In-process typed Zebra adapter"]
    end

    subgraph MakerSidecarProcess["Run-scoped maker LEZ sidecar process"]
        MakerCapability["Capability and RUN_ID check"]
        MakerOfficial["Official LEZ types, signer, and nonce lease"]
    end

    subgraph TakerProcess["Role-fixed taker reference actor process"]
        TakerSdk["LEZ/ZEC swap SDK and agreement validators"]
        TakerState[("Taker-only schema-v10 state")]
        TakerBridge["Typed local LEZ bridge client"]
        TakerZebra["In-process typed Zebra adapter"]
    end

    subgraph TakerSidecarProcess["Run-scoped taker LEZ sidecar process"]
        TakerCapability["Capability and RUN_ID check"]
        TakerOfficial["Official LEZ types, signer, and nonce lease"]
    end

    Mailbox -.->|"Bounded dual-signed terms before first lock"| MakerSdk
    Mailbox -.->|"Bounded dual-signed terms before first lock"| TakerSdk
    MakerSdk --> MakerState
    TakerSdk --> TakerState
    MakerSdk -.->|"Primitive request"| MakerBridge
    TakerSdk -.->|"Primitive request"| TakerBridge
    MakerBridge -.->|"Bounded serde-only local protocol"| MakerCapability
    TakerBridge -.->|"Bounded serde-only local protocol"| TakerCapability
    MakerCapability --> MakerOfficial
    TakerCapability --> TakerOfficial
    MakerOfficial -.->|"Official LEZ transaction and JSON-RPC types"| LezNode
    TakerOfficial -.->|"Official LEZ transaction and JSON-RPC types"| LezNode
    LezExternal -->|"Start exact upstream service on fresh state"| LezNode
    LezNode -->|"Official verification RPC"| LezExternal
    LezExternal -->|"No-clobber mode-0600 publish"| LezReady
    LezReady -.->|"Future private handoff"| LezRunner
    LezRunner -.->|"Maker-only endpoint and key files"| MakerCapability
    LezRunner -.->|"Taker-only endpoint and key files"| TakerCapability
    MakerSdk -.->|"Typed requests and validated snapshots"| MakerZebra
    TakerSdk -.->|"Typed requests and validated snapshots"| TakerZebra
    MakerZebra -.->|"Direct bounded Zebra JSON-RPC"| Zebra
    TakerZebra -.->|"Direct bounded Zebra JSON-RPC"| Zebra
    MakerCapability -.->|"Primitive facts only"| MakerBridge
    TakerCapability -.->|"Primitive facts only"| TakerBridge
    Mailbox -.->|"Destroyed after immutable terms persist"| MakerSdk
    Mailbox -.->|"Destroyed after immutable terms persist"| TakerSdk

    classDef planned stroke-dasharray: 5 5,fill:#fff7e6,stroke:#9a6700;
    class Mailbox,Zebra,LezRunner,MakerSdk,MakerState,MakerBridge,MakerZebra,MakerCapability,MakerOfficial,TakerSdk,TakerState,TakerBridge,TakerZebra,TakerCapability,TakerOfficial planned;
```

Each actor and each actor-owned sidecar has a distinct PID. Actor state, claim
keys, LEZ signing keys, sidecar capabilities, and nonce leases are owner-local;
the maker and taker persist the agreement separately. The reference runner owns
the sidecar listener, selects an ephemeral loopback port and high-entropy
capability, binds every call to its `RUN_ID`, and cleans only its own processes
and resources. The actor never sends official LEZ RPC through the local bridge:
only the separately locked sidecar constructs, signs, decodes, and calls the
official LEZ protocol. The in-process Zebra adapter calls Zebra directly and
delegates agreement/consensus validation to the SDK.

The local mailbox is explicitly a test adapter, not Logos Delivery or Chat.
Once terms persist and the first lock is submitted, it is destroyed; both actors
must complete or refund using only their own state and selected chain nodes.

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
        MakerLezBridge["Context-owning LEZ SDK ports + adapter"]
        MakerLezSidecar["Official-wire LEZ sidecar"]
        MakerLez["Selected maker LEZ route"]
    end

    subgraph TakerBoundary["Taker-owned boundary"]
        TakerCLI["Taker CLI or SDK"]
        TakerState[("Taker recovery state")]
        TakerZebra["Selected taker Zebra route"]
        TakerLezBridge["Context-owning LEZ SDK ports + adapter"]
        TakerLezSidecar["Official-wire LEZ sidecar"]
        TakerLez["Selected taker LEZ route"]
    end

    Delivery["Offer discovery"]
    Chat["Signed negotiation channel"]

    Maker --> MakerCLI
    MakerCLI -.->|"Owner-local authenticated control RPC"| MakerDaemon
    Core -.->|"start, endpoint, health, stop"| MakerDaemon
    MakerDaemon --> MakerStore
    MakerDaemon -.->|"Zebra JSON-RPC; broadcast and stable canonical observations"| MakerZebra
    MakerDaemon -.->|"Bounded local adapter protocol"| MakerLezBridge
    MakerLezBridge -.-> MakerLezSidecar
    MakerLezSidecar -.->|"Official LEZ JSON-RPC"| MakerLez
    Taker --> TakerCLI
    TakerCLI --> TakerState
    TakerCLI -.->|"Zebra JSON-RPC"| TakerZebra
    TakerCLI -.->|"Bounded local adapter protocol"| TakerLezBridge
    TakerLezBridge -.-> TakerLezSidecar
    TakerLezSidecar -.->|"Official LEZ JSON-RPC"| TakerLez
    MakerDaemon -.->|"Authenticated expiring offers only"| Delivery
    TakerCLI -.->|"Offer queries only"| Delivery
    MakerDaemon -.->|"Both-role signed terms before first lock"| Chat
    TakerCLI -.->|"Both-role signed terms before first lock"| Chat

    classDef planned stroke-dasharray: 5 5,fill:#fff7e6,stroke:#9a6700;
    class MakerCLI,Core,MakerDaemon,MakerZebra,MakerLezBridge,MakerLezSidecar,MakerLez,TakerCLI,TakerState,TakerZebra,TakerLezBridge,TakerLezSidecar,TakerLez,Delivery,Chat planned;
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
| SQLite | Running | Local file; no RPC or port | Daemon/runtime process filesystem authority; SDK adapter fixes one local role per handle; claim key material is supplied externally and never stored | Aggregate, revision, ZEC journal, immutable binding, alerts, separate lock/claim/refund owner intents, protected claim material, owner/observer claim/refund transitions, and canonical observation transitions | WAL, `FULL` synchronous, foreign keys, immediate transactions; schema-v10 replay retains prior lock/claim journals, rejects inconsistent history, and closes/reopens both directions at revision 4 and `Completed` or `Refunded`. Owner refund commit copies the exact intent, inserts the transition, advances revision once, and deletes pending intent in one immediate transaction; observer rows retain no signing intent. The v8→v9 migration still replaces legacy plaintext claim evidence and scrubs SQLite/WAL remnants; 39 store tests pass; one process mutex remains |
| Deterministic LEZ/ZEC SDK lifecycle | Running library/test boundary | No socket, RPC, node, Docker, faucet, or public endpoint; bounded Borsh schema-2 bytes enter from an untrusted negotiation adapter | Fixed maker/taker roles and the signed direction select observations/effects; separate role databases and external claim keys prevent shared claim-recovery authority; refund observers cannot sign | Exact agreement validation, protected activation, both lock directions, LEZ reveal, observer preimage extraction, Zcash follow-up, and fixed LEZ-then-Zcash refund driving with observe-before-rebroadcast and versioned durable records. After signed-wire acceptance, activation and resume require no discovery or negotiation capability. `resume_all_capable` replays lock, claim, and refund records without a chain call for truthful terminal status | 16 KiB agreement and 2,000,000-byte submission caps; 132 SDK checks plus 39 store tests pass, with one actual-Zebra SDK case intentionally ignored outside its isolated runner. Claims and refunds replay through SQLite with forced-rollback, exact-conflict, corruption, future-schema, and terminal full-resume checks. Chain evidence comes from deterministic port doubles, so this row is not an actual-node claim |
| SDK-facing LEZ bridge client and adapter | Main-workspace eight-method client plus signed-agreement native first-lock, escrow observation, revealing-claim, and native-refund validation adapters implemented. Crash-safe context-owning SDK ports implement first-lock, taker/maker observation, claim, and refund boundaries; actor wiring remains under ADR 0022 | Literal-IP ephemeral owner-loopback HTTP for the M2 runner; owner-restricted Unix socket preferred for production; direct client follows no redirects or proxy settings | Sensitive high-entropy capability plus exact `RUN_ID` and fixed sidecar role; full run/role/runtime echoes and one-use caller-owned request IDs are validated. The production factory rereads a bounded regular non-symlink mode-0600 capability file for every fresh client, detects replacement, and retains no secret bytes. The actor source creates 128-bit OS-random request IDs and asks the injected authority only for the four bounded-discovery window shapes. A role-local SQLite operation journal, shared by all SDK-port clones, retains caller-owned IDs/windows by run, swap, role, and operation; ambiguous attempts reopen the exact context through a fresh client. Reopened role-local recovery state is the only canonical LEZ claim-funding source and reverse direction additionally requires the durable Zcash second lock. The adapters recheck signed environment/channel/genesis/program/role/signer before preparation or observation and independently invoke SDK validators; official wire decoding remains inside the sidecar | All eight typed `lez_bridge.v1.*` client calls; exact native initialize/fund conversion; exact-owner or bounded-discovery escrow/claim/refund observation; explicit-ID/window state, prepare, observe, and one-attempt submit conversion. Method names, identity headers, and body cap are owned once by the protocol crate | Sixty-one adapter tests cover secret/path redaction, exact permissions, symlinks, bounds, rotation, random request grammar/freshness, exhaustive operation/window shape, shared clone journal identity, reopened both-direction canonical funding, missing opposite lock, agreement/role substitution, durable identity mutation, production-store initialize-before-fund ordering, ambiguous context reuse, exact fields, windows, deadline, depth, one-attempt failures, and unknown outcomes. Strict Clippy passes. Neither layer speaks or duplicates official LEZ JSON-RPC |
| Official-wire LEZ sidecar | Native/revealing-claim/native-refund planners and observations, node-RPC core, authenticated eight-method server, and executable role runner implemented; context-owning actor composition remains under ADR 0022 | Official LEZ RPC client and actor-facing server enforce literal loopback; each actor listener binds `127.0.0.1:0`; one connection and 5.5 MB bodies maximum. The process test starts maker and taker concurrently with distinct private files and state | Capability, exact `RUN_ID`, and fixed role are checked in middleware before JSON parsing. Planners bind runtime/role/signer/program; native preparation reserves one consecutive nonce pair, revealing claim binds the exact preimage plus request/cache funding identity, and permissionless refund uses no nonce or witness. The 0600 durable store validates identity, rejects symlinks, fsyncs file plus directory, and restores all prepared operation kinds | All eight local methods register. Describe, official native/revealing-claim/refund prepare, exact cached submit, and all three observation families are complete. Observation decodes official transaction/signature/Risc0-instruction/account types, validates block/genesis links and stable clock/tip brackets, rejects ambiguity, and reports only full-window absence. Generated upstream RPC covers nonce, submit, clock, tip/block/account, and bounded range scans | Thirty-three all-target tests prove server/planner/node/runner behavior, exact randomized/unsigned restoration, native/claim/refund cache coexistence, owner/discovery observations, terminal account/custody rules, ambiguity/digest/moving-tip rejection, role isolation, and the durable pre-submit marker. Strict format/Clippy/rustdoc/advisory/license/source gates pass; exact misses and old, bounded, or partially covered discovery misses never imply false absence |
| In-process typed Zebra adapter | Agreement-bound both-direction funding discovery plus first-lock owner, owner/counterparty follow-up-claim, refund, agreement-committed exact-outpoint funding planning, and one checked all-trait SDK composite implemented. The production role-keyed funding/claim/refund signer is GREEN; actor-process wiring remains | Direct bounded Zebra JSON-RPC from each reference actor; explicit nonzero-port `127.0.0.1`/`::1` HTTP only; no bridge or sidecar | Regtest may be unauthenticated loopback; self-hosted Testnet consumes bounded cookie contents into a sensitive Basic header omitted from `Debug`. The production signer fixes one role and public key, stores only zeroizing secret bytes, reconstructs and erases a short-lived secp256k1 key per canonical SDK-builder call, and can later be replaced behind the narrow traits by an HSM or OS keystore. The funding signer is a trusted consensus-authorization capability; the planner independently revalidates complete transaction policy and shape without reimplementing Zcash script verification. The composite constructor rejects role drift before cloning its shared RPC handle, configures one finite scan horizon on both existing delegates, and redacts both capabilities. Signed bytes, Zebra broadcast validation, and canonical node observations remain authoritative | Typed chain/block/raw/verbose/UTXO/broadcast DTOs, exact signed candidate-set planning from stable-tip confirmed `gettxout` results, full-anchor funding discovery through canonical blocks and mempool, exact owner claim/refund, observe-before-rebroadcast, CLTV maturity, and bounded counterparty spend discovery. Under-depth, moving-tip, short-horizon, unresolved, or older evidence is `Unstable`, never absent. Pending: durable maker-lock post-`BothLegsLocked` removal/replacement ingestion and actor wiring | Sixty-six crate checks use authentic signed agreements and SDK/SQLite contexts and fail closed on role/key, candidate count/commitment, network/branch/genesis, UTXO ownership/confirmation/tip, identity, canonicality, transparent-only shape, output/outpoint/destination/change/fee/expiry policy, ambiguity, mutation, returned hash, scan exhaustion, replacement, removal, and transport deviations. Three signer, seven planner, and four composite API/configuration contracts prove byte equality, both directions, foreign-role/key rejection, stable-tip planning, mutation rejection, shared checked construction, and secret redaction; exact bytes are sent at most once on every post-send error branch |
| Composed LEZ plus Zebra corridor runner | Planned | One unique `RUN_ID`, ephemeral loopback sidecar listeners, one isolated Zebra Compose project, and run-scoped state paths | Generates separate maker/taker capabilities, claim keys, signers, funding, and databases; records endpoints without secrets | Starts nodes and sidecars, drives both signed directions, restarts after every effect, then adds refund/reorg/concurrency cases | Single cleanup owner; shared heavy-suite lock; never nests existing runners or removes unrelated Docker/process resources |
| Primary Zebra | Running in ignored E2E | Container `0.0.0.0:18232`; ephemeral host `127.0.0.1` mapping | Regtest fixture has no cookie auth; signed transactions and consensus remain authoritative | `getblockcount`, `generate`, `getblockhash`, `getblock`, `getblockheader`, `submitblock`, `getaddressutxos`, `getrawtransaction`, `sendrawtransaction`, `getblockchaininfo` | Unique Compose project and tmpfs state per `RUN_ID` |
| Fork Zebra | Running in ignored E2E | Same container port; distinct ephemeral host-loopback mapping | Same Regtest-only policy | Same RPC set; produces independent higher-work branch | Separate tmpfs state; no initial peer; fixture-controlled block relay |
| LEZ standalone v0.1.2 | Running in ignored E2E | Upstream server `0.0.0.0:0`; client uses `127.0.0.1:<assigned>` | No transport credential; actor signatures authorize transactions | `checkHealth`, `sendTransaction`, `getLastBlockId`, `getTransaction`, `getAccountsNonces`, `getAccount`, `getBlock`, and `getProgramIds` for static built-ins only | In-process handle, temporary state, deterministic genesis actors; not public v0.2 |
| Reusable external LEZ standalone process | Exact schema-v2 process, rejection paths, native/two-definition lifecycle, strict Clippy, and recursive-cost runner previously GREEN; nonempty actor channel focused suite GREEN and exact full rerun pending | Own process; upstream `0.0.0.0:0` server; publishes only the allocated literal `http://127.0.0.1:<port>` client URL | No RPC transport credential; mode-0600 no-clobber readiness is a run-local capability because it contains two actor private keys. Actor signatures remain transaction authority | Preflight tracked manifest/ELF/ImageID before state; start service; `checkHealth`; exact genesis; mandatory block progress; `sendTransaction` deployment; locate the exact hash/variant in `getTransaction` and containing `getBlock`; derive ProgramId from those ELF bytes; use `getProgramIds` only to bind the static authenticated-transfer owner; verify two `getAccount` ownership/balance results; graceful stdin/Ctrl-C shutdown | Initial exact run rejected the false custom-program-list assertion. Corrected schema-v2 source refuses pre-existing home/readiness, creates a fresh mode-0700 home, and binds endpoint, nonempty deterministic channel, genesis ID/hash, ELF SHA-256, ImageID/ProgramId, deployment transaction hash, containing block ID/hash, authenticated-transfer built-in, account IDs, keys, and balances. The earlier exact full runner passed; after correcting the agreement-invalid zero channel, the three-test locked-graph readiness suite passes and the full runner is a pre-corridor gate. No Docker, faucet, public RPC, or fixed port |
| Logos Core adapter | Planned | No transport/port selected beyond the daemon control endpoint | Protected OS credential handle | `start`, `endpoint`, `health`, `stop` | Optional supervisor of the same daemon binary |
| Delivery / Chat | Planned | No protocol, endpoint, or port selected | Authenticated offers and both-role signed transcript | `OfferDiscovery`; `NegotiationChannel` | Untrusted/removable after first lock |
| Production Zebra watcher routes | Self-hosted and public-provider routes selected; public HTTPS adapter/evidence planned | Zebra 6.0.0 JSON-RPC on operator-owned loopback with cookie auth, or `https://zcash-testnet-zebrad.gateway.tatum.io` with a sensitive `x-api-key`; no Zcash Foundation-operated public Zebra RPC was found | Self-hosted: operator owns cookie/node and public peers provide consensus. Public: Tatum operates the authoritative node/gateway; never substitute generic Zcash RPC or lightwalletd gRPC | Required on both routes: sync/branch/genesis preflight, stable-tip observation, `gettxout`, raw transaction/mempool/block lookup, broadcast, and reorg reconciliation. Public route must pass an exact method smoke before use | Self-hosted initial sync/disk/P2P/epoch risk; provider provisioning/quota/outage/lag/method-policy/trust risk; never switch routes mid-effect or automatically retry an ambiguous broadcast; project transparent signer and live actor suite remain |
| Official LEZ testnet v0.2 node | Live node; local fail-closed deployer implemented, public deployment evidence pending | HTTPS JSON-RPC `https://testnet.lez.logos.co` | Public reads and program deployment transaction; rate limits and reset schedule unspecified | Previously verified `checkHealth`, `getLastBlockId`, `getProgramIds`; live deployment gate additionally requires `getChannelId`, exactly one `sendTransaction`, bounded block-range observation, exact transaction bytes, and canonical containing-block identity | Official LEZ v0.2.0 commit `a58fbce...`; guest/client use `/LEE/` PDA domain; announced resets or channel drift invalidate evidence and prevent automatic resubmission |
| LEZ v0.2 deployment/query client | Executable engineering lane GREEN; live mutation not yet run | Fixed HTTPS JSON-RPC to the official node; loopback `jsonrpsee` server only in exact-once tests; no production listener | Official LEZ transaction/RPC types; program deployment bytes are derived from the checked ELF; no wallet or actor-signing graph is imported | Validate endpoint, channel, built-ins, ATA provenance, ELF SHA-256, ImageID, and ProgramId before RPC; submit deployment once; bind returned/local hash, exact transaction bytes, post-tip block range, block ID, and block hash; timeout or ambiguity forbids retry | Official RPC/type dependencies still pull Logos common/libp2p/Hickory 0.25. Graph-local cargo-deny and feature/reachability tests constrain that disclosed upstream risk; it does not stop M2 evidence under ADR 0018 but remains a production-release blocker |
| Bitcoin Core | M3 planned | No port/image/provider selected | Actor-owned node and wallet credentials | Typed `BitcoinChain` port | Do not infer conventional ports before selection |
| `monerod` plus wallet RPC | M4 planned | No ports/images/providers selected | Actor-owned daemon/wallet credentials | Typed `MoneroChain` port | Wallet/key state remains actor-owned |

## External resources and flakiness

The deterministic SDK/schema-v10 corridor uses only local temporary files and
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
routes are a self-hosted Zebra 6.0.0 public-Testnet node and Tatum's
API-key-authenticated Testnet Zebrad gateway. The former carries initial sync,
disk, peer, epoch, and organic-reorg risk; the latter carries provisioning,
quota, outage, lag, method-policy, and authoritative-provider trust risk.
Community faucets or Discord funding have no
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
