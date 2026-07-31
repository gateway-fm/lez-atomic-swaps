# ADR 0114: Compose the XMR application before chain effects

- Status: Accepted for application cutoff; role-correct chain replay pending
- Date: 2026-07-30
- Milestone: M5 progressive local-functional PoC

## Context

ADR 0113 proves the pre-effect XMR application boundary in component and
process tests. Authenticated Delivery determines the swap ID, Chat atomically
activates canonical Stage A and Stage B in application SQLite, and a sealed
Maker child semantically validates role authority before returning typed
`Blocked`. That child contacts neither chain.

The actual M5 splice must connect that application path to the existing M4
Tag13 through Tag15 claim path without creating a second swap identity or two
concurrent effect owners. M4 role journals necessarily advance during Tag14,
Tag15, extraction, and sweep. Schema-v2 application authority pins their raw
pre-effect bytes, so the application supervisor must finish and stop before
legacy journal progression begins.

This record defines the composition and evidence boundary. A 2026-07-30 role audit found that the historical chain runner used the provisioner as funder, the Taker RPC as shared-wallet host, and the Maker as claim-sweep destination. The retained run remains valid cryptographic, chain, binder, and cleanup evidence, but no longer certifies the user-economic role topology. The corrected decision requires Maker funding, a distinct neutral provisioner shared-wallet process, Maker-mined claim confirmations, and Taker receipt; fresh exact-commit replay evidence is pending. Exact isolated run
`m5-xmr-app-20260730-2c6aec1-h` completed every functional step from the exact
pushed tree and passed fail-closed cleanup schema v2. This decision is accepted
only for the private local-functional PoC boundary described here.

## Decision

1. A Delivery-only Maker publishes one signed run-local `Monero` plus
   `TakerSellsLez` offer. `lez-taker` authenticates the exact offer, reservation,
   principal, and no-rounding quote, then derives the public swap ID from the
   signed-envelope commitment and reservation ID.
2. That Delivery-derived swap ID is passed explicitly to the M4 agreement
   composer. Its receipt, decoded Stage A, Stage B, both role bundles, and all
   later receipts must bind the exact same lowercase 32-byte identifier.
   Format-only validation is insufficient.
3. The Delivery-only process stops by exact identity before the full Maker
   application starts. The full daemon receives daemon-owned Maker agreement
   identity, private view authority, and a registry entry pinning the Maker
   schema-v2 manifest, role journal, and `xmr-maker-actor` binary.
4. Maker and Taker bundles are provisioned from the same canonical stages while
   retaining role-correct private roots, packet ordering, journals, stage
   paths, and source digests. The Taker publishes its acceptance receipt only
   after durable activation.
5. Stage A reserves revision 2 without a coordinator, actor, or chain effect.
   One Stage-B SQLite transaction commits revision 3, consumes the offer,
   creates one coordinator, registers one queued Monero Maker actor, and records
   replay. Exact replay preserves those cardinalities and published inodes.
6. Before any legacy journal access, the normal supervisor executes one bounded
   due cycle. The sealed child returns typed `Blocked`, remains queued without a
   child identity or manual action, records
   `xmr_chain_effects_not_yet_composed`, and creates no RPC or public effect.
7. The full daemon and supervisor stop by exact identity, the original Delivery
   mailbox is archived, and the same daemon restarts from durable SQLite. Its
   publisher intentionally reconciles the consumed offer because consumed
   offers remain retryable for lost-response recovery. The real Taker
   authenticates that reconstructed advertisement, the mailbox is archived,
   and an empty outage mailbox replaces it. The Taker then exact-replays from
   durable Stage A, Stage B, actor, and receipt state without a Delivery
   argument. The daemon stops synchronously using registered PID, start ticks,
   binary digest, and process-group identity. Process and group absence,
   mailbox emptiness, Chat-socket and readiness-file absence, application state,
   and both pre-effect journal digests are checked. Only this durable cutoff
   permits chain effects.
8. After the cutoff, the legacy actual-chain path owns liveness and advances
   the same signed swap in this exact order: finalized LEZ Initialize, finalized
   LEZ Fund, confirmed Monero lock, prepared and finalized Tag14 authorization,
   Maker journal completion, finalized Tag15 claim, Taker ingestion and
   adaptor-scalar extraction, Monero sweep, and exact claim-to-sweep binding.
