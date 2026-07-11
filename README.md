# LEZ Atomic Swap Suite

Trustless swaps between Logos Execution Zone (LEZ) and Bitcoin, Monero, and
Zcash's transparent pool.

The accepted delivery scope is Gateway's replacement proposal
[logos-co/rfp#112](https://github.com/logos-co/rfp/issues/112), interpreted
together with the live
[RFP-003](https://github.com/logos-co/rfp/blob/master/RFPs/RFP-003-atomic-swaps.md).
The earlier issue #61 is superseded and Ethereum is not an in-scope pair.

## Current status

Development has started with protocol acceptance tests. The first executable
slice enforces:

- the taker-funded lock is confirmed before the maker can lock the second leg;
- claim completion after the first lock needs only on-chain evidence; and
- pair-specific claim and recovery ordering, including LEZ-before-ZEC claim and
  refund in both ZEC trade directions.

See the living [implementation plan](docs/implementation-plan.md), the
[whole-system actor and flow architecture](docs/architecture/system-architecture.md),
the [architecture decision log](docs/architecture/README.md), and the first
[acceptance tests](crates/swap-core/tests/e2e_swap_lifecycle.rs).

## Development

Prerequisites: Rust 1.96.0. No Docker services are needed for the current
in-process protocol slice.

    cargo test --locked --workspace --all-targets
    cargo fmt --all --check
    cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
    cargo deny check advisories bans licenses sources

Docker-based suites will use an isolated Compose project, private networks,
named volumes prefixed with `lez-atomic-swaps-`, and ephemeral host ports. The
project never prunes or stops resources it did not create.

## Licensing

Licensed under either the Apache License, Version 2.0 or the MIT License, at
your option.
