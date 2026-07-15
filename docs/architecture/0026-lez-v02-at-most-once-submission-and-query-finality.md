# ADR 0026: Use at-most-once LEZ v0.2 submission and query-bound finality

Status: Accepted; role-bound at-most-once Vault submission and durable Admitted
state are GREEN. The M3 witnessed-claim bridge now integrates bounded finalized
block and same-containing-BlockId terminal account observation for either
participant. Vault query/journal finality, ambiguous multi-effect
restart/recovery, refunds/reorg/chaos, upstream historical-account proofs, and
public execution remain deferred -- reconciled 2026-07-15

```mermaid
flowchart LR
    Actor["Role-isolated actor process"]
    Journal[("Role-local durable effect journal")]
    Effect["LEZ v0.2 effect coordinator"]
    Sequencer["LEZ v0.2 sequencer RPC"]
    Bedrock["Bedrock settlement node"]
    Indexer["LEZ v0.2 indexer RPC"]
    Admitted["Durable Vault Admitted state"]
    Auditor["Separate post-run finality auditor"]
    Evidence[("Aggregate run evidence")]

    Actor --> Effect
    Effect -->|"persist exact request and bytes"| Journal
    Journal -->|"durable AttemptStarted"| Effect
    Effect -->|"one sendTransaction call"| Sequencer
    Effect -.->|"integrated bounded inclusion query deferred"| Sequencer
    Sequencer -->|"publish LEZ block"| Bedrock
    Bedrock -->|"finalized channel block"| Indexer
    Effect -.->|"Vault integrated finality deferred"| Indexer
    WitnessedObserver["M3 exact or peerless witnessed-claim observer"] -->|"bounded finalized blocks by ID and hash<br/>unique terms and transcript match"| Indexer
    WitnessedObserver -->|"metadata and custody at containing BlockId"| Indexer
    Sequencer -->|"accepted hash"| Admitted
    Effect -->|"persist Admitted"| Admitted
    Auditor -->|"exact transaction and block queries"| Sequencer
    Auditor -->|"exact finalized block and account queries"| Indexer
    Admitted --> Evidence
    Auditor --> Evidence
    Evidence --> Complete["Durable Vault admission plus external finality audit<br/>and both canonical corridors GREEN"]
    Complete -.-> Deferred["Integrated query/journal finality, ambiguous restart,<br/>refund/reorg/chaos, public execution deferred"]
```

## Context

ADR 0025 originally left durable submission and finality observation pending.
This decision fixed that boundary before any prepared v0.2 transaction became
eligible for a node effect. Current Vault actor evidence crosses durable
submission and admission; a separate run audit proves exact inclusion, finality,
and account state, and the canonical corridor effects are independently audited.
Integrated bounded query and journal progression remain deferred. It applies to
Vault Claims, escrow deployment,
native escrow initialization and funding, claims, refunds, and every later
official v0.2 transaction sent by a role process.

The decision comes from direct source review of official LEZ tag `v0.2.0`, exact
commit `a58fbce2ff48c58b7bb5001b1a27e64b9596ee3a`. All upstream paths below are
relative to that commit. The relevant executable behavior is:

- the generated sequencer RPC trait at
  `lez/sequencer/service/rpc/src/lib.rs:35-92`;
- sequencer admission at `lez/sequencer/service/src/service.rs:46-82`;
- later stateful validation and rejection at
  `lez/sequencer/core/src/lib.rs:392-435`;
- the volatile Tokio-channel mempool at `lez/mempool/src/lib.rs:3-67`;
- sequencer transaction lookup at
  `lez/sequencer/core/src/block_store.rs:97-114`;
- Bedrock finalization handling at `lez/sequencer/core/src/lib.rs:188-198` and
  `lez/sequencer/core/src/block_publisher.rs:158-171`;
- the generated indexer RPC trait at
  `lez/indexer/service/rpc/src/lib.rs:9-74`;
- finalized Bedrock ingestion at `lez/indexer/core/src/lib.rs:73-159`;
- indexer transaction lookup at `lez/indexer/core/src/block_store.rs:56-68`;
- finalized block storage at `lez/indexer/core/src/block_store.rs:196-202`;
  and
- absent-account defaulting at `lee/state_machine/src/state.rs:260-265`.

The official APIs expose enough information to prove an exact transaction by
queries, but they do not expose one atomic submit-and-receipt operation.
Specifically, v0.2 has no caller idempotency key, `AlreadyKnown` result, mempool
lookup, execution-rejection query, or transaction receipt containing block
position and finality.

