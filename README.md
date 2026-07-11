# LEZ Atomic Swap Suite

Trustless swaps between Logos Execution Zone (LEZ) and Bitcoin, Monero, and
Zcash's transparent pool.

The accepted delivery scope is Gateway's replacement proposal
[logos-co/rfp#112](https://github.com/logos-co/rfp/issues/112), interpreted
together with the live
[RFP-003](https://github.com/logos-co/rfp/blob/master/RFPs/RFP-003-atomic-swaps.md).
The earlier issue #61 is superseded and Ethereum is not an in-scope pair.

## Current status

Development has started with protocol and real-node acceptance tests. The
current executable slices enforce:

- the taker-funded lock is confirmed before the maker can lock the second leg;
- claim completion after the first lock needs only on-chain evidence; and
- pair-specific claim and recovery ordering, including LEZ-before-ZEC claim and
  refund in both ZEC trade directions;
- exact BIP-199 P2SH plus canonical Zcash V5 funding, claim, and refund
  transactions; and
- actor-keyed funding/claim/refund acceptance and rejection through pinned
  Zebra NU6.2 Regtest consensus; and
- checked-guest deployment plus real-key native LEZ initialize/fund/claim and
  permissionless-refund execution in an isolated standalone sequencer; and
- two-definition official-ATA claim/refund lifecycles with real owner keys,
  immutable destinations, and cross-definition substitution rejection; and
- machine-checked recursive native/authenticated-transfer and token/ATA/Token
  Risc0 session costs with setup and Clock noise excluded.

See the living [implementation plan](docs/implementation-plan.md), the
[whole-system actor and flow architecture](docs/architecture/system-architecture.md),
the [architecture decision log](docs/architecture/README.md), and the first
[acceptance tests](crates/swap-core/tests/e2e_swap_lifecycle.rs).

## Development

Prerequisites: Rust 1.96.0. Docker Compose is needed only for the isolated Zebra
consensus suite.

    cargo test --locked --workspace --all-targets
    cargo fmt --all --check
    cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
    cargo deny check advisories bans licenses sources
    RUN_ID=local-zebra-1 ./scripts/run-zebra-e2e.sh

The Zebra suite uses a unique `lez-atomic-swaps-${RUN_ID}` Compose project. It
copies the binary from the digest-pinned official Zebra 5.2.0 image into a
digest-pinned distroless nonroot runtime, then uses a project-only network,
read-only filesystem, tmpfs state, resource caps, no Linux capabilities, and an
ephemeral localhost RPC port. Cleanup addresses that exact project and never
prunes or stops resources it did not create.

## Licensing

Licensed under either the Apache License, Version 2.0 or the MIT License, at
your option.
