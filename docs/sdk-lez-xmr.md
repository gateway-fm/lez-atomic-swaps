# The LEZ/XMR SDK

`lez-xmr-swap-sdk` ([`crates/xmr-swap-sdk`](../crates/xmr-swap-sdk)) is the
Monero pair's protocol layer: the countersigned two-stage agreement, the two
adaptor-session descriptors, the cross-curve DLEQ proof, the shared Monero
address and the recovery of the Monero spend key from both shares. It does no
I/O: it reads no chain, holds no wallet and sends nothing. The Nodes' Monero
actors and effect tools ([`crates/xmr-reference-actor`](../crates/xmr-reference-actor))
build on its types and do the chain work; the Monero RPC boundary is
[`crates/xmr-monero-adapter`](../crates/xmr-monero-adapter).

The protocol it serves, and why it is secp256k1 adaptor signatures plus a
cross-curve DLEQ and not "Ed25519 adaptor signatures", is in
[protocol-design.md](milestone-1/protocol-design.md#xmrlez). One direction
only: the Taker sells LEZ and locks first. Regtest only: the agreement knows a
`PublicStagenet` profile, but no Node takes swaps under it and no stagenet run
exists. The crate's own documentation says it "does not claim production
cryptographic acceptance"; the construction has had no third-party review.

Its order departs from the common flow of BTC and ZEC (the Taker authorises, the Maker claims LEZ first); [ADR 0217](architecture/0217-xmr-taker-sells-lez-deviates-from-the-common-flow.md) records that exception.

## What it gives

Paths are under `crates/xmr-swap-sdk/src/`. The named tests are unit tests in
the same file unless stated otherwise; they run in CI under
`cargo test --locked --workspace --all-targets`.

| Area | Public API | Where | Proven by |
| --- | --- | --- | --- |
| Stage A, the agreement | `XmrAgreementBodyV1`, `ValidatedXmrAgreementBodyV1::from_unsigned_wire`, `XmrAgreementV1::from_wire`, `XmrAgreementRecordV1`, `XmrWindowsV1`, `XmrNamedProfileV1`, `XmrSwapDirectionV1` | `agreement_v1.rs` | `unsigned_stage_a_wire_is_the_checked_signed_prefix_and_fails_closed`, `unsigned_stage_a_is_semantically_validated_before_role_signatures_are_attached`, `agreement_mutations_fail_closed_at_their_exact_boundary` |
| The two adaptor sessions | `XmrAgreementV1::{claim_session_descriptor, refund_session_descriptor, claim_partial_context_binding, commit_taker_claim_partial}`, `XmrAdaptorSessionDescriptorV1`, `XmrSessionTranscriptV1` | `agreement_v1.rs` | `adaptor_session_descriptors_reconstruct_exact_contexts_and_fail_closed` |
| Stage B, the activation | `ValidatedXmrActivationBodyV1::from_unsigned_wire`, `XmrActivatedAgreementV1`, `XmrActivationRecordV1` | `agreement_v1.rs` | `unsigned_stage_b_requires_validated_stage_a_and_view_key_before_countersigning`, `unsigned_stage_b_wire_is_the_checked_signed_prefix_and_fails_closed`, `stage_b_activation_and_private_view_key_mutations_fail_closed` |
| The LEZ lock check | `XmrLezInitializePlanV1`, `XmrLezLockCandidateV1`, `XmrValidatedLezLockCandidateV1` | `agreement_v1.rs` | `canonical_stage_b_activation_enables_lez_init_and_candidate_validation`, `lez_lock_candidate_checks_exact_terms_finality_and_funding_cutoff` |
| Cross-curve DLEQ | `CrossCurveScalar::{generate, from_monero_little_endian, adaptor_scalar_big_endian, monero_public_key}`, `CrossCurveDleqProofV1::{prove, verify, verify_scalar, to_wire_bytes, from_wire_bytes}` | `cross_curve.rs` | [`tests/cross_curve_vectors.rs`](../crates/xmr-swap-sdk/tests/cross_curve_vectors.rs): `dleq_vectors_reproduce_and_verify_from_fixed_bytes`, `tampered_proof_and_commitment_are_rejected`, `wrong_point_on_either_curve_is_rejected`, `truncated_and_over_long_encodings_are_rejected`, `trailing_proof_bytes_are_rejected`, `alternate_generators_are_the_published_nums_points`, `proofs_interoperate_with_the_eigenwallet_parameters`, `archived_xmr_btc_swap_basepoint_parameters_are_refused` |
| Shared address and key-share recovery | `MoneroPrivateViewKey`, `MoneroSharedAddressV1::{derive, derive_from_public_view_key}`, `ReconstructedMoneroSpendKey::reconstruct`, `MoneroAddressNetworkV1` | `shared_spend.rs` | `claim_and_signed_refund_reveal_orders_reconstruct_the_same_spend_key`, `unrelated_extracted_or_retained_share_cannot_open_the_funded_address`; `tests/cross_curve_vectors.rs`: `recovery_vector_reconstructs_the_same_key_on_both_paths`, `share_that_does_not_match_is_rejected` |
| A lifecycle facade | `XmrPairSdk`, `ActiveXmrSwap`, `XmrRoleActorPort`, `XmrNegotiationEnvelopeV1`, `XmrLifecyclePhaseV1` | `sdk.rs` | `lifecycle_transition_graph_accepts_claim_refund_and_exact_replay`, `negotiation_envelope_rejects_empty_trailing_and_unknown_schema`; see [the facade](#the-facade-is-not-what-the-nodes-run) |

### The agreement stages

- **Stage A** is one canonical body both roles sign with BIP-340: the
  direction, the named profile, both role packets (agreement, claim-session and
  refund-session keys, the Monero share point and its DLEQ proof), the Monero
  amount, the shared address, the LEZ terms, the three windows (funding cutoff
  C, refund R, punish P) and the exact LEZ messages of the claim, the refund
  and the punish. `from_unsigned_wire` validates an unsigned body before either
  signature is attached; `from_wire` accepts only the canonical dual-signed
  bytes (it re-encodes and compares).
- **The sessions.** The claim session's adaptor point is the Maker's share
  point, the refund session's the Taker's; both session ids derive from the
  agreement commitment, so no random id travels. The Taker's claim partial is
  committed (`commit_taker_claim_partial`), not sent: it is published on LEZ
  later. The SDK says so itself: this "proves consistency at publication, not
  pre-funding validity to the Maker"; there is no verifiable encryption of the
  partial.
- **Stage B** commits Stage A's commitment, both sessions' transcripts, the
  Maker's partials, both refund partials, the commitment to the Taker's claim
  partial and the refund pre-signature; validating it takes the per-swap
  private view key and refuses one that is not the agreement's.
- **The LEZ lock check** validates a finalized LEZ escrow against the
  activated terms, its finality and the funding cutoff before the Maker may
  fund Monero.

### The DLEQ proof and key-share recovery

A share is a `CrossCurveScalar`: nonzero, below 2^252, so one integer is
canonical on secp256k1 and on Ed25519 without reduction. `prove` creates and
self-verifies a proof that the secp256k1 point (the adaptor point) and the
Ed25519 point (the Monero public spend share) have the same discrete
logarithm; `verify` rejects malformed or torsion points, trailing proof bytes
and a changed transcript commitment. A proof exists only verified (`prove` and
`from_wire_bytes` both verify it), so `MoneroSharedAddressV1::derive` does not
verify the two proofs again; it adds their Ed25519 shares into the address's
spend key.
`ReconstructedMoneroSpendKey::reconstruct` takes the scalar extracted from a
completed LEZ adaptor signature (big-endian, as `musig2` gives it), checks it
opens both points of the revealing role's proof, adds the retained share and
checks the result opens the agreed address. The same call serves both orders:
the Taker with the Maker's proof after a LEZ claim, the Maker with the Taker's
after a LEZ refund.

The files under [`tests/vectors/`](../crates/xmr-swap-sdk/tests/vectors) are
**self-generated regression vectors**: deterministic inputs run once through
the pinned `sigma_fun`, `musig2` and `monero` crates. They pin today's bytes;
nobody else publishes vectors for this proof.

Conformance with COMIT is shown for the parameters only
([`tests/cross_curve_vectors.rs`](../crates/xmr-swap-sdk/tests/cross_curve_vectors.rs),
whose header gives the sources):

- `comit-network/cross-curve-dleq`, COMIT's archived proof of concept, proves
  the same statement (one scalar below 2^252 behind the two public keys on the
  standard basepoints) with a different proof, so no proof verifies across
  the two. It is unlicensed and git-only, which `deny.toml` refuses, so it is
  not built here. Its two Pedersen generators are the ones used here:
  `alternate_generators_are_the_published_nums_points` derives them from their
  definitions (BIP-341's NUMS point on secp256k1, Monero's `H` on Ed25519).
- `eigenwallet/core`, the community fork that continues COMIT's archived
  `xmr-btc-swap`, uses `sigma_fun`'s proof with the same two
  generators and the same transcript, on `sigma_fun` 0.7.0, whose proof code
  differs from 0.9.0 only in imports and type signatures.
  `proofs_interoperate_with_the_eigenwallet_parameters` rebuilds its proof
  system with 0.9.0: it accepts the vectors' proofs and makes the same proof
  bytes from the same inputs. That is one library with one set of
  parameters, not a second implementation; their messages carry the proof
  struct in their own encoding, not this SDK's envelope.
- The archived `xmr-btc-swap` passed the standard basepoints instead;
  `archived_xmr_btc_swap_basepoint_parameters_are_refused` shows its proofs
  are refused here and ours there.

The proof's Fiat-Shamir challenge covers the whole statement (both public
keys, both generators, all 252 commitment pairs) under `sigma_fun`'s protocol
name; this SDK's domain string is in the envelope's transcript commitment, not
in the challenge.

### Wire limits

| Constant | Bound |
| --- | --- |
| `MAX_XMR_AGREEMENT_WIRE_BYTES` | 270 KiB, the dual-signed Stage A |
| `MAX_XMR_UNSIGNED_STAGE_A_WIRE_BYTES` | that less 128 bytes (the two signatures and framing) |
| `MAX_XMR_ACTIVATION_WIRE_BYTES` | 2 KiB, the dual-signed Stage B |
| `MAX_XMR_UNSIGNED_STAGE_B_WIRE_BYTES` | that less 128 bytes |
| a DLEQ proof | 128 KiB of proof bytes, 128 KiB + 256 bytes as a wire envelope (`cross_curve.rs`); 129 KiB inside an agreement |

The agreement and activation decoders refuse input over their bound, unknown
schema versions and any bytes that do not re-encode identically; the DLEQ
envelope also refuses empty input and trailing proof bytes. The Chat contracts that carry these bytes
(`crates/node-common/src/chat_contracts.rs`) bound them again as Base64: role
packets at 270 KiB, ceremony packets at 8 KiB.

## A runnable example

[`examples/dleq-spike.rs`](../crates/xmr-swap-sdk/examples/dleq-spike.rs)
uses only public API: two parties' shares (fixed scalars 1 and 2), a DLEQ
proof for each, both verified, the shared regtest address from both proofs and
a view key, and the claim-order recovery of the spend key from the Maker's
extracted share and the Taker's retained one. It asserts the two public keys
of the scalar 1 (the secp256k1 and Ed25519 base points) and prints public
values only (`private_key_bytes_emitted=false`):

```sh
cargo run --locked -p lez-xmr-swap-sdk --example dleq-spike
```

It needs no feature flag and no network. CI compiles it (`--all-targets`) but
does not run it; a run of it is not recorded. The refund order, and an
unrelated share being refused, are the unit tests named in the table above.

## The facade is not what the Nodes run

`XmrPairSdk` composes Delivery, Chat and an `XmrRoleActorPort` and hands back
an `ActiveXmrSwap` whose `advance`, `claim` and `refund` delegate every effect
to the actor port. No Node uses it: the Maker and Taker Nodes run the
take-time Chat rounds and their Monero actors directly on
`xmr-reference-actor`, which uses the agreement, DLEQ and recovery types
above. [`examples/lifecycle-wiring.rs`](../crates/xmr-swap-sdk/examples/lifecycle-wiring.rs)
is a **type-checked skeleton**, not a runnable lifecycle: its actor port's
`activate` and `drive` are `unreachable!()`. Unlike the Bitcoin and Zcash
SDKs, this one has no example that walks offer, negotiation, lock, claim and
refund.

## How the Nodes use it

| Step | SDK | Caller |
| --- | --- | --- |
| Per-swap role material: keys, share, DLEQ proof, view key | `CrossCurveScalar::generate`, `CrossCurveDleqProofV1::prove`, `MoneroPrivateViewKey::generate` | `provision_role_root`, `crates/xmr-reference-actor/src/lib.rs` |
| Stage A composed and checked at take time | `ValidatedXmrAgreementBodyV1::from_unsigned_wire`, `XmrAgreementV1::from_wire` | `reserve_xmr_maker_role`, `check_xmr_taker_role`, `crates/xmr-reference-actor/src/reserve.rs` |
| Both MuSig2 sessions and Stage B | the session descriptors, `ValidatedXmrActivationBodyV1`, `XmrActivatedAgreementV1` | `crates/xmr-reference-actor/src/ceremony.rs` |
| The Taker's sweep after the Maker's LEZ claim | `ReconstructedMoneroSpendKey::reconstruct` | `crates/taker-node/src/taker_service/xmr_supervisor.rs` |

## Scope

The SDK is a pure library and makes no claim about chains. What ran end to end
is the Nodes on the Docker devnet ([deploy/README.md](../deploy/README.md#monero-lez_monero1),
[traceability](requirements-traceability.md) F3); limits that belong to the
construction (no review, no third-party vectors, no verifiable encryption of
the Taker's claim partial, the view key sent to the Maker over Chat, the punish
branch) are stated in [protocol-design.md](milestone-1/protocol-design.md#xmrlez).