## RPC facts

The implementation must use the official generated clients and protocol types.
It must not copy the JSON, base64, Borsh, transaction, account, block, hash, or
signature wire models.

| Boundary | Exact generated method | What it can prove |
|---|---|---|
| Sequencer | `sendTransaction(LeeTransaction) -> HashType` | Size and stateless signature checks passed and the value was queued before the response |
| Sequencer | `getTransaction(HashType) -> Option<LeeTransaction>` | The exact hash exists in the sequencer's persisted local block store |
| Sequencer | `getLastBlockId() -> BlockId` | Current local block scan bound |
| Sequencer | `getBlock(BlockId) -> Option<Block>` | Block identity, transactions, position, and local Bedrock status |
| Sequencer | `getAccountsNonces(Vec<AccountId>) -> Vec<Nonce>` | Current sequencer-state nonces in request order |
| Sequencer | `getAccount(AccountId) -> Account` | Current sequencer account value, subject to absent/default ambiguity |
| Indexer | `getLastFinalizedBlockId() -> Option<BlockId>` | Last block stored by the finalized-only indexer ingestion path |
| Indexer | `getTransaction(HashType) -> Option<Transaction>` | Exact hash is present in the indexer's finalized store, but not its position |
| Indexer | `getBlockById(BlockId) -> Option<Block>` | Finalized block, transactions, and transaction position |
| Indexer | `getBlockByHash(HashType) -> Option<Block>` | Independent lookup of the same finalized block identity |
| Indexer | `getAccountAtBlock(AccountId, BlockId) -> Account` | Account state at the proved finalized block |

The indexer has no `getAccountBalance` method. Its `Account.balance` is the
balance result. Neither sequencer nor indexer `getTransaction` returns a block
ID, block hash, transaction index, execution status, or rejection reason.

`LeeTransaction` on the sequencer JSON-RPC is a standard-base64 representation
of Borsh bytes. The first implementation slice is deliberately limited to Vault
Claim onboarding: its `PreparedTransaction.exact_bytes` is the canonical inner
`PublicTransaction::to_bytes()` value. That submitter decodes and revalidates
those exact inner bytes with official types and wraps the result once as
`LeeTransaction::Public`. It must not persist, reconstruct, or submit a
different encoding. This rule is variant-specific rather than a claim that all
effects are public transactions: escrow deployment uses the official
`LeeTransaction::ProgramDeployment` variant and requires its own exact prepared
payload validation.

## Decision

### Durable at-most-once submission

Each node effect has one durable isolation key containing run, a typed scope,
role, logical operation, request identity, complete runtime identity, and exact
transaction identity. Scope is explicit rather than an optional or invented
swap ID:

- Vault onboarding uses `VaultOnboarding { owner, vault, allocation }`; and
- corridor effects use `Swap { swap_id }`; the implementation now exists and is
  exercised by both canonical local directions.

Each journal is created with an immutable run, role, complete runtime, and
signer binding. A maker journal cannot be opened as taker or with a different
signer or runtime. Maker and taker use separate owner-only directories and
separate journal files; sharing one database does not satisfy role isolation.
The owner-only directory and database defend against accidental sharing and
other OS principals; they do not make an actively malicious process running as
the same UID harmless. Production role isolation therefore uses distinct OS
users or containers for mutually untrusted actors. Every path ancestor must be
root/current-UID owned and either non-group/other-writable or sticky. The code
revalidates that chain, the held directory, database inode, schema, canonical
metadata, and actor binding before each read or mutation. Active same-UID or
root writers remain outside this filesystem threat model because they can
rewrite process memory or actor-owned files.

The same role-local transaction that makes an effect eligible persists:

1. a typed, bounded, deny-unknown-fields request and the immutable actor binding;
2. the canonical exact inner transaction bytes and official transaction hash;
3. the bounded sequencer and indexer discovery windows;
4. typed official `nssa::Account` before-snapshots for every affected account,
   plus the captured sequencer and indexer query tips; and
5. the effect state.

Arbitrary JSON blobs, nonce-only before-state, and caller-asserted transaction
fields are not durable authority. Construction is fallible and revalidates the
stored request, signer, account order, allocation, nonce, canonical bytes,
signature, official hash, runtime, and program identity with the same official
Vault Claim planner used during preparation.

