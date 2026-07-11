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
- immutable local/public-testnet ZEC profiles with network/branch binding,
  checked deadlines, required calibration, and exact margin enforcement;
- exact BIP-199 P2SH plus canonical Zcash V5 funding, claim, and refund
  transactions; and
- actor-keyed funding/claim/refund acceptance and rejection through pinned
  Zebra NU6.2 Regtest consensus, including a two-node conflicting
  four-over-three-block canonical fork replacement; and
- checked-guest deployment plus real-key native LEZ initialize/fund/claim and
  permissionless-refund execution in an isolated standalone sequencer; and
- two-definition official-ATA claim/refund lifecycles with real owner keys,
  immutable destinations, and cross-definition substitution rejection; and
- machine-checked recursive native/authenticated-transfer and token/ATA/Token
  Risc0 session costs with setup and Clock noise excluded.

See the living [implementation plan](docs/implementation-plan.md), the
[whole-system actor and flow architecture](docs/architecture/system-architecture.md),
the [architecture decision log](docs/architecture/README.md), the living
[manual reproduction guide](docs/manual-user-flows.md), and the first
[acceptance tests](crates/swap-core/tests/e2e_swap_lifecycle.rs).

## Development

Prerequisites: Rust 1.96.0. Docker is needed for the isolated Zebra consensus
suite and the pinned Risc0 guest builder; Docker Compose v2 is used by Zebra.
The [manual reproduction guide](docs/manual-user-flows.md) lists the complete
per-run prerequisites, isolation rules, commands, expected evidence, and
cleanup behavior.

### External dependencies and flakiness

The current operator, Zebra, and LEZ flows use no public blockchain RPC or
faucet. All chain endpoints are ephemeral loopback services, and test funds are
deterministic local genesis/Regtest outputs. Cold builds still depend on
rustup/crates.io, locked GitHub sources, digest-pinned Docker Hub/GCR images,
the checksum-pinned Logos circuits release, and `rzup`'s pinned Risc0 tools.
Availability, DNS, proxy, registry throttling, or GitHub/CDN outages can block
an uncached run, but cannot relax the lockfile, digest, checksum, ELF, ImageID,
or consensus checks. Warm verified caches reduce this availability risk.

CI also refreshes RustSec and Trivy vulnerability data. A database outage may
block scanning; a newly published advisory may deliberately turn a prior pass
red. Do not bypass that failure as “flaky.” Public-testnet RPCs/faucets have not
yet been selected; their provider limits, health checks, fallback/self-hosted
routes, and funding assumptions are an explicit M2 documentation gate. See the
[full resource/flakiness table](docs/manual-user-flows.md#external-resources-and-flakiness).

    cargo test --locked --workspace --all-targets
    cargo fmt --all --check
    cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
    cargo deny check advisories bans licenses sources
    RUN_ID=local-zebra-1 ./scripts/run-zebra-e2e.sh

The Zebra suite uses a unique `lez-atomic-swaps-${RUN_ID}` Compose project. It
copies the binary from the digest-pinned official Zebra 5.2.0 image into a
digest-pinned distroless nonroot runtime, then runs two disconnected nodes on a
project-only network with read-only filesystems, independent tmpfs state,
resource caps, no Linux capabilities, and separate ephemeral localhost RPC
ports. Cleanup addresses that exact project and never prunes or stops resources
it did not create.

## Licensing

Licensed under either the Apache License, Version 2.0 or the MIT License, at
your option.
