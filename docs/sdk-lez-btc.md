# The LEZ/BTC SDK

The milestone-3 deliverable "LEZ/BTC SDK with full lifecycle coverage" is two
crates:

| Crate | Path | What it is |
| --- | --- | --- |
| `lez-btc-swap-sdk` | [`crates/btc-swap-sdk`](../crates/btc-swap-sdk) | The SDK. Canonical BIP-341 output and transaction construction, role-separated MuSig2 adaptor signing, the agreement codec, and the durable pair lifecycle. |
| `lez-swap-sdk-core` | [`crates/swap-sdk-core`](../crates/swap-sdk-core) | The adapter-independent contracts it implements: `SwapProtocol`, the pre-lock ports, the public-effect vocabulary, and the error taxonomy. |

`lez-btc-swap-sdk` is the only crate that implements
[`SwapProtocol`](../crates/swap-sdk-core/src/lifecycle.rs) for the BTC/LEZ pair.
Everything else in `crates/` is either a chain adapter, a durable store, a Node
binary, or another pair's SDK. The layering is
[ADR 0013](architecture/0013-sdk-layering.md).

## Lifecycle coverage

Every stage below is public API, and every stage has a test that drives it.
Paths are under `crates/btc-swap-sdk/src/` unless stated otherwise.

| Stage | Public API | Where | Proven by |
| --- | --- | --- | --- |
| Offer discovery | `OfferDiscovery::{publish, discover}`, `BtcLifecycleSdk::{publish_offer, discover}` | [`swap-sdk-core/src/ports.rs`](../crates/swap-sdk-core/src/ports.rs), `sdk.rs` | `pre_lock_ports_compose_then_activation_loses_negotiation_capability` |
| Negotiation | `NegotiationChannel::negotiate`, `BtcLifecycleSdk::negotiate`, `BtcPairSdk::accept_wire` | `ports.rs`, `sdk.rs`, `agreement_v1.rs` | same test, plus `tests/agreement_v1.rs` |
| Escrow creation | `BtcPreparedLockEffectsV1`, `PreparedBitcoinFundingV1`, `PreparedLezFundingV1`, `BtcLifecycleSdk::activate`, `ActiveBtcSwap::{first_lock_plan, validate_first_lock, second_lock_plan}` | `sdk.rs`, `asset_sdk.rs` | `both_directions_expose_role_fixed_exact_plans_and_restart` |
| Claim | `ActiveBtcSwap::{claim_order, apply_transition, followup_claim_plan}`, `BtcPairSdk::prepare_claims` | `sdk.rs`, [`swap-sdk-core/src/lifecycle.rs`](../crates/swap-sdk-core/src/lifecycle.rs) | `public_runtime_completes_both_claim_directions_with_restart_and_zero_replay` |
| Refund | `ActiveBtcSwap::recovery_action`, `BtcRecoveryActionV1`, `BtcPreparedRecoveryEffectsV1`, `PreparedBitcoinRefundV1`, `PreparedLezRefundV1` | `sdk.rs` | `public_runtime_completes_both_ordered_refund_directions`, `recovery_boundaries_cover_first_lock_and_ordered_two_lock_timeouts` |
| Durability across all of them | `StoredBtcLifecycleSdk::{activate, resume, apply_transition}`, `BtcLifecycleStore`, `BtcLifecycleRuntime::drive_once` | `sdk/persistence.rs`, `sdk/runtime.rs` | `durable_lifecycle_replays_ordered_refunds_to_revision_four` |

The named tests are in
[`crates/btc-swap-sdk/tests/sdk_facade.rs`](../crates/btc-swap-sdk/tests/sdk_facade.rs).
They run in CI under `cargo test --locked --workspace --all-targets`.

### A worked run

[`examples/full-lifecycle.rs`](../crates/btc-swap-sdk/examples/full-lifecycle.rs)
walks all five stages against an in-memory store and prints what it did:

```sh
cargo run -p lez-btc-swap-sdk --example full-lifecycle
```

It publishes an offer, discovers it, negotiates a countersigned agreement,
activates the swap, drives both locks and both claims to completion, then
replays the same agreement on a second swap that nobody claims and takes it
through both ordered refunds instead. It uses fixed secrets and no I/O, so it
is deterministic and runs offline.