9. The original schema-v2 supervisor is not restarted after a legacy journal
   changes. Its raw-journal digest is a pre-effect authorization snapshot and
   reuse must fail closed. A future application-owned corridor needs a new
   versioned authority projection.
10. Every application process and ephemeral path enters the exact run ledger
    immediately. The in-flow cutoff does not rely on EXIT cleanup. The prior
    cleanup-reset hazard that cleared accumulated failures before final
    evaluation is addressed in source but remains unverified pending an actual
    run and cleanup evidence.

## Components, nodes, and RPC topology

```mermaid
flowchart LR
    subgraph Application["M5 application boundary"]
        Planner["Delivery-only Maker"]
        Delivery["Signed run-local Delivery"]
        Reconciled["Authenticated retry advertisement"]
        Outage["Empty Delivery outage mailbox"]
        Taker["lez-taker"]
        Composer["M4 Stage composer"]
        Bundles["Maker and Taker bundles"]
        Daemon["Full Maker daemon"]
        Chat["Chat Unix socket"]
        AppDb[("Application SQLite")]
        Supervisor["Fenced supervisor"]
        Child["xmr-maker-actor"]
        Cutoff["Process-absence cutoff"]
    end

    subgraph Legacy["Existing actual-chain path"]
        Runner["Tag13 to sweep runner"]
        Journals["Maker and Taker journals"]
        TakerSidecar["Taker LEZ sidecar"]
        MakerSidecar["Maker LEZ sidecar"]
    end

    subgraph LezLocal["Isolated LEZ v0.2 stack"]
        Sequencer["Sequencer RPC"]
        Indexer["Indexer RPC"]
        Escrow["Authenticated-transfer program"]
    end

    subgraph MoneroLocal["Official Monero regtest stack"]
        MoneroDaemon["Daemon JSON-RPC"]
        MakerWallet["Maker economic wallet RPC: fund source and claim miner"]
        SharedWallet["Neutral provisioner RPC: shared wallet only"]
        TakerWallet["Taker economic wallet RPC: claim destination"]
    end

    Planner --> Delivery
    Delivery --> Taker
    Daemon -->|"durable reconcile"| Reconciled
    Reconciled --> Taker
    Reconciled -->|"archive then isolate"| Outage
    Taker -->|"derived swap ID"| Composer
    Composer --> Bundles
    Bundles --> Taker
    Bundles --> Daemon
    Taker -->|"public stages"| Chat
    Chat --> Daemon
    Daemon --> AppDb
    AppDb --> Supervisor
    Bundles --> Supervisor
    Supervisor --> Child
    Child -->|"typed Blocked"| Supervisor
    Daemon --> Cutoff
    Supervisor --> Cutoff
    Cutoff -->|"liveness handoff"| Runner
    Journals --> Bundles
    Runner --> Journals
    Runner --> TakerSidecar
    Runner --> MakerSidecar
    TakerSidecar --> Sequencer
    TakerSidecar --> Indexer
    MakerSidecar --> Sequencer
    MakerSidecar --> Indexer
    Sequencer --> Escrow
    Indexer --> Escrow
    Runner --> MoneroDaemon
    Runner --> MakerWallet
    Runner --> SharedWallet
    Runner --> TakerWallet
```

Delivery and Chat are filesystem and Unix-socket transports. LEZ uses isolated
v0.2 sequencer and indexer RPCs plus authenticated role sidecars. Monero uses
an isolated official regtest daemon and authenticated wallet RPCs for funding,
shared-output observation, and destination receipt. Test funds are deterministic
local genesis or regtest funds. No public RPC, faucet, DNS lookup, deployment,
or external finality provider participates. Local process and Docker startup,
block production, wallet synchronization, and bounded finality polling can
still vary with host load and must appear in phase evidence.

## End-to-end sequence

