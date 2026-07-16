# ADR 0039: Admit first-lock recovery only after a cross-chain cutoff

Status: Accepted for the M3 BTC PoC; refund-side actual-node evidence is GREEN,
canonical maker-lock admission is GREEN in code, and SDK-owned submission plus
fresh actual-node admission evidence remain pending

## Context

The taker always funds first. If the maker disappears before its second lock,
the taker must eventually recover without the maker. A local timeout alone is
unsafe: the taker could decide to refund while a valid maker lock is becoming
canonical on the other chain. The two roles would then disagree about whether
the protocol has one lock or two.

Bitcoin supplies a stable-tip funding observation and the LEZ v0.2 sidecar can
scan a bounded finalized window. Neither observation, alone, is a cross-chain
transaction or a lock on the other actor. LEZ v0.2 also does not expose an
account proof or an atomic multi-account snapshot token. The admission rule
therefore has to be explicit, conservative, and fail closed.

This decision changes the pre-release `BtcRecoveryPlanV1` canonical body by
adding the maker-second-lock cutoff. M3 has not been certified or tagged, so no
released M3 wire is being replaced. Fixtures, manual decoding, agreement
validation, and local provisioning change together. A future post-release wire
change requires a new schema version.

## Decision

Bind `maker_second_lock_cutoff_unix_seconds` into both signatures over the
canonical agreement. Validate that the cutoff is nonzero and no later than the
first recovery boundary after reserving the signed reaction margin.

At lifecycle revision one, normal refund driving has no send authority. The
taker-specific first-lock recovery path must obtain two fresh matching safety
observations. Each observation must prove all of the following:

- the expected maker-funded chain;
- the exact signed cutoff and a canonical chain clock at or beyond it;
- affirmative absence of the exact maker lock, not an RPC error, pending
  transaction, moving tip, incomplete window, or unknown state; and
- an agreement-bound bounded evidence envelope.

After those reads, the refund observer freshly proves that the taker's exact
first lock is canonical, unspent, and eligible at its signed recovery boundary.
Only then may the actor consume the already durable one-attempt refund intent.
Any canonical maker lock wins the admission race and moves the lifecycle toward
the two-lock path. Any uncertainty withholds submission.

```mermaid
sequenceDiagram
    actor Taker
    participant SwapActor as Role-fixed actor
    participant MakerChain as Maker-funded chain RPC
    participant TakerChain as Taker-funded chain RPC
    participant Store as Durable lifecycle store

    Taker->>SwapActor: Recover from revision one
    SwapActor->>Store: Load signed cutoff and durable refund intent
    SwapActor->>MakerChain: Fresh exact-lock classification at stable tip
    alt Maker lock found
        MakerChain-->>SwapActor: Canonical exact second lock
        SwapActor->>Store: Preserve refund intent and follow two-lock path
    else Unknown, pending, incomplete, or moving view
        MakerChain-->>SwapActor: Uncertain
        SwapActor-->>Taker: Awaiting canonical observation
    else Cutoff passed and exact lock absent
        MakerChain-->>SwapActor: Stable bounded absence plus chain clock
        SwapActor->>MakerChain: Repeat fresh exact-lock classification
        alt Second read is not the same safe absence
            MakerChain-->>SwapActor: Found or uncertain
            SwapActor-->>Taker: Awaiting canonical observation
        else Second read confirms safe absence
            MakerChain-->>SwapActor: Stable bounded absence plus chain clock
            SwapActor->>TakerChain: Recheck exact first lock, unspent and eligible
            alt First lock is not safely refundable
                TakerChain-->>SwapActor: Pending or uncertain
                SwapActor-->>Taker: Awaiting canonical observation
            else Exact refund is canonical or safely submitted
                TakerChain-->>SwapActor: Exact canonical refund evidence
                SwapActor->>Store: CAS revision one to revision two Refunded
                SwapActor-->>Taker: Terminal first-lock recovery
            end
        end
    end
```

For `TakerSellsForeign`, the maker chain is LEZ and the taker chain is Bitcoin.
The finalized LEZ classifier returns `Found` or `Absent` only after a complete
stable bounded scan and carries the finalized block timestamp. The Bitcoin
refund observer then validates the exact taker outpoint, CSV eligibility, and
unspent state.

For `TakerSellsLez`, the maker chain is Bitcoin and the taker chain is LEZ.
Bitcoin `Ready` means the maker lock exists, `Pending` includes mempool presence
and remains uncertain, and only stable-tip `Absent` may pass the first gate. The
LEZ refund observer then revalidates the exact escrow state and signed deadline.

The target maker-owned admission flow is direction-specific. It is an
architecture requirement for the remaining SDK/actor-owned submission slice;
the current pushed actor already enforces the final containing-block cutoff when
observing a submitted lock.

### Taker sells Bitcoin and maker funds LEZ