## The five stages in the facade

`BtcLifecycleSdk` composes the deterministic pair SDK with the two pre-lock
ports and hands back a type that no longer has them:

```rust
pub async fn publish_offer(&self, offer: Discovery::Offer)
    -> Result<Discovery::OfferRef, BtcSdkError>;

pub async fn discover(&self, query: &Discovery::Query)
    -> Result<Vec<Discovery::OfferRef>, BtcSdkError>;

pub async fn negotiate(&self, offer: &Discovery::OfferRef,
                       proposal: Negotiation::LocalProposal)
    -> Result<AcceptedBtcAgreementV1, BtcSdkError>;

pub fn activate(&self, accepted: AcceptedBtcAgreementV1,
                prepared: BtcPreparedProtocolV1)
    -> Result<ActiveBtcSwap, BtcSdkError>;
```

`negotiate` does not trust what the channel returns. It passes the bytes to
`BtcPairSdk::accept_wire`, which enforces the wire-size bound, decodes the
exact schema version, verifies both roles' signatures, and validates the
transcript. `activate` consumes both ports' results and returns an
`ActiveBtcSwap`, which has no discovery or negotiation capability at all — once
a lock can be published, the pre-lock ports are gone from the type.

From there the swap is a state machine. `ActiveBtcSwap::next_action` returns a
`BtcLifecycleActionV1` — `PublishBitcoinFirstLock`, `PublishLezSecondLock`,
`PublishBitcoinRevealingClaim`, `PublishLezFollowupClaim`,
`AwaitCounterpartyRefund`, `EvaluateRecovery`, or `Complete` — and
`apply_transition` is the only way to move it forward. Each transition is
revalidated and committed by exact compare-and-swap before the next action is
chosen, so a restart replays rather than re-sends.

## What is deliberately not in the SDK

A reviewer will look for these, so they are stated here rather than discovered.

**Delivery and Chat adapters.** `OfferDiscovery` and `NegotiationChannel` are
generic ports. The SDK defines them, calls them, and validates what they
return, but it ships no transport. Concrete adapters are application-owned and
belong to M5. The example supplies its own in-process implementations, and
`sdk_facade.rs` uses test doubles.

**LEZ transaction construction.** The SDK consumes exact, already-signed LEZ
bytes — `PreparedLezFundingV1::new` takes the signed initialization and funding
transactions. It does not build them. That lives in
[`crates/lez-bridge-adapter`](../crates/lez-bridge-adapter). So "escrow
creation" here means planning, validating, ordering, and driving the escrow
effects, not authoring the LEZ transactions.

**Submission and observation.** `BitcoinBtcLifecyclePort` and
`LezBtcLifecyclePort` are ports too. Sending bytes to a node and observing
canonical evidence is a chain adapter's job
([`crates/btc-core-adapter`](../crates/btc-core-adapter) on the Bitcoin side).

## This SDK and the owner JSON-RPC

"SDK" is overloaded in this repository, so to be explicit: the owner JSON-RPC
in [`docs/api/README.md`](api/README.md) is the *product* surface. The Maker and
Taker Nodes expose it, and they are built on this crate. The two map like this:

| Owner JSON-RPC method | SDK stage |
| --- | --- |
| `maker_offer_publish_v1` | offer discovery (publish) |
| `taker_offer_list_v1` | offer discovery (discover) |
| `taker_swap_initiate_v1` | negotiation, then escrow preparation |
| `taker_swap_lock_v1` | escrow creation — first lock |
| `taker_swap_claim_v1` | claim |
| `taker_swap_refund_v1` | refund |

If you are building a strategy or an integration, the JSON-RPC is the stable
surface. If you are embedding the protocol in your own binary, this crate is.
The Python file under `examples/owner-api/` is a wire example for the former,
not an SDK.

## Scope

This document describes the BTC/LEZ pair at tag `v0.2.6`. `lez-xmr-swap-sdk`
and `lez-zec-swap-sdk` are the Monero and Zcash pair crates and are outside
milestone 3.
