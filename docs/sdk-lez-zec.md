# The LEZ/ZEC SDK

The milestone-2 deliverable "LEZ/ZEC SDK with full lifecycle coverage
(discovery, negotiation, escrow creation, claim, refund)" is two crates:

| Crate | Path | What it is |
| --- | --- | --- |
| `lez-zec-swap-sdk` | [`crates/zec-swap-sdk`](../crates/zec-swap-sdk) | The SDK. BIP-199 HTLC scripts and transaction construction on Zcash's transparent pool, the countersigned agreement codec, the named recovery profiles, and the durable pair lifecycle. |
| `lez-swap-sdk-core` | [`crates/swap-sdk-core`](../crates/swap-sdk-core) | The adapter-independent contracts it implements: `SwapProtocol`, the pre-lock ports, the public-effect vocabulary, and the error taxonomy. |

`lez-zec-swap-sdk` is the only crate that implements
[`SwapProtocol`](../crates/swap-sdk-core/src/lifecycle.rs) for the ZEC/LEZ pair.
The layering is [ADR 0013](architecture/0013-sdk-layering.md): a deterministic
engine, a `ZecPairSdk` facade that composes the Delivery, Chat, chain and
recovery-store ports, and an `ActiveZecSwap` that has no discovery or
negotiation capability once a lock can be published.

## Lifecycle coverage

Every stage below is public API, and every stage has a test that drives it.
Paths are under `crates/zec-swap-sdk/src/` unless stated otherwise; the named
tests are in
[`crates/zec-swap-sdk/tests/sdk_lifecycle.rs`](../crates/zec-swap-sdk/tests/sdk_lifecycle.rs)
unless stated otherwise.

| Stage | Public API | Where | Proven by |
| --- | --- | --- | --- |
| Offer discovery | `OfferDiscovery::{publish, discover}`, `ZecPairSdk::{publish_offer, discover_offers}` | `ports.rs`, `sdk.rs` | `independent_roles_validate_the_same_wire_and_persist_before_activation`; `accepted_wire_activation_and_resume_need_no_discovery_or_negotiation_capability` (the capability is gone after activation) |
| Negotiation | `NegotiationChannel::negotiate`, `ZecPairSdk::negotiate_at`, `ZecAgreementDraftV1`, `ZecMakerAgreementProposalV1`, `ZecAgreementV1::from_wire_at`, `AcceptedZecAgreementV1::accept_wire_at` | `ports.rs`, `sdk.rs`, `agreement_v1.rs` | same test, plus `tests/agreement_v1_cross_binding.rs` (`maker_validates_and_signs_before_the_taker_countersigns_exact_wire`, `devnet_profile_round_trips_on_the_wire_and_an_unknown_tag_is_rejected`) |
| Escrow creation | `FirstLockPlanV1`, `ActiveZecSwap::{stage_first_lock, drive_first_lock, project_first_lock}` (the first leg), `ActiveZecSwap::{observe_taker_first_lock, refresh_maker_funding_eligibility, drive_maker_lock, project_maker_lock, observe_maker_lock}` (the second leg), `build_funding_transaction`, `Bip199Contract` | `sdk.rs`, `first_lock.rs`, `maker_lock.rs`, `transaction.rs`, `lib.rs` | `first_lock_intent_is_role_fixed_and_durable_before_any_effect`, `lez_initialize_is_observed_before_fund_and_each_retry_is_exact`, `maker_independently_observes_and_durably_projects_taker_first_locks_in_both_directions`, `maker_happy_path_locks_the_opposite_chain_and_restarts_in_both_legs_locked`, `independent_actors_reach_both_legs_locked_from_chain_evidence_in_both_directions` |
| Claim | `ActiveZecSwap::{drive_claim, zcash_claim_context, zcash_funding_context}`, `ClaimDriveOutcome`, `ClaimPreimage`, `ZcashClaimContextV1` (the pre-reveal funding gate) | `sdk.rs`, `claim.rs`, `zcash_claim_context.rs` | `first_claimant_activation_persists_agreement_and_claim_preimage_in_both_directions`, `claim_activation_rejects_wrong_role_or_digest_before_activation_or_effect`, `independent_actors_complete_both_claims_taker_first_in_both_directions`, `a_revealing_claim_after_its_window_is_refused_with_nothing_sent_in_both_directions`; `tests/lez_claim_observations.rs` |
| Refund | `ActiveZecSwap::drive_refund`, `RefundDriveOutcome`, `ZecRefundPlanV1`, `ZecRefundProfile::recovery_schedule`, `RefundIntentV1` | `sdk.rs`, `refund.rs`, `profile.rs`, `agreement_v1.rs` | `refund_contract_keeps_lez_before_zcash_selling_zec`, `taker_sells_lez_refunds_each_funder_its_own_leg_in_either_order`, `taker_reorg_between_lez_maker_steps_suspends_funding_until_replacement`; `tests/zec_profiles.rs` |
| Durability across all of them | `ZecPairSdk::{resume, resume_claim_capable, resume_all_capable}`, `RecoveryStore`, `ClaimRecoveryStore`, `RefundRecoveryStore` (implemented on SQLite by `lez-swap-store`) | `sdk.rs`, `ports.rs`, [`crates/swap-store/src/zec_recovery.rs`](../crates/swap-store/src/zec_recovery.rs) | `confirmed_first_lock_commits_before_apply_and_replays_after_restart`, `restart_observes_before_exact_zcash_rebroadcast_without_advancing_phase`, `stale_maker_replays_committed_second_lock_without_resubmission`, `maker_restart_observes_unknown_zcash_submission_before_any_rebroadcast` |
| Zcash consensus | the funding, claim and refund transactions this crate builds, broadcast to a pinned Zebra regtest node | `transaction.rs`, [`crates/zebra-node-adapter`](../crates/zebra-node-adapter) | [`tests/zebra_regtest.rs`](../crates/zec-swap-sdk/tests/zebra_regtest.rs) `real_actor_keys_fund_claim_and_refund_through_zebra_consensus` (fund, claim and refund accepted by Zebra; a competing fork and a concurrent claim/refund reorg recorded) |

