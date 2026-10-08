# lez-btc-swap-sdk

The LEZ/BTC pair SDK: canonical BIP-341 output and transaction construction,
role-separated MuSig2 adaptor signing, the countersigned agreement codec, and
the durable swap lifecycle.

This crate plus [`lez-swap-sdk-core`](../swap-sdk-core) is the artefact the
RFP-003 milestone-3 deliverable "LEZ/BTC SDK with full lifecycle coverage"
refers to. It is the only crate implementing `SwapProtocol` for the BTC/LEZ
pair.

- **What it covers, stage by stage, with the tests that prove each one:**
  [`docs/sdk-lez-btc.md`](../../docs/sdk-lez-btc.md)
- **A worked run of all five stages:**
  [`examples/full-lifecycle.rs`](examples/full-lifecycle.rs)

```sh
cargo run -p lez-btc-swap-sdk --example full-lifecycle
```

It publishes an offer, discovers it, negotiates a countersigned agreement,
activates the swap, drives both locks and both claims to `Completed`, then
replays the same agreement on a swap nobody claims and takes it through both
ordered refunds. Fixed secrets, no I/O.

## Boundaries

`OfferDiscovery` and `NegotiationChannel` are ports. This crate defines, calls,
and validates them, but ships no Delivery or Chat transport — those are
application-owned and belong to M5.

The SDK consumes exact pre-signed LEZ bytes rather than building LEZ
transactions; construction lives in
[`lez-bridge-adapter`](../lez-bridge-adapter). Submission and observation are
likewise ports, implemented for Bitcoin by
[`lez-btc-core-adapter`](../btc-core-adapter).

The owner JSON-RPC in [`docs/api/README.md`](../../docs/api/README.md) is the
product surface built on this crate; `docs/sdk-lez-btc.md` maps the two.
