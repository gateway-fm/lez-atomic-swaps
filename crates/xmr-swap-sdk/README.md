# lez-xmr-swap-sdk

The LEZ/XMR pair SDK: the countersigned two-stage agreement (Stage A, the
terms; Stage B, both adaptor sessions), the claim and refund adaptor-session
descriptors, the secp256k1 ↔ Ed25519 cross-curve DLEQ proof that binds each
role's Monero spend-key share to its adaptor point, the shared 2-of-2 Monero
address, the LEZ lock check, and the recovery of the Monero spend key from
one role's share and the other's share revealed by a LEZ signature.

This crate is the artefact the RFP-003 milestone-4 deliverable "LEZ/XMR SDK
with full lifecycle coverage including the Monero spend-key-share recovery
path under partial information loss" refers to. It does no I/O. The Nodes'
Monero actors ([`xmr-reference-actor`](../xmr-reference-actor)) build on its
types and do the chain work; the Monero RPC boundary is
[`lez-xmr-monero-adapter`](../xmr-monero-adapter).

- **What it covers, area by area, with the tests that prove each one:**
  [`docs/sdk-lez-xmr.md`](../../docs/sdk-lez-xmr.md)
- **A worked run of the whole lifecycle, offline:**
  [`examples/full-lifecycle.rs`](examples/full-lifecycle.rs)

```sh
cargo run -p lez-xmr-swap-sdk --example full-lifecycle
```

## The lifecycle

One direction: the Taker sells LEZ. Both roles agree Stage A (each composes
it, the Taker requires the Maker's bytes, both sign), run the claim and the
refund `MuSig2` adaptor sessions, and countersign Stage B. The Taker
withholds its claim partial: Stage B carries only a commitment to it. The
Taker locks LEZ (escrow tag 13); the Maker checks the lock against Stage B
and funds the shared Monero address only on a lock final before the funding
cutoff C. Then:

- **Claim.** The Taker publishes its claim partial (tag 14), the Maker
  completes the claim signature with its Monero share (tag 15) and takes the
  LEZ; the signature reveals the Maker's share, and the Taker sweeps the
  Monero with both shares.
- **Refund.** From the refund time R, before the punish time P, the Taker
  completes Stage B's refund presignature with its share (tag 16) and takes
  its LEZ back; the signature reveals the Taker's share, and the Maker sweeps
  its Monero back.
- **Punish.** If the Taker neither claims nor refunds before P, the Maker
  takes the LEZ (tag 17). No signature reveals a share and the Monero stays
  locked.

[protocol-design.md](../../docs/milestone-1/protocol-design.md#xmrlez) gives
the windows, the escrow's instructions and why the order differs from BTC
and ZEC ([ADR 0217](../../docs/architecture/0217-xmr-taker-sells-lez-deviates-from-the-common-flow.md)).

## Recovery under partial information loss

A role's adaptor journal (its signing nonces, partials and presignatures) and
its session keys can be lost in a restart. What a role keeps in its role
root, its Monero share, the per-swap view key and the bytes of Stage A and
Stage B, is enough with what the LEZ chain shows:

| Lost | Still possible | Through |
| --- | --- | --- |
| The Taker's journal, after tag 15 | The Taker rebuilds the spend key: Stage B's nonces and Maker partial with the partial tag 14 published give the claim presignature again, tag 15 reveals the Maker's share | `XmrActivatedAgreementV1::recover_claim_spend_key` |
| The Maker's journal, after tag 16 | The Maker rebuilds the spend key: tag 16 completes Stage B's refund presignature and reveals the Taker's share | `XmrActivatedAgreementV1::recover_refund_spend_key` |
| The Maker's journal, after tag 14 | The Maker still claims: the claim presignature aggregates from Stage B and the published partial, its share completes it | `verify_published_taker_claim_partial`, then `lez-adaptor-signature`'s `aggregate_adaptor_presignature` and `adapt_presignature` |
| The Taker's journal, before tag 14 | The withheld claim partial is gone, so the Taker cannot authorise the claim; it refunds from Stage B's refund presignature and its share | `XmrAgreementV1::refund_session_descriptor`, `adapt_presignature` |
| A role's own share | Nothing: a wrong share, a partial Stage B does not commit to, or the other session's signature is refused, and every rebuilt key is checked against the agreed address | the errors of the two recovery methods and `ReconstructedMoneroSpendKey::reconstruct` |

Each recovered share is checked against the revealing role's DLEQ proof, and
the rebuilt key against the shared address, before it is returned.

## The example

[`examples/full-lifecycle.rs`](examples/full-lifecycle.rs) runs four swaps
for both roles: a completed one, in which the Maker claims after losing its
journal and the Taker rebuilds the key after losing its own; a refunded one,
in which the Taker loses its journal before authorising, refunds from Stage
B, and the Maker rebuilds the key after losing its journal; a refund after an
unanswered tag 14; and a punished one, in which no key can be rebuilt. Every
key, proof, session, signature and recovery is this SDK's; the LEZ escrow
(tags 13 to 17 with their states, windows, the tag-14 commitment check and
the BIP-340 checks) and the Monero output are modelled in memory, and Chat is
a function call. It prints public values only.

[`tests/full_lifecycle.rs`](tests/full_lifecycle.rs) runs the same swaps and
asserts that every rebuilt key equals the shared address's private spend key
(the sum of both shares) and that every refusal happens. CI runs both.

The same lifecycle on the actor layer the Nodes run (`SQLite` adaptor
journals, the role runner's ceremony seats), with the Monero leg live on the
devnet's regtest, is
[`xmr-reference-actor`'s `examples/full-lifecycle.rs`](../xmr-reference-actor/examples/full-lifecycle.rs).
[`examples/dleq-spike.rs`](examples/dleq-spike.rs) shows the shares, the DLEQ
proofs, the shared address and the recovery on fixed scalars.

## Boundaries

The crate reads no chain and holds no wallet. The LEZ transactions of tags
13 to 17 are built and observed by the role-local sidecar and the Nodes; the
Monero funding, the view-only check and the sweep by the Nodes through
[`lez-xmr-monero-adapter`](../xmr-monero-adapter).

`XmrPairSdk` and `ActiveXmrSwap` are a lifecycle facade over Delivery, Chat
and a role actor port; the Nodes do not use it
([`docs/sdk-lez-xmr.md`](../../docs/sdk-lez-xmr.md#the-facade-is-not-what-the-nodes-run)).
The crate's documentation says it does not claim production cryptographic
acceptance; the construction has had no third-party review.