The in-process tests run in CI under `cargo test --locked --workspace
--all-targets`; the Zebra test runs in the Zebra regtest job against the pinned
`zfnd/zebra` image.

### A worked run

[`examples/full-lifecycle.rs`](../crates/zec-swap-sdk/examples/full-lifecycle.rs)
walks all five stages against an in-memory store and prints what it did:

```sh
cargo run -p lez-zec-swap-sdk --example full-lifecycle
```

It publishes an offer, discovers it, negotiates a countersigned agreement,
activates the swap for both roles, drives the LEZ first lock and the Zcash
second lock, the Taker's revealing Zcash claim and the Maker's follow-up LEZ
claim to `Completed`, then takes a second swap, which nobody claims, through
the Maker's Zcash refund and the Taker's later LEZ refund to `Refunded`. It uses fixed secrets
and no I/O, so it is deterministic and runs offline.

## The five stages in the facade

`ZecPairSdk` composes the deterministic pair engine with the two pre-lock
ports and the chain ports, and `activate` hands back a type that no longer has
the pre-lock ports:

```rust
pub async fn publish_offer(&self, offer: Discovery::Offer)
    -> Result<Discovery::OfferRef, ZecSdkError>;

pub async fn discover_offers(&self, query: &Discovery::Query)
    -> Result<Vec<Discovery::OfferRef>, ZecSdkError>;

pub async fn negotiate_at(&self, offer: &Discovery::OfferRef,
                          proposal: Negotiation::LocalProposal, now: UnixSeconds)
    -> Result<AcceptedZecAgreementV1, ZecSdkError>;

pub async fn activate(&self, accepted: AcceptedZecAgreementV1)
    -> Result<ActiveZecSwap<Lez, Zcash, Store>, ZecSdkError>;
```

`negotiate_at` does not trust what the channel returns. It passes the bytes to
`AcceptedZecAgreementV1::accept_wire_at`, which enforces the wire-size bound,
decodes the exact schema version, verifies both roles' signatures, checks the
named profile against the LEZ environment and the Zcash network, and validates
the transcript against the clock. `activate` persists the accepted agreement
(and, for the Taker, the claim preimage under a protected key) before any
chain effect.