For the pinned genesis path, `SupplyAccount` executes Faucet to Vault to
authenticated-transfer. The pre-Claim owner is `Account::default()`; its
derived Vault has the exact allocation, zero nonce and default data, and
`program_owner == programs::authenticated_transfer().id()`. The official LEE
genesis block ID is 1. Consequently, an indexer with no finalized tip starts its
fixed discovery window at `lee::GENESIS_BLOCK_ID`, not at block 0.

Before the first network call, the journal atomically advances the effect to
`AttemptStarted` with attempt count exactly one and synchronously commits it.
Only a newly committed `AttemptStarted` transition may authorize the single
`sendTransaction` call. No code path may call the sequencer first and record the
attempt afterward.

An exact reopen from `AttemptStarted` or any later state has observe-only
permission. It may query the exact transaction, blocks, finality, and accounts,
but it must never call `sendTransaction` again. Observe-only is not an evidence
state and cannot erase or demote `Admitted` or `Included`. A transport timeout,
connection reset, process crash, missing response, or returned-hash mismatch
leaves the evidence state at `AttemptStarted`, records bounded uncertainty
metadata, and removes send permission permanently.

A JSON-RPC call error with code `-32602` (`InvalidParams`) is a definitive
pre-enqueue rejection in the pinned handler and may be recorded as rejected.
No transport error or other JSON-RPC error is reclassified as rejection. A
successful `sendTransaction` response is recorded as admission only when the
returned hash equals the durable official transaction hash; a wrong returned
hash never replaces the expected identity and remains uncertain
`AttemptStarted` evidence.

```mermaid
sequenceDiagram
    participant A as Actor effect coordinator
    participant J as Durable journal
    participant S as Sequencer RPC

    A->>J: persist exact request bytes hash and query bounds
    J-->>A: Prepared committed
    A->>J: commit AttemptStarted with attempt one
    J-->>A: AttemptStarted durable
    A->>S: sendTransaction exactly once
    alt Exact hash returned
        S-->>A: admitted hash
        A->>J: record Admitted
    else InvalidParams returned
        S-->>A: definitive pre-enqueue rejection
        A->>J: record Rejected
    else Transport wrong hash or response ambiguity
        S--xA: outcome unknown
        A->>J: retain AttemptStarted and annotate uncertainty
    end
```

This is an at-most-once call guarantee, not exactly-once delivery. A crash after
`AttemptStarted` commits but before the HTTP call can leave a transaction that
was never submitted and can never be retried automatically. Moving the commit
after the call would close that liveness gap but create a crash window in which
the same effect can be submitted again. Without an upstream idempotency or
receipt contract, both guarantees cannot be obtained simultaneously. This ADR
chooses safety and explicit operator-visible uncertainty over blind resubmission.

The official public nonce rules provide a second state-effect boundary: a
transaction whose signer nonce executed once cannot execute again with the same
nonce. That does not make repeat submission acceptable. Duplicate copies may
still enter the volatile mempool, consume resources, and produce ambiguous
evidence before one is rejected during stateful block construction.

### Exact sequencer inclusion

Admission is never inclusion. The coordinator durably captures a finite
`DiscoveryWindow` before submission and uses only official sequencer queries.
Within that exact inclusive height range it:

1. obtains the current `getLastBlockId` without expanding the durable maximum;
2. queries each covered `getBlock` by ID;
3. validates the requested block ID and linked block identity when adjacent
   blocks are available;
4. locates exactly one transaction whose official hash and canonical inner
   bytes equal the durable prepared transaction;
5. records the block ID, block hash, transaction index, and returned Bedrock
   status; and
6. cross-checks `getTransaction(hash)` against the same official transaction.

A missing, duplicated, malformed, substituted, outside-window, or moving
result is not rejection and not absence. It remains unknown or pending. A
sequencer block with `Pending` or `Safe` status proves local inclusion only.
The dependent effect, including native escrow funding after initialization,
does not become eligible merely because the predecessor was admitted or locally
included.

### Query-bound indexer finality

Finality is proved through the independent indexer view of Bedrock-finalized
LEZ blocks. Indexer `getTransaction` is necessary evidence but does not expose
the containing block. The coordinator therefore performs the exact durable
bounded block scan and requires all of the following:

