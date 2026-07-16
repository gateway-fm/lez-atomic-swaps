# ADR 0039: Admit first-lock recovery only after a cross-chain cutoff

Status: Accepted for the M3 BTC PoC; actual-node evidence pending

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
role ownership, exact terminal projection, and replay idempotence.

M3 PoC certification additionally requires fresh isolated local-node runs for
both directions, proving that no maker second-lock effect occurred, only the
taker owner submitted recovery, both stores converged on revision two
`Refunded`, replay submitted nothing, and exact run-owned resources were
cleaned. Until that evidence exists, this ADR is an implemented component
decision, not an M3 completion claim.
