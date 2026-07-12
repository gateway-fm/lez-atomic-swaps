# ADR 0017: Durable intent before first-lock effects

Status: taker intent/projection and maker-independent SQLite observation/replay
proven; canonical LEZ native observation and restart replay are proven, while
official-wire LEZ decoding, exact-head reorg reconciliation, and production chain
adapters pending — 2026-07-12

```mermaid
flowchart TB
    Accepted["Validated role-fixed agreement"] --> Prepare["Prepare exact signed submissions"]
    Prepare --> Stage["Atomically stage first-lock intent"]
    Stage --> Durable["Role-local RecoveryStore"]
    Stage --> Encode["Versioned primitive intent record"]
    Encode --> Durable
    Durable --> SQLite["Production SQLite adapter"]
    SQLite --> Tables["Agreement + open or closed intent + transition"]
    Durable --> Restart["Resume without Delivery or Chat"]
    Restart --> Decode["Deserialize only primitive untrusted fields"]
    Decode --> Revalidate["Revalidate agreement, role, revision, direction, and bytes"]
    Revalidate --> Observe["Fresh chain observation before submission"]
    Observe -->|"unstable"| Wait["Wait without node effect"]
    Observe -->|"stable absence"| Submit["Submit byte-identical durable bytes"]
    Submit --> Observe
    Observe -->|"confirmed"| Next{"Another durable LEZ step?"}
    Next -->|"fund pending"| Fund["Observe then submit LEZ fund step"]
    Fund --> Observe
    Next -->|"no"| Projection["Atomic evidence projection"]
    Projection --> Transition["Versioned primitive transition record"]
    Transition --> ClosedIntent["Revalidate with exact retained closed intent"]
    ClosedIntent --> Core
    Projection --> Core["Advance in-memory coordinator after durable proof"]

    Maker["Independent maker SDK + store"] --> MakerObserve["Observe through agreement-selected node"]
    MakerObserve -->|"absent, unstable, or RPC error"| MakerWait["Remain Offered; write nothing"]
    MakerObserve -->|"canonical Zcash or LEZ assertion"| MakerProjection["Atomic maker-role transition + revision"]
    MakerProjection --> Journal["Schema v6 contiguous role-local journal"]
    Journal --> Tracker["Agreement-selected Zcash or LEZ exact tracker fold"]
    Tracker --> Canonical["Canonical or same-inclusion depth event"]
    Tracker --> Replaced["Atomic same-tip removal plus replacement event"]
    Tracker --> Removed["Affirmative exact-head removal event"]
    Canonical --> MakerCore["Replay exact coordinator phase"]
    Replaced --> MakerCore
    Removed --> MakerCore
    MakerCore -->|"poll again after restart"| MakerObserve
    MakerCore --> Gate["SDK next action remains Wait"]
    Gate --> Fresh["Fresh non-cached exact-head eligibility requery"]
    Fresh -.-> MakerEffect["Maker second-lock effect consumes eligibility internally"]

    classDef planned stroke-dasharray: 5 5,fill:#fff7e6,stroke:#9a6700;
    class Observe,Submit,Fund,MakerEffect planned;
```

## Context

The first active SDK seam retained chain and recovery capabilities but did not
use them. Treating a successful RPC return as a protocol transition would be
unsafe: the process can crash after node acceptance but before receiving the
response, or after observation but before persistence. LEZ also has separate
initialize and fund transactions, so one opaque `CreateAndFundLez` effect would
lose the exact crash boundary between them.

## Decision

Before any first-lock node call, the role-fixed taker atomically stages a
versioned immutable intent containing the accepted agreement commitment,
application swap ID, predecessor revision, fixed role, expected chain identity,
and exact signed bytes. A Zcash plan has one funding submission. A LEZ plan
contains separate initialize and fund submissions, both durable before either
node call. Each submission is nonempty and capped at 2,000,000 bytes; expected
identities must be nonzero and the two LEZ identities must differ.

The signed direction selects the plan shape; callers cannot choose a different
first-lock chain. Exact retry is idempotent and changed bytes under the same
role-local swap key conflict. Stable effect IDs are domain-separated by the
agreement commitment, fixed role, and step, not by mutable submission bytes.

Durable JSON never deserializes directly into trusted intent, evidence, or
transition domain types. Version-1 primitive records use explicit stable
snake-case role, step, and plan spellings, reject unknown fields, and retain
schema, swap, commitment, predecessor revision, exact submission bytes, and
confirmed evidence. Reconstruction independently resumes the accepted
agreement and revalidates every bound. A committed transition additionally
requires the exact separately retained closed intent; transition JSON alone is
insufficient recovery evidence.