1. `getTransaction` returns the exact official transaction hash and content;
2. exactly one `getBlockById` result in the durable range contains it;
3. the transaction index, block ID, and block hash are recorded;
4. the block reports `BedrockStatus::Finalized`;
5. `getBlockByHash` returns the same block ID, hash, and transaction content;
6. `getLastFinalizedBlockId` is at least the containing block ID; and
7. every effect-specific account invariant passes against
   `getAccountAtBlock(account_id, block_id)`.

The proof uses account-at-block rather than only current account state because
later valid transactions may have changed the current value. For a Vault Claim,
the finalized owner balance increases by the exact allocation, the owner Vault
balance decreases by that amount, the owner nonce increments once, and expected
program ownership and data remain valid.

An account response alone cannot prove existence. The state machine returns
`Account::default()` for an absent account, so an effect-specific validator must
check the complete expected program owner, balance, data, and nonce tuple. It
must not interpret a successful `getAccount` or `getAccountAtBlock` RPC response
as proof that the requested account existed before the query.

The `subscribeToFinalizedBlocks` subscription is a wake-up hint only. The pinned
indexer can log a failed block-store write and still yield the block ID to
subscribers. A notification therefore cannot advance an effect to `Finalized`
without the complete query proof above.

```mermaid
sequenceDiagram
    participant R as Recovered actor
    participant J as Durable journal
    participant S as Sequencer RPC
    participant I as Indexer RPC

    R->>J: load exact effect and fixed windows
    J-->>R: AttemptStarted or later
    Note over R,S: Reopen has query-only permission
    R->>S: getTransaction and bounded getBlock queries
    S-->>R: exact local inclusion or pending result
    R->>J: record unique inclusion position
    R->>I: getTransaction and bounded getBlockById queries
    I-->>R: exact finalized block candidate
    R->>I: getBlockByHash and getLastFinalizedBlockId
    I-->>R: matching finalized block identity
    R->>I: getAccountAtBlock for affected accounts
    I-->>R: finalized account snapshots
    R->>J: record Finalized only after all checks
```

## Durable state contract

The monotonic minimum state progression is:

```mermaid
stateDiagram-v2
    [*] --> Prepared
    Prepared --> AttemptStarted: durable attempt one
    AttemptStarted --> Admitted: exact hash response
    AttemptStarted --> Rejected: InvalidParams only
    Admitted --> Included: exact sequencer block proof
    Included --> Finalized: exact indexer block and account proof
    Finalized --> [*]
    Rejected --> [*]

    note right of AttemptStarted
        Ambiguity or reopen removes send permission
        and records uncertainty without changing evidence state
    end note
```

Observe-only permission and uncertainty are orthogonal annotations, not
transitions in the monotonic evidence state. They may coexist with
`AttemptStarted`, `Admitted`, or `Included`; reopening never demotes one of
those states. Reaching the end of a configured window produces an explicit
unresolved result requiring policy or operator action; it does not turn the
effect into `Rejected`, `Absent`, or automatically retryable. A fresh
transaction with a new nonce and new durable effect identity requires a higher
level recovery decision after the old effect is reconciled conservatively.

Every transition uses a compare-and-swap revision in one immediate SQLite
transaction. Exact replay is idempotent. Changed bytes, hash, typed scope,
request, immutable actor binding, window, operation, or state revision is a
conflict. A forced database failure rolls back the complete transition and
cannot leave a completed request without its exact payload or a finality marker
without its block and account proof.

## Historical TDD delivery plan and current reconciliation

The architecture is accepted. The RED/GREEN list below records the original
delivery plan. Items 1 and 2 are GREEN for the role-local Vault journal. The
actual PoC adds generated-RPC submission and durable Admitted state; a separate
manual audit supplies aggregate inclusion, indexer block/hash/account-at-block
finality, and later spendability evidence toward items 3 and 4. The integrated
bounded-query state machine and journal progression beyond Admitted are not
implemented. Both canonical corridor directions were independently audited.
Item 5 remains partial: positive actual-node ordering is GREEN, while integrated
finality, ambiguous response, and restart injection across every composed effect
remain later hardening.

### RED

1. A fake sequencer records whether durable `AttemptStarted` exists before its
   first call and fails until the ordering is enforced.
2. Restart tests kill or reopen after `Prepared`, after `AttemptStarted`, after
   the remote response but before its local commit, after `Included`, and before
   `Finalized`; the total sequencer send-call count must never exceed one.
3. Transport failures, wrong returned hashes, and process loss fail until they
   retain `AttemptStarted`, record uncertainty, and reopen without send
   permission.
