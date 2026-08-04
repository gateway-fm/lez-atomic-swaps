# Journey: integrate the LEZ-BTC SDK

<!-- logos-docs-template-commit: 63ecf397ca5dae4b81de85a578ec839a78fec1c0 -->

## What the user achieves

A Rust developer validates and runs the durable LEZ-BTC pair lifecycle and can
embed its typed agreement, Taproot, adaptor, claim and refund boundaries.

## Why it matters

The SDK keeps protocol bytes and branch authority out of application code while
exposing complete, role-safe integration points.

## Key components

- `lez-btc-swap-sdk`: agreement, Taproot/MuSig2 adaptor and lifecycle facade.
- `lez-swap-sdk-core`: shared bounded discovery and negotiation vocabulary.
- `lez-btc-core-adapter`: authenticated canonical Bitcoin Core observations.
- LEZ bridge adapter/sidecar: version-pinned LEZ effect preparation and facts.

## Repository

https://github.com/mandrigin/lez-atomic-swaps @ `main` (use the reviewed M7 candidate commit when published)

## Runtime target

local

## Prerequisites

Linux x86_64; Rust 1.96.0 with rustfmt/clippy; Git; approximately 8 GB RAM and
20 GB free disk. Docker is needed only for the actual-node journey.

## Commands and expected outputs

```sh
git clone https://github.com/mandrigin/lez-atomic-swaps.git
cd lez-atomic-swaps
cargo test --locked -p lez-btc-swap-sdk --all-targets
cargo run --locked -p lez-btc-swap-sdk --example durable-lifecycle
```

Tests finish without failures. The example compiles the public durable
lifecycle wiring helpers and prints the integration instruction; the
`sdk_facade` target executes agreement, lock, claim/refund, restart and replay
boundaries without public network access.

## Success command

`cargo test --locked -p lez-btc-swap-sdk --test sdk_facade`

## Expected result

The `sdk_facade` integration target finishes with zero failed tests.

## Configuration details

The deterministic SDK example needs no environment variables. Actual-node
composition uses run-scoped paths and dynamically allocated literal-loopback
RPCs documented in `docs/manual-user-flows.md`; never hard-code retained-run
ports or share Maker/Taker stores.

## Failure modes and limits

- A Rust/toolchain mismatch fails the locked build; install exactly 1.96.0.
- Actual-node cold setup may wait on immutable image/release sources; retry only
  setup, not an ambiguous chain effect.
- Local evidence is Regtest/private LEZ evidence, not Testnet4 or mainnet proof.

## GitHub point of contact

@mandrigin

## Discord point of contact

mandrigin.eth

## Existing docs or specs

`docs/architecture/system-architecture.md`, ADRs 0009/0029/0050,
`docs/m3-local-poc-operator-guide.md`, and `docs/manual-user-flows.md`.

## Hardware requirements

SDK-only: 2 CPU, 8 GB RAM, 20 GB disk. Actual local nodes: 4 CPU, 16 GB RAM,
80 GB temporary disk recommended.

## Estimated time to complete

5-10 minutes warm for SDK-only; 20-60 minutes for cold actual-node setup.

## Security notes

Use fresh nonce material once, protect role journals and keys, authenticate
RPCs, validate network/genesis identities, and never change bytes when retrying
an ambiguous submission. Formal composition review remains required.
