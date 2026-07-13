# Architecture

The canonical composed view is [System architecture and actor
flows](system-architecture.md), with the concrete process/node/RPC inventory in
[Deployment components, RPCs, and local nodes](deployment-components-and-rpcs.md). It defines the independent actors, runtime and
trust boundaries, and the happy, recovery, and restart lifecycles used to judge
whether a test is genuinely end to end. The ADRs below are append-only records
of the decisions behind that system.

ADRs are append-only. Superseded decisions remain here and link to their
replacement.

## Diagram compatibility policy

Every tracked Markdown Mermaid block must use the conservative GitHub-compatible
subset enforced by CI: stable flowchart, sequence, state-v2, class, or ER
declarations without host configuration, beta/new-shape syntax, or interactive
links and callbacks. The same gate renders every diagram with the exact Mermaid
CLI 11.16.0 pin. GitHub's live Viewscreen asset also reported 11.16.0 on
2026-07-12; its URL and SHA-256 are retained in
[`../evidence/github-mermaid-renderer.json`](../evidence/github-mermaid-renderer.json).
GitHub controls that renderer, so visual verification on GitHub is still
required after pushing documentation changes.

```mermaid
flowchart TB
    System["System architecture + actor flows"] --> Scope["0001 Scope"]
    System --> Deployment["Deployment components + RPCs"]
    System --> Ports["0002 Ports/adapters"]
    Scope --> Ports
    Ports --> Persistence["0003 Persistence"]
    Ports --> Zcash["0004 Zcash stack"]
    Ports --> Docker["0005 Isolated E2E"]
    Scope --> LEZ["0006 LEZ semantics"]
    Ports --> RPC["0007 Maker RPC"]
    Scope --> Direction["0008 Bidirectional ordering"]
    Direction --> Bitcoin["0009 Bitcoin refund"]
    Direction --> Deadlines["0010 Typed deadlines"]
    LEZ --> Deadlines
    Deadlines --> Recovery["0011 Recovery triggers"]
    LEZ --> Custody["0012 Escrow custody"]
    Ports --> SDK["0013 SDK layering"]
    Zcash --> ZecPins["0014 M2 ZEC pins"]
    Custody --> ZecPins
    SDK --> ZecPins
    ZecPins --> ZecReconcile["0015 ZEC reconciliation"]
    SDK --> Agreement["0016 Concrete ZEC agreement"]
    Agreement --> FirstLock["0017 Durable first-lock intent"]
    FirstLock --> MakerLock["0020 Durable maker second lock"]
    MakerLock --> ClaimRecovery["0021 Protected claim recovery"]
    ClaimRecovery --> LezSidecar["0022 LEZ official-wire sidecar"]
    ZecPins --> LezSidecar
    LEZ --> LezSidecar
    ZecPins --> Agreement
    Agreement -.-> ZecReconcile
    FirstLock -.-> ZecReconcile
    FirstLock --> Upstream["0018 Logos production exceptions"]
    Upstream -.-> ZecPins
    Persistence --> ZecReconcile
    Persistence --> SDK
    Persistence --> RPC
    Persistence --> ClaimRecovery
    LezSidecar -.-> Upstream
```