```mermaid
sequenceDiagram
    actor Taker
    participant Bitcoin as Bitcoin Core
    participant MakerSDK as Maker BTC-pair SDK
    participant Store as Maker durable store
    participant Lez as LEZ finalized RPC
    participant TakerActor as Taker actor

    Taker->>Bitcoin: Submit exact Bitcoin first lock
    Bitcoin-->>MakerSDK: Canonical unspent outpoint at signed depth
    MakerSDK->>Lez: Read finalized clock before signed cutoff
    MakerSDK->>Store: Persist exact LEZ initialize and fund plan
    MakerSDK->>Lez: Submit initialize step
    Lez-->>MakerSDK: Finalized initialized escrow
    MakerSDK->>Bitcoin: Fresh exact unspent first-lock check
    MakerSDK->>Lez: Fresh finalized clock before cutoff
    alt Still eligible
        MakerSDK->>Lez: Submit exact fund step
        Lez-->>MakerSDK: Finalized funding and containing timestamp
        alt Containing timestamp at or before cutoff
            MakerSDK->>Store: Commit maker-lock evidence at revision two
            Lez-->>TakerActor: Exact canonical maker lock
            TakerActor->>TakerActor: Open both-lock claim gate
        else Funding exists after cutoff
            MakerSDK->>Store: Record uncertain late presence
            TakerActor->>TakerActor: Withhold claim and first-lock refund
        end
    else First lock changed or cutoff reached
        MakerSDK->>Store: Keep intent without funding authority
    end
```

This direction is conditionally atomic because LEZ funding is the maker's
value-bearing step. Before that step, only the taker's Bitcoin is locked and it
has its signed CSV refund. A timely finalized LEZ funding transaction creates
the two-lock claim path. A late LEZ funding transaction cannot authorize
revision two, but its presence makes first-lock absence unprovable, so the taker
cannot simultaneously refund Bitcoin and treat the LEZ leg as nonexistent.
Initialization without funding holds no maker asset and grants no claim.

### Taker sells LEZ and maker funds Bitcoin

```mermaid
sequenceDiagram
    actor Taker
    participant Lez as LEZ finalized RPC
    participant MakerSDK as Maker BTC-pair SDK
    participant Store as Maker durable store
    participant Bitcoin as Bitcoin Core
    participant TakerActor as Taker actor

    Taker->>Lez: Submit exact LEZ initialize and fund steps
    Lez-->>MakerSDK: Finalized funded escrow and custody balance
    MakerSDK->>Bitcoin: Read stable tip clock before signed cutoff
    MakerSDK->>Store: Persist exact signed Bitcoin funding bytes
    MakerSDK->>Lez: Fresh finalized funded and custody check
    MakerSDK->>Bitcoin: Fresh stable tip clock before cutoff
    alt Still eligible
        MakerSDK->>Bitcoin: Submit exact Bitcoin maker lock once
        Bitcoin-->>MakerSDK: Canonical containing header and median time
        alt Containing median time at or before cutoff
            MakerSDK->>Store: Commit maker-lock evidence at revision two
            Bitcoin-->>TakerActor: Exact canonical maker lock
            TakerActor->>TakerActor: Open both-lock claim gate
        else Funding exists after cutoff
            MakerSDK->>Store: Record uncertain late presence
            TakerActor->>TakerActor: Withhold claim and first-lock refund
        end
    else First lock changed or cutoff reached
        MakerSDK->>Store: Keep intent without funding authority
    end
```

This direction is conditionally atomic because the maker's exact Bitcoin
transaction is persisted before its sole send and the taker's LEZ escrow is
rechecked immediately before that action. Canonical inclusion no later than the
cutoff opens the two-lock claim sequence. Mempool presence is never canonical
admission. Inclusion after cutoff is quarantined as uncertain: the late Bitcoin
leg may require its own timeout recovery, but the taker cannot use the
absent-maker branch while it exists. That sacrifices automatic liveness rather
than permitting incompatible claim and refund histories.

## Atomicity argument

This rule preserves atomicity only in combination with each direction's
construction-specific sequence in `system-architecture.md`:

- before the cutoff, the maker may still create the second lock and the taker
  cannot refund merely because it has not observed it yet;
- after the cutoff, two stable absence reads plus a fresh first-lock unspent
  check prevent ordinary observation lag or one moving view from authorizing a
  conflicting refund;
- a found maker lock suppresses the one-lock branch, so claims remain disabled
  until both exact locks are projected; and
- the refund can return only the taker's own first-locked value. It cannot
  transfer the maker's asset or create claim authority on the other chain.

There is still no distributed cross-chain commit. A sufficiently deep reorg,
Byzantine RPC/indexer, or maker implementation that ignores the signed cutoff
can violate the observations or admission assumption. Production hardening must
add independent-node/reorg evidence and a protocol-level late-lock admission
mechanism where the chain itself cannot enforce the cutoff. These residual
assumptions are disclosed rather than described as unconditional atomicity.

## Consequences and evidence

The additive LEZ classifier preserves the legacy found-only method and never
maps transport, availability, malformed history, or a moving finalized tip to
absence. The actor's ordinary revision-one refund path remains fail closed.
Deterministic RED-GREEN tests cover no-proof refusal, cutoff validation, a
maker-lock appearance between reads, ambiguous second reads, restart/no-rearm,
role ownership, exact terminal projection, and replay idempotence. Pushed
commit `3d202f7` additionally binds Bitcoin containing-block median time or LEZ
finalized containing-block timestamp into maker-lock evidence, accepts before
and exactly at the signed cutoff, rejects later inclusion from revision two,
and maps late presence to uncertain on the refund side. Typed Core
`getblockheader` validation ties hash, confirmation count, height, and median
time to one stable tip while preserving the existing evidence-v1 wire.

Run `m3firstlock-20260716h` supplies the isolated two-direction absent-maker
evidence: no maker second-lock effect, taker-only recovery, revision-two
`Refunded` convergence, zero replay submission, and exact run-owned cleanup.
The remaining certifying run must prove the complementary timely maker path
with the fresh eligibility check and durable plan consumed by the SDK-owned send
action. Until that evidence exists, this ADR is an implemented safety decision,
not an M3 completion claim.
