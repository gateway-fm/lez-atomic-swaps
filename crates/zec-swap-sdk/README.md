# lez-zec-swap-sdk

The LEZ/ZEC pair SDK: BIP-199 HTLC scripts and transaction construction on
Zcash's transparent pool, the countersigned agreement codec with its named
recovery profiles, and the durable swap lifecycle.

This crate plus [`lez-swap-sdk-core`](../swap-sdk-core) is the artefact the
RFP-003 milestone-2 deliverable "LEZ/ZEC SDK with full lifecycle coverage"
refers to. It is the only crate implementing `SwapProtocol` for the ZEC/LEZ
pair.

- **What it covers, stage by stage, with the tests that prove each one:**
  [`docs/sdk-lez-zec.md`](../../docs/sdk-lez-zec.md)
- **A worked run of all five stages:**
  [`examples/full-lifecycle.rs`](examples/full-lifecycle.rs)

```sh
cargo run -p lez-zec-swap-sdk --example full-lifecycle
```

It publishes an offer, discovers it, negotiates a countersigned agreement,
activates the swap for both roles, drives the LEZ first lock, the Zcash second
lock, the revealing LEZ claim and the follow-up Zcash claim to `Completed`,
then takes a second swap nobody claims through the ordered LEZ and Zcash
refunds to `Refunded`. Fixed secrets, no I/O.

## Boundaries

`OfferDiscovery` and `NegotiationChannel` are ports. This crate defines, calls,
and validates them, but ships no Delivery or Chat transport — those are
application-owned.

The SDK consumes exact pre-signed LEZ bytes rather than building LEZ
transactions; construction lives in
[`lez-bridge-adapter`](../lez-bridge-adapter) and the role-local sidecar.
Zcash submission and observation are likewise ports, implemented by
[`lez-zebra-node-adapter`](../zebra-node-adapter) against Zebra's JSON-RPC.

The Zcash leg is transparent: amounts, addresses, the script and the revealed
preimage are public. The user pattern is to move a claimed amount to a
shielded address afterwards; `docs/sdk-lez-zec.md` has the privacy posture.

The owner JSON-RPC in [`docs/api/README.md`](../../docs/api/README.md) is the
product surface built on this crate; `docs/sdk-lez-zec.md` maps the two.