Driving a staged plan always loads and revalidates the durable intent, then asks
the typed chain adapter to observe the expected identity before any submission.
An unstable observation waits without an effect. Stable absence permits only a
byte-identical submission. Confirmed LEZ initialization permits observation and
possible submission of the already-durable fund step. The adapter receives the
validated agreement as well as the exact submission and must independently
decode and recompute chain policy.

## Executable evidence

The SDK RED–GREEN lifecycle cases prove maker rejection, signed-direction plan
selection, durable-before-effect staging, exact replay, changed-byte conflict,
unstable-query non-submission, observe-before-rebroadcast restart, and ordered
LEZ initialize/fund behavior. They also prove that invalid evidence and a failed
commit leave the coordinator in `Offered`, an unknown successful commit advances
only after an exact predecessor-slot probe, and restart replays the committed
transition to `TakerLockConfirmed`. Adversarial primitive-record tests reject
future schemas, unknown fields, substituted swap/role/commitment/revision/plan,
oversized exact bytes, wrong final step/identity, zero-confirmation evidence,
and a corrupt retained closed intent. Additional cases prove that a maker
queries only its agreement-derived node route, writes no taker intent, does not
advance on absence, instability, RPC failure, wrong-chain evidence, or a failed
commit, adopts an unknown successful commit only by exact probe, and replays its
own role-local observation after restart. Forward Zcash rejects the primitive
transaction-ID/depth assertion: it retains the complete canonical transaction,
block, tip, outpoint, output, script, and depth record, revalidates it against
the signed agreement's HTLC output binding after SQLite restart, and only then
projects. Input candidates, change, fee target, and expiry remain the funder's
role-local construction policy rather than a disclosure requirement imposed on
the remote wallet. The public next-action projection remains Wait, including
after restart, so observation history cannot authorize the maker lock. The
forward maker actor now commits and replays canonical evidence, an atomic
different-transaction replacement, a same-inclusion depth change, and
affirmative removal across revisions 1 through 4. The package currently passes
86 ordinary tests plus one doctest, with the real-Zebra Docker case
intentionally delegated to its isolated runner.

Ten production-store cases instantiate the SDK with a cloneable role-fixed
`SqliteZecRecoveryStore`. They prove exact agreement replay and changed same-key
conflict; maker/taker isolation for the same application ID; an open intent
durable before effects; one immediate transaction that inserts the transition,
advances active revision, and closes but retains the intent; exact replay; and
close/reopen resume to `TakerLockConfirmed`. An external SQLite trigger forces
the middle update to fail and proves all three writes roll back. Future payloads,
malformed primitive JSON, an active revision missing its transition, a closed
intent missing its transition, and an orphan taker row fail closed rather than
reconstructing trusted state. Maker cases prove exact and historical replay,
no taker intent, rollback of row plus revision after a trigger failure,
stale-instance catch-up, four-event close/reopen recovery, rejection of a
same-cardinality journal hole, and rejection of an individually valid
different transaction that lacks atomic replacement evidence.

## Consequences and remaining boundary

The first-lock recovery boundary is now production SQLite durability, but it is
not a completed corridor swap. The adapter uses WAL, `FULL` synchronous mode,
foreign keys, immediate transactions, role-composite keys, primitive payloads,
and full revalidation on every load. Forward Zcash now requires the existing
complete canonical output type and persists its primitive event record; a
production Zebra port must still assemble it from fresh stable RPC snapshots.
The ordered SDK removal/replacement journal is implemented for maker-observed
forward Zcash. Before every append/load, schema v6 proves the exact contiguous
row range and folds all prior records through the agreement-selected exact
tracker and the coordinator. Zcash replacement halves must share the same stable tip;
duplicate canonical polls write nothing; changed inclusion requires explicit
replacement; and stale removal must match the exact tracker head. Replacement
applies remove plus observe to a clone, and the store rejects history poison
before mutation. Reverse LEZ canonical and same-inclusion finality/depth updates
now fold through `LezObservationTrackerV1` in both the active SDK and SQLite;
exact duplicates write no row, and a historical payload-v1 `swap_id` record
is upgraded according to the signed native/token asset before revalidation.
Complete primitive LEZ removal/replacement records now survive restart, consume
one predecessor slot, suppress exact duplicate replacement, and reject stale
old-head removal without mutation. The official-wire node adapter remains. The SDK
therefore exposes no maker second-lock submit method and returns Wait after
maker projection. The distinct fresh pre-effect eligibility call now replays
the durable head and re-queries the exact tracker head. It returns a
revision-bound result without caching authority, adding a journal row, or
changing `next_action`; a future maker effect must consume it internally in
the same operation. Claim/refund
effects, production adapters, and real independent actors remain M2 work;
Logos-owned live-release dependencies are tracked separately under ADR 0018.