| ADR | Decision | Status |
|---|---|---|
| [0001](0001-authoritative-scope.md) | Live RFP plus accepted issue #112 define BTC/XMR/ZEC scope | Accepted |
| [0002](0002-ports-and-adapters.md) | Explicit protocol core with ports/adapters around external systems | Accepted |
| [0003](0003-sqlite-persistence.md) | SQLite/`rusqlite` persistence behind a repository port | Schema-v10 role-local locks, protected claims, ordered refunds, legacy-secret migration, and atomic replay proven; production effect composition pending |
| [0004](0004-zcash-stack.md) | Zebra plus local canonical transaction construction; selective Zallet use | Accepted |
| [0005](0005-docker-isolation.md) | Per-run Compose project, networks, volumes, and ephemeral ports | Accepted |
| [0006](0006-lez-upstream-semantics.md) | Pin LEZ behavior and verify source assumptions executablely | Accepted |
| [0007](0007-maker-local-rpc.md) | Authenticated local JSON-RPC with a transport-hardening gate | Accepted, production transport pending |
| [0008](0008-bidirectional-role-ordering.md) | Separate product direction from reviewed pair funding capability | Accepted; XMR is LEZ-first only |
| [0009](0009-bitcoin-refund-path.md) | Taproot key-path cooperative claim with script-path CSV refund | Accepted, M3 validation pending |
| [0010](0010-typed-cross-chain-deadlines.md) | Typed consensus clocks plus conservative cross-chain safety bounds | Accepted for deadline legs; XMR superseded by 0011 |
| [0011](0011-event-gated-recovery.md) | Recovery uses typed deadlines or canonical events; XMR has no native timelock | Accepted and represented in core/RPC/CLI |
| [0012](0012-lez-escrow-custody.md) | Split metadata PDA from authenticated-transfer custody or required custom-token ATA | Source-correct native/ATA TDD in progress |
| [0013](0013-sdk-layering.md) | Deterministic common core plus complete per-pair async facades | Concrete ZEC negotiation, locks, role-local claims/refunds, and schema-v10 replay proven; production chain composition pending |
| [0014](0014-zec-m2-implementation-pins.md) | SPEL/LEZ, canonical Zcash crates, and vulnerability-clean minimal Zebra runtime pins for M2 | Accepted for M2 implementation |
| [0015](0015-durable-zcash-observation-reconciliation.md) | Stable affirmative Zcash canonical/removal evidence plus two-phase durable reconciliation | Binding, journal, role projection, conflicts, terminal alerts, and actual two-Zebra restart/requery proven; production poller pending |
| [0016](0016-canonical-zec-agreement.md) | Canonical bounded dual-signed LEZ/ZEC terms bind actors, chains, custody, deadlines, and transaction policy | Validator, activation/resume, locks, claims, and ordered refunds proven; production composition pending |
| [0017](0017-crash-safe-first-lock-intent.md) | Exact role-fixed lock bytes are durable and observed before byte-identical submission | Taker and maker intents plus canonical history survive restart; schema-v10 claims/refunds continue from `BothLegsLocked` |
| [0018](0018-logos-upstream-production-exceptions.md) | Logos-owned live-release blockers are disclosed separately from repository-controlled milestone evidence | Accepted; living production register required through final release |
| [0019](0019-canonical-lez-observation.md) | Stable agreement-bound LEZ fund transaction, block, metadata, and custody evidence is replayed from primitive snapshots | Canonical/update/removal/replacement exact-head folding accepted in SDK/SQLite; official v0.1.2 owner/discovery decoder and main adapter GREEN, SDK-port composition pending |
| [0020](0020-durable-maker-second-lock.md) | Fresh eligibility is consumed by a separately durable opposite-chain maker effect | Separate schema-v10 maker/taker stores replay `BothLegsLocked`; terminal journals continue under 0021 and production adapters remain pending |
| [0021](0021-protected-claim-recovery.md) | Claim material and exact submissions use authenticated envelopes plus role-local schema-v10 claim/refund journals | Both directions, legacy-secret migration/scrub, corruption/rollback/unknown-outcome hardening, and independent replay at `Completed` or `Refunded` proven; key rotation and production adapters pending |
| [0022](0022-isolate-lez-official-wire-sidecar.md) | Keep incompatible pinned LEZ official-wire and Zcash graphs in separate processes behind a bounded local protocol | Accepted for the M2 actual-node corridor; all eight sidecar methods, main LEZ/Zebra validation ports, crash-safe SDK ports, private fresh-client factory, and external-v0.1.2 schema-v2 canonical deployment handoff GREEN; reference actors and composed evidence remain |