From there the swap is a state machine. `ActiveZecSwap::next_action` returns a
`ZecLifecycleAction` — `CreateAndFundLez`, `FundZcash`, `ClaimLez`,
`ClaimZcash`, `RefundZcash`, `RefundLez`, `Wait`, or `Complete` — and the
`drive_*` methods
are the only way to move it forward. Each transition is revalidated and
committed by exact compare-and-swap at the next revision before the next
action is chosen, so a restart replays rather than re-sends. The reference
actors in
[`crates/zec-reference-actor`](../crates/zec-reference-actor) wrap this
engine as one-shot processes (`activate`, `drive`, `claim`, `recover`,
`status`); the Maker Node's supervisor and the Taker Node's lifecycle call the
same engine in-process.

## Directions

Both directions are implemented and tested at the SDK level (`sdk_lifecycle.rs`
drives every stage in `TakerSellsForeign` and `TakerSellsLez`). They follow
the flow BTC follows
([ADR 0216](architecture/0216-zec-taker-reveals-first-in-both-directions.md)):
the Taker locks first and holds the preimage, the Taker claims the
Maker-funded leg first and so reveals it, the Maker claims the Taker-funded
leg with the preimage it extracts, and the Maker-funded leg refunds first.

| | `TakerSellsForeign` | `TakerSellsLez` |
| --- | --- | --- |
| First lock | Taker funds the Zcash HTLC | Taker creates and funds the LEZ escrow |
| Second lock | Maker funds the LEZ escrow | Maker funds the Zcash HTLC |
| Revealing claim (Taker) | `ClaimLez`, step `RevealingLez` | `ClaimZcash`, step `RevealingZcash` |
| Follow-up claim (Maker) | `ClaimZcash`, step `FollowupZcash` | `ClaimLez`, step `FollowupLez` |
| Earlier refund (Maker) | LEZ, at the LEZ refund time | Zcash, at the Zcash refund height |
| Later refund (Taker) | Zcash, at the Zcash refund height | LEZ (`RefundLez`), at the LEZ refund time |

Who holds the secret: the per-swap preparation generates the preimage and
gives it to the Taker's half only. `activate_with_claim_preimage` is accepted
for the Taker and refused for the Maker in both directions; the Maker's store
first holds the preimage when it commits the observed revealing claim, whose
transition carries the extracted preimage checked against the digest. The prepared ZEC setup is single-operator: the preparation ceremony (`zec-provision.sh`, `local_poc.rs`) signs the template agreement with both roles' keys and holds both halves, the preimage included, while it runs. It is safe only where the preparer is trusted by the Taker (a devnet, a Testnet acceptance run by one owner). Node-side take-time preparation, in which the Taker's Node creates the secret and each Node its own Zcash key, is tracked in issue #162 for M5.

The Taker's revealing claim is gated. In `TakerSellsLez`, before it stages or
submits its Zcash claim, the Taker requires the Maker's funding output to be
canonical, unspent and at the profile's Zcash confirmations,
`tip + zcash_reveal_margin_blocks < zcash_refund_height`
(else `ZecSdkError::ZcashRevealWindowClosed`), and
`now < lez_refund_at − required_margin` (else `ClaimWindowClosed`), the margin
being the Maker's budget for its follow-up LEZ claim. The gates apply to the
first send only: once the claim is staged its preimage may be public, so the
exact stored bytes are re-broadcast whenever they are absent and the HTLC
output is unspent, whatever the window. A Taker whose window closed with no
claim staged refunds its LEZ at the LEZ refund time.

The Maker learns the preimage through
`ZcashClaimPort::observe_counterparty_revealing_claim`, which finds the spend
of the exact HTLC outpoint and reads it on consensus facts only: the input
takes this contract's claim branch and carries a 32-byte preimage of the
digest (`read_bip199_spend_branch`). Destination, fee, other outputs, expiry
height, sighash flags and signature form are the Taker's business; Zebra
admits only consensus-valid transactions. A refund spend or a wrong preimage
never yields one. The Maker commits the reveal as soon as it sees the claim,
in the mempool or in a block (`RevealingClaimEvidenceV1::from_observed_zcash_claim`):
a valid preimage stays valid whatever happens to that transaction, and
waiting for depth would only spend the Maker's margin. The Taker's own
revealing evidence still waits for the Zcash leg's depth. The Maker's
follow-up LEZ claim uses the `LezClaimPort` follow-up methods
(`prepare_followup_claim` and the rest).