```mermaid
sequenceDiagram
    participant PlanMaker as Delivery-only Maker
    participant Delivery as Run-local Delivery
    participant Taker as lez-taker
    participant Composer as M4 composer
    participant Daemon as Full Maker daemon
    participant Store as Application SQLite
    participant Supervisor as Maker supervisor
    participant Child as xmr-maker-actor
    participant Legacy as Legacy M4 runner
    participant Journals as Role journals
    participant LEZ as Isolated LEZ stack
    participant XMR as Monero regtest stack

    PlanMaker->>Delivery: Publish signed Monero offer
    Taker->>Delivery: Authenticate exact offer and quote
    Taker->>Taker: Derive swap ID
    Taker->>PlanMaker: Stop exact provisional process
    Taker->>Composer: Compose with explicit swap ID
    Composer-->>Taker: Stage A, Stage B, and role material
    Taker->>Daemon: Start with pinned Maker authority
    Taker->>Daemon: Submit Stage A
    Daemon->>Store: Reserve revision 2 with no actor
    Taker->>Daemon: Submit Stage B
    Daemon->>Store: Atomically activate revision 3 and queue actor
    Daemon->>Supervisor: Run one due cycle
    Supervisor->>Child: Pass sealed authority on FD 196
    Child-->>Supervisor: Typed Blocked with zero chain requests
    Supervisor->>Store: Persist zero-effect bounded recheck
    Taker->>Daemon: Stop exact authority process and group
    Taker->>Delivery: Archive original advertisement mailbox
    Taker->>Daemon: Restart from durable SQLite
    Daemon->>Delivery: Reconcile consumed retry advertisement
    Taker->>Delivery: Authenticate identical swap and terms
    Taker->>Delivery: Archive reconciliation and create empty outage mailbox
    Taker->>Daemon: Replay Stage A and Stage B without Delivery argument
    Daemon->>Store: Revalidate with no duplicate rows or files
    Taker->>Daemon: Stop replay process and group
    Taker->>Taker: Prove process, socket, and readiness absence
    Taker->>Legacy: Cross exclusive cutoff
    Legacy->>LEZ: Finalize Initialize then Fund
    Legacy->>XMR: Fund and confirm shared output
    Legacy->>Journals: Prepare Taker Tag14 state
    Legacy->>LEZ: Submit and finalize Tag14
    Legacy->>Journals: Complete Maker claim signature
    Legacy->>LEZ: Submit and finalize Tag15
    Legacy->>Journals: Ingest claim and extract scalar
    Legacy->>XMR: Sweep exact shared output
    Legacy->>Legacy: Bind finalized claim and sweep
```

The first legacy journal access occurs only after process absence. Canonical
stages, application bundles, and acceptance receipts remain immutable while
the source journals perform their expected protocol transitions.

## Atomicity and cutoff argument

```mermaid
flowchart TD
    Plan["Authenticated Delivery plan"] --> Identity["One swap ID"]
    Identity --> Stages["Signed Stage A and Stage B"]
    Stages --> Reserve["Stage A reservation with zero authority"]
    Reserve --> Tx["One Stage-B SQLite transaction"]
    Tx --> Local{"Application commit succeeds"}
    Local -->|"no"| NoActor["No actor and no chain effect"]
    Local -->|"yes"| Queued["One coordinator and queued actor"]
    Queued --> Validate["Sealed semantic validation"]
    Validate --> Blocked["Typed Blocked with zero effect"]
    Blocked --> Reconcile["Restart reconciles consumed retry advertisement"]
    Reconcile --> Authenticate{"Same signed swap and terms"}
    Authenticate -->|"no"| Abort
    Authenticate -->|"yes"| Outage["Archive advertisement and create empty mailbox"]
    Outage --> Replay["Delivery-free durable replay"]
    Replay --> Stop["Synchronous process stop"]
    Stop --> Absent{"Exact absence proven"}
    Absent -->|"no"| Abort["Abort before chain effects"]
    Absent -->|"yes"| Handoff["Legacy runner owns liveness"]
    Handoff --> LezLock["Finalized LEZ locks"]
    LezLock --> XmrLock["Maker funds confirmed shared output in neutral RPC"]
    XmrLock --> Auth["Finalized Tag14"]
    Auth --> Claim["Finalized Tag15"]
    Claim --> Reveal["Bound scalar extraction"]
    Reveal --> Sweep["Taker receives exact Monero sweep"]
    Sweep --> Binding["Claim and sweep binding"]
    Binding --> Conditional["Conditional cross-chain atomicity"]
    Conditional -.-> NoDtx["No distributed transaction"]
```

