# ADR 0035: Project claims only from canonical public evidence

Status: Accepted and implemented for the M3 reference-actor claim-projection
and one-attempt submission boundary in pushed commit `66d352f`. Reproducible
actual-node execution through the public actor processes remains pending.

## Context

After both funding legs are durable at revision two, a revealing claim makes
the adaptor scalar public on the maker-funded chain. The opposite claim can
then spend the taker-funded chain. A node admission response, a transaction ID,
or a locally constructed signature is not canonical chain evidence. Projecting
from any of those would let local lifecycle state get ahead of consensus.

The two roles also learn the revealing scalar differently. The taker already
has the private scalar and must prove that it reproduces the exact observed
signature. The maker may learn it only by extracting it from that signature and
the retained presignature. Persisting the scalar itself would turn ordinary
recovery state into a secret store.

## Decision

The reference actor selects a claim transition only from its durable
predecessor revision:

- revision 2 to 3 observes the revealing claim on the maker-funded chain and
  advances to `ClaimEvidenceAvailable`;
- revision 3 to 4 observes the follow-up claim on the taker-funded chain and
  advances to `Completed`.

The agreement coordinator derives both chains. Bitcoin evidence must meet the
agreement's signed confirmation policy. LEZ evidence must be the exact
finalized observation. Pending evidence preserves the predecessor and grants no
projection authority.

For a locally owned claim, the live actor completes the exact public
transaction from the agreement-bound presignature and the role-appropriate
scalar source, persists its complete bytes and expected ID in the public-effect
journal, and then classifies chain presence. Only a stable bounded absence plus
the journal's one-winner compare-and-swap permits one submission. An accepted
submission remains at the predecessor revision; only later canonical public
evidence can project. A non-submitting role never completes or submits its
peer's claim: it uses terms-and-transcript discovery without receiving a peer
transaction ID.

Immediately before either claim observation, the actor reruns the complete
activation-authority gate from ADR 0034. For revision three, the observation
must include the exact public 64-byte revealing signature. The actor reopens
the agreement-derived signer session and then:

- as taker, rereads the protected scalar, adapts the retained presignature, and
  requires byte equality with the observed signature;
- as maker, extracts the scalar from the retained presignature and observed
  signature through the SDK's relation and adaptor-point checks.

Only `ClaimEvidence`, the one-way scalar commitment, enters lifecycle evidence.
Neither role persists the recovered or reread scalar in the recovery store.
The follow-up observation carries no second revealing signature and reuses the
already durable revision-three claim evidence.

```mermaid
flowchart TB
    Agreement["Validated countersigned agreement"]
    Config["Role fixed actor config"]
    Signer[("Existing signer journal")]
    Gate["Rerun complete activation authority"]
    Completion["Role owned exact claim completion"]
    Effect[("Public effect journal")]
    Observer["Exact or peerless chain observation"]
    Core["Bitcoin Core confirmed evidence"]
    Lez["LEZ finalized evidence"]
    Relation["Taker reproduce or maker extract and point check"]
    Commitment["One way ClaimEvidence"]
    Recovery[("Role local recovery store")]

    Agreement --> Gate
    Config --> Gate
    Signer --> Gate
    Gate --> Completion
    Signer --> Completion
    Completion --> Effect
    Effect --> Observer
    Core --> Observer
    Lez --> Observer
    Gate --> Observer
    Observer --> Relation
    Signer --> Relation
    Relation --> Commitment
    Commitment --> Recovery
    Observer --> Recovery
```

Projection uses the recovery store's expected-predecessor compare-and-swap.
Only a winner at the exact target revision and phase is accepted as concurrent
convergence. Any other conflict fails closed.

```mermaid
sequenceDiagram
    participant Actor as Role fixed actor
    participant Authority as Agreement and signer authority
    participant Effect as Public effect journal
    participant Chain as Bitcoin Core or LEZ adapter
    participant Store as Role local recovery store

    Actor->>Store: Read durable predecessor
    Store-->>Actor: Revision 2 or revision 3
    Actor->>Authority: Rerun complete activation gate
    Authority-->>Actor: Exact role and session authority
    opt Actor owns this claim transition
        Actor->>Actor: Complete exact public claim
        Actor->>Effect: Persist exact bytes and expected ID
    end
    Actor->>Chain: Classify exact or peerless claim presence
    alt Evidence pending or uncertain
        Chain-->>Actor: No canonical exact claim
        Actor-->>Actor: Return with predecessor unchanged
    else Stable bounded absence and owned claim
        Chain-->>Actor: Definitive NotFound
        Actor->>Effect: CAS Prepared to Started
        Effect-->>Actor: SubmitOnce or ObserveOnly
        Actor->>Chain: Submit only after SubmitOnce
        Actor-->>Actor: Keep predecessor until canonical presence
    else Revealing claim at revision 2
        Chain-->>Actor: Confirmed or finalized exact evidence and signature
        Actor->>Authority: Reproduce or extract and point check scalar
        Authority-->>Actor: One way ClaimEvidence only
        Actor->>Store: CAS revision 2 to revision 3
    else Follow-up claim at revision 3
        Chain-->>Actor: Confirmed or finalized exact evidence
        Actor->>Store: CAS revision 3 to revision 4
    end
```

## Atomicity boundary

This decision does not claim an atomic transaction across Bitcoin, LEZ, two
role-local stores, or RPC submission and lifecycle projection. Each actor
projects only its own store after independently observing canonical public
evidence. A crash before projection leaves the predecessor replayable; a crash
after projection replays the committed revision. The separate public-effect
journal in ADR 0033 owns one-attempt submission authority and exact public
bytes. This boundary neither weakens nor replaces that journal.

A chain response that claims presence but conflicts with the durable exact
public effect consumes still-fresh authority as `ConflictingPresence`, moving
it to observation-only `Unknown` without a transport call. Later absence cannot
rearm it. Ordinary finality, history, timeout, and transport uncertainty remain
retryable while the effect is still `Prepared`.

## Consequences

- Thirty-four deterministic actor library tests and seven CLI integration tests
  are GREEN. Eight focused LEZ claim tests cover both owner directions, both
  peerless observers, deterministic request identity, a later discovery window,
  activation reruns, finalized-only projection, and no-rearm restart states.
- An unrelated revealing signature, the wrong chain, insufficient Bitcoin
  confirmations, non-finalized LEZ evidence, or an unexpected signature on the
  follow-up leg fails before projection.
- Offline status can now name `ObserveRevealingClaim`,
  `ObserveFollowupClaim`, and `Complete` from durable revisions two, three, and
  four.
- Exact transaction construction, ADR 0033 submission/reconciliation, and the
  concrete Bitcoin and LEZ actor adapters are composed in source. A fresh
  run-owned harness must still execute both directions through actual local
  Bitcoin and LEZ nodes using the public actor processes and retain terminal
  revision-four evidence before the M3 actor PoC is certified.