4. Inclusion tests reject wrong bytes, wrong hashes, duplicate positions,
   noncontiguous block facts, height overflow, moving bounds, and matches outside
   the durable window.
5. Finality tests reject an indexer transaction without its exact containing
   block, a non-final status, block-ID/hash disagreement, incomplete account
   evidence, and default-account ambiguity.
6. A finalized-block subscription notification without durable query results
   must not advance state.
7. Role, run, typed scope, signer, runtime, request, operation, and transaction
   substitutions must not share state or authorize a call; maker and taker
   journals must reject cross-role opens and records.
8. A forced SQLite failure must preserve the previous complete durable state.

### GREEN gate

1. Implement the first slice only for a revalidated Vault Claim wrapped once as
   official `LeeTransaction::Public`; do not treat program deployment as a
   public transaction or add copied wire types or manual encoding.
2. Bind each role-local journal immutably to run, role, runtime, and signer;
   persist typed exact preparation and `AttemptStarted` atomically before the
   one node call, and prove query-only reopen through the restart matrix.
3. Prove exact bounded sequencer inclusion and indexer block/hash/account-at-block
   finality with deterministic adapters.
4. Repeat the same path against the isolated full LEZ v0.2 stack for maker and
   taker as separate processes and role-local journals.
5. Retain one actual-node happy path plus ambiguous-response and restart evidence
   before any actor-readiness, corridor, or milestone claim.

The initial 2026-07-14 slice implemented items 1 and 2 for Vault onboarding.
Seventeen focused tests prove typed owner/Vault/allocation/runtime binding,
canonical exact-byte duplication checks, immutable role-local actor binding,
durable attempt-before-call ordering, one-call replay, forced concurrent
compare-and-swap, crash windows, wrong-hash and transport uncertainty,
coordinator-owned JSON-RPC error classification, failed response persistence,
malicious reset-trigger rejection, exact revision shapes, parent-chain and
filesystem substitution rejection, and redacted diagnostics. Later actual-node
evidence added real one-shot submission and durable Admitted state. The separate
manual audit supplies aggregate evidence toward items 3 and 4 and proved the
balances spendable; it does not complete the integrated query/journal path.
Composed ambiguous-response, process-restart, refund, reorg, and chaos evidence
remains required for the corresponding hardening claims, not for the
owner-approved M2 local happy-path PoC.

## 2026-07-14 canonical actual-node reconciliation

Separate maker and taker Vault effects committed AttemptStarted before their
only generated-RPC submission and each role journal reached durable Admitted. A
separate manual auditor then proved unique sequencer inclusion, independent
indexer finality, and finalized account state. The resulting funds were used by both canonical actor corridors. The same
external audit located their initialize/fund/claim transactions in
finalized LEZ blocks 2594/2595/2596 and 2605/2606/2607; both actor pairs ended
revision 4 Completed.

This proves aggregate positive effect plus external finality evidence. It does
not prove integrated bounded finality/journal progression or that every corridor
effect recovers from a crash-before-call, ambiguous response, process kill,
refund, removal/replacement, reorg, or chaos. Public endpoint behavior also
remains deferred.

## Consequences

- RPC admission, local inclusion, Bedrock finality, and effect-specific state
  transition are separate evidence states.
- Restart recovery is deterministic and does not create a second automatic
  submission attempt.
- The chosen safety property can strand an effect after a crash-before-call
  ambiguity. Operators see an unresolved state instead of a false success,
  false rejection, or unsafe retry.
- Bounded scans avoid unbounded node work and prevent a later unrelated
  transaction from satisfying an old effect.
- Account-at-block proof binds the state transition to the same finalized block
  as the exact transaction.
- Subscriptions can reduce polling latency but never become finality authority.
- Dependent LEZ and cross-chain effects remain gated on the required finalized
  predecessor, preserving the protocol's atomicity arguments as far as the
  available upstream interface permits.

## Upstream production requirements

The missing idempotency, receipt, mempool, and execution-rejection surfaces are
Logos-owned production gaps recorded in
[`../upstream-production-blockers.md`](../upstream-production-blockers.md).
They do not waive this repository's durable ordering, exact-query validation,
bounded recovery, role isolation, or actual-node M2 evidence. A supported
idempotent submit contract and authenticated transaction receipt could later
improve liveness, but replacing this decision requires a new ADR and regression
evidence against an immutable upstream release.