Application atomicity completes before supervision. The Stage-B transaction
commits the negotiation, consumed offer, coordinator, actor, and replay row
together or commits none. The supervisor is a post-commit authorization check.
Its typed blocked child cannot submit chain effects, so stopping it cannot leave
a partial submission or weaken the durable application commit. The cutoff
transfers liveness, not safety.

Cross-chain atomicity is conditional, not transactional. Signed stages bind the
LEZ-first ordering and adaptor transcripts. Finalized LEZ funding gates Monero
funding, confirmed Monero funding gates Tag14, finalized Tag14 gates Maker
completion, and finalized Tag15 exposes the signature needed to recover the
checked Maker scalar and sweep Monero. Each gate has independent replay-bound
evidence.

There is no distributed transaction among application SQLite, journal SQLite,
LEZ, and Monero. The cutoff prevents concurrent effect ownership. Stopping the
supervisor can reduce liveness if the legacy runner never starts, but cannot
authorize a claim, refund, or sweep.

## Execution evidence and acceptance

Exact run `m5-xmr-app-20260730-da9be26-f` proved:

- plan, composer, decoded stages, bundles, and receipts share one swap ID;
- Stage A has no executable rows and Stage B plus replay retain exactly one
  coordinator and actor without replacing published files;
- both journal bytes and inodes remain unchanged through application replay and
  the typed blocked cycle;
- restart reconciliation republishes the exact retryable consumed offer, the
  real Taker authenticates it, and exact replay remains unchanged after that
  mailbox is archived and replaced by an empty Delivery outage mailbox;
- supervisor status, child absence, bounded recheck, zero effects, zero chain
  RPCs, and the process cutoff are durable evidence rather than log inference;
- Tag13, funding, Tag14, Tag15, extraction, sweep, and binding succeeded for the
  application-derived swap. Claim finalized at LEZ tip 146; the Monero sweep
  reached 10 confirmations at tip 130; and the binder explicitly rejected a
  distributed-transaction claim.

Every exact run resource was absent and the foreign sentinel survived, but one
cleanup command returned nonzero. The fail-closed result therefore remained
`failed` even though the source result was zero. Cleanup schema v1 did not
identify the operation. Schema v2 now records stable failure reason codes.

Exact pushed-tree run `m5-xmr-app-20260730-9067ba3-g` repeated the same
functional sequence with source status zero. Claim finalized at LEZ height 140
and tip 143; the Monero sweep received 998191600000 of 1000000000000 funded
piconero after a 1808400000 fee and reached 10 confirmations at tip 130.
Binding completed, every exact resource was absent, and the foreign sentinel
survived. Schema v2 isolated exactly three
`ephemeral_path_boundary_failed` reasons. Each rejected path was a nested
directory below the exact run-owned private namespace; the boundary accepted
the namespace itself but not its descendants. Commit `fb4e279` canonicalizes
paths and admits only descendants of the run-owned private root, while focused
tests reject traversal, symlinks, and foreign paths.

Exact pushed-tree run `m5-xmr-app-20260730-2c6aec1-h` then passed the same
functional sequence and cleanup schema v2. Swap `9d627d18...abfeb7c` retained
one identity through the application cutoff, tag 13/14/15, extraction, sweep,
and binding. Claim transaction `05cb9052...349fce` was included at LEZ height
139 and observed at finalized tip 142. Monero sweep transaction
`37930570...1603c8` received 998191600000 of 1000000000000 funded piconero
after a 1808400000 fee and reached 10 confirmations at tip 130. Cleanup recorded
source status zero, exact resources/processes/ports absent, preserved foreign
sentinel and no-retry latch, no foreign target, no broad cleanup, and no failure
reason.

The cross-chain binder scopes the result to
`successful_claim_path_conditional_atomicity`. It explicitly sets both
`distributed_cross_chain_transaction_claimed` and
`future_reorg_immunity_claimed` to false. No public RPC, faucet, peer, or public
fund participates. This evidence accepts the cutoff/ownership decision for the
local PoC; it does not accept daemon-owned XMR lifecycle execution, arbitrary
concurrency, adverse reorg/race behavior, or production readiness. No latched
successful-effect run ID is ever reused.