A Maker Node signs every LEZ claim with one account, so its follow-up LEZ
claims go one at a time (a submitted claim holds the next until it is final
or its swap's LEZ refund time passes; a failed swap's claim for one
admission bound after the failure), the swap with the earliest LEZ refund
time first,
and it accepts at most `zec_taker_sells_lez_capacity(profile)` open
`TakerSellsLez` ZEC swaps (refused with `zec_taker_sells_lez_capacity_reached`):
3 under `deterministic-local-devnet-v7`/`-v8`, where `M = 120 s` holds three
38 s slots (a LEZ claim measured at most 6.3 s from submission to final on
the devnet, three samples on 2026-11-18, doubled with the held-claim retry),
and 1 under `deterministic-local-v3` and `public-testnet-v3`. On the LEZ
testnet the escrow takes the claim if it is included before the refund
time, but the Node holds the next claim until the previous one is final,
which is the reason for 1 there; no LEZ testnet claim latency has been
measured.

A Node refuses to start while it holds an open `TakerSellsLez` ZEC swap under
a legacy profile and names the swaps: finish or refund them with the previous
release first (no upgrade between releases before 1.0.0, issue #133). The
same holds for a swap under a Taker-reveals-first profile: a Taker claim
staged by an earlier build (which set an expiry height) no longer validates
against this build, which stages it with none, so such a swap is finished
with the build that staged it; there is no fallback.

In `BothLegsLocked` with no claim, each role drives its own refund (the Taker
cannot sign the Zcash refund, so its LEZ refund does not wait for it); once
one leg's refund is recorded the other role's follows. No refund is admitted
after claim evidence is recorded, except the `TakerSellsLez` Maker's own
Zcash refund once its follow-up LEZ claim can no longer land (the Taker's
LEZ refund final; ADR 0216). `TakerSellsLez` deviates from RFP-003 F4,
which puts the LEZ refund first in both directions; ADR 0216 records why.

<!-- Part D: rewrite when TakerSellsForeign through the Nodes lands (handover steps 17-21). -->
The Node-driven settlement on the local Docker stack and the M6 PoC
certificates cover `TakerSellsLez`. `TakerSellsForeign` through the Nodes is
not claimed here; it is covered at SDK level and by the PoC corridor tests
only, and the evidence set says so.

## Recovery profiles

`ZecRefundProfile` names immutable parameter sets
([`docs/milestone-1/parameter-profiles.md`](milestone-1/parameter-profiles.md)).
`TakerSellsLez` needs a Taker-reveals-first profile:
`deterministic-local-v3` (test lane), `deterministic-local-devnet-v7`
(devnet swaps that claim), `deterministic-local-devnet-v8` (devnet refund
journeys) or `public-testnet-v3`; under any older profile the agreement is
refused (`ZecAgreementV1Error::LegacyRevealOrder`). The older profiles admit
`TakerSellsForeign` only:
`deterministic-local-v1` for the isolated test lane, `public-testnet-v1` for
the public networks (NU6.2; `public-testnet-v2` is its NU7 successor), and
`deterministic-local-devnet-v3` (eight-block Zcash
refund window and a 12-minute LEZ refund delay; `-v2` is its 30-minute
predecessor and `-v1` the four-block one, both kept for the agreements signed
under them; `-v4`, its 4-minute twin, serves the devnet's refund journeys;
`-v5` and `-v6` are `-v3` and `-v4` on NU7, which the devnet's Zebra 7 runs
from height 1, and `deterministic-local-v2` is the isolated lane's NU7 twin) for a local devnet
whose LEZ chain finalises in minutes rather than instantly. The LEZ escrow
program emits a timestamp validity window that ends at the agreement's LEZ
refund time, so a profile's `lez_refund_delay` is also the longest a lock may
take to reach the chain after the agreement's anchor; the devnet profile
sets it to 12 minutes (v3, v5, v7), short enough for a refund to finish
inside the pinned devnet chain's finality lifetime (upstream #3643). A
revealing claim held behind another swap's needs attention a third of that
delay before the refund time. The profile is part of the signed agreement
(`ZecProfileRecordV1`), and evidence states which profile it ran under.

## Privacy posture

The Zcash leg lives on the **transparent pool**. The BIP-199 HTLC, its
funding, the claim that reveals the preimage, and any refund are ordinary
transparent transactions: amounts, addresses, the script, and the preimage are
public on the Zcash chain, and the LEZ leg is public on LEZ. The SDK makes no
privacy claim for the swap itself; what it does is bounded:

- the Maker and Taker Zcash keys in an agreement are per-swap keys
  (`ZecParticipantIdentityV1`), so a swap does not reuse a long-lived address;
- the claim material is kept under a protected key in the recovery store
  (`ProtectedClaimEnvelope`), so a stopped actor's database does not expose the
  preimage in the clear;
- no shielded operations are performed by the SDK, and none are implied.

The user pattern is **shield after swap**: once a transparent ZEC claim has
confirmed, move the funds to a shielded address in a wallet that supports it.
The Taker Node reports exactly this on a completed ZEC swap
(`TakerPrivacyGuidanceV1::ShieldReceivedTransparentZecSeparately`), the Taker
desk shows it as "After a transparent ZEC claim, move funds to a shielded
wallet address.", and the design background is in
[`docs/milestone-1/protocol-design.md`](milestone-1/protocol-design.md) and
[ADR 0004](architecture/0004-zcash-stack.md).

## What is deliberately not in the SDK

**Delivery and Chat adapters.** `OfferDiscovery` and `NegotiationChannel` are
generic ports. The SDK defines them, calls them, and validates what they
return, but it ships no transport. The run-local Delivery directory and the
Maker's Chat socket the Nodes use are application-owned
([`crates/node-common`](../crates/node-common), `maker-node/src/zec_chat.rs`,
`taker-node/src/zec_taker_accept.rs`). The example supplies its own
in-process implementations, and `sdk_lifecycle.rs` uses test doubles.

**LEZ transaction construction.** The SDK consumes exact, already-signed LEZ
bytes through the LEZ ports. Building and signing the escrow initialisation,
funding, claim and refund transactions lives in
[`crates/lez-bridge-adapter`](../crates/lez-bridge-adapter) and the role-local
sidecar it talks to. "Escrow creation" here means planning, validating,
ordering and driving the escrow effects, not authoring the LEZ transactions.

**Zcash submission and observation.** `ZcashFirstLockPort`, the observation
ports, `ZcashClaimPort` and `ZcashRefundPort` are ports too. Talking to a Zcash
node is [`crates/zebra-node-adapter`](../crates/zebra-node-adapter)'s job
(Zebra's JSON-RPC, regtest and public testnet).

## This SDK and the owner JSON-RPC

The owner JSON-RPC in [`docs/api/README.md`](api/README.md) is the *product*
surface. The Maker and Taker Nodes expose it, and they are built on this
crate. The two map like this:

| Owner JSON-RPC method | SDK stage |
| --- | --- |
| `maker_offer_publish` / `maker_offer_publish_v1` | offer discovery (publish) |
| `taker_offer_list_v1` | offer discovery (discover) |
| `taker_swap_initiate_v1` (a `prepared_zec` catalog entry) | negotiation over the Maker's Chat socket (`zec_chat_propose_v1`, `zec_chat_complete_v1`), then actor provisioning |
| the Taker actor's `drive` (no desk button; driven on the Node host) | escrow creation — first lock |
| the Maker Node's supervisor | escrow creation — second lock, then the Maker's follow-up claim |
| `taker_swap_claim_v1` | claim (the Taker's revealing claim: Zcash in `TakerSellsLez`, LEZ in `TakerSellsForeign`) |
| `taker_swap_refund_v1`, `maker_actor_refund_v1` | refund (the Taker's LEZ or Zcash leg; the Maker's Zcash or LEZ leg) |

A ZEC take on the Nodes is prepared, not composed: the Taker's catalog entry
carries the signed offer envelope, the unsigned draft bound to that offer, the
signing key and the actor template, all produced by
`scripts/zec-provision.sh` from live chain facts. If you are building a
strategy or an integration, the JSON-RPC is the stable surface. If you are
embedding the protocol in your own binary, this crate is.

## Scope

This document describes the ZEC/LEZ pair at the M2 tag, `v0.3.0`. Conditional atomicity holds as the M6 service certificates state it:
under canonical local-chain observations, secret-gated claiming, durable
one-winner authorization, and actor/sidecar at-most-once journals; it is not a
distributed atomic commit and not a future-reorg proof. `lez-btc-swap-sdk` and
`lez-xmr-swap-sdk` are the Bitcoin and Monero pair crates and are outside
milestone 2.
