# Journey: integrate the LEZ-XMR SDK

<!-- logos-docs-template-commit: 63ecf397ca5dae4b81de85a578ec839a78fec1c0 -->

## What the user achieves

A Rust developer validates the LEZ-first Monero agreement, cross-curve DLEQ,
spend-key-share and role-actor integration used by the local swap corridor.

## Why it matters

The journey exposes the XMR construction's distinct share-release authority
without pretending Monero has a script or accepting unsupported XMR-first flow.

## Key components

- `lez-xmr-swap-sdk`: agreement, DLEQ and shared-spend primitives.
- `xmr-reference-actor`: role-fixed Stage A/B and terminal actor boundary.
- `lez-xmr-monero-adapter`: bounded authenticated daemon/wallet observations.
- LEZ v0.2 sidecar: finalized witnessed-claim/refund evidence and preparation.

## Repository

https://github.com/mandrigin/lez-atomic-swaps @ `main` (use the reviewed M7 candidate commit when published)

## Runtime target

local

## Prerequisites

Linux x86_64; Rust 1.96.0; Git. Docker and the checksum-verified Monero release
are required only for the isolated actual Regtest corridor.

## Commands and expected outputs

```sh
git clone https://github.com/mandrigin/lez-atomic-swaps.git
cd lez-atomic-swaps
cargo test --locked -p lez-xmr-swap-sdk --all-targets
cargo test --locked -p xmr-reference-actor --all-targets
./scripts/test-m5-xmr-application-poc-contract.sh
```

All targets pass, including negative DLEQ/agreement/share cases, role ownership
and the sealed application-corridor contract. No public Stagenet endpoint is
contacted by these commands.

## Success command

`cargo test --locked -p lez-xmr-swap-sdk --all-targets`

## Expected result

The SDK unit, integration and `dleq-spike` example targets finish with zero
failed tests.

## Configuration details

The SDK checks need no runtime configuration. The actual corridor allocates
run-local authenticated daemon and wallet RPCs, LEZ ports, wallet directories,
actor journals and Docker names; use Flow 1R/1W in the manual guide.

## Failure modes and limits

- XMR-first agreements are rejected by design; select LEZ-first.
- Cold release/image setup can fail on immutable-source availability; verify
  checksums and do not fall back to an unpinned binary.
- Regtest timing/topology is deterministic local evidence, not Stagenet parity.

## GitHub point of contact

@mandrigin

## Discord point of contact

mandrigin.eth

## Existing docs or specs

ADRs 0053-0055, 0121-0123 and 0126; `docs/manual-user-flows.md` Flow 0 and
Flows 1R-1W; `docs/upstream-production-blockers.md`.

## Hardware requirements

SDK-only: 2 CPU, 8 GB RAM, 20 GB disk. Full local LEZ/Monero run: 4 CPU,
16 GB RAM and 100 GB temporary disk recommended.

## Estimated time to complete

5-15 minutes warm for component checks; 30-120 minutes for a cold node corridor.

## Security notes

Treat shares, wallet seeds, DLEQ nonces, Stage-B records and recovery journals
as secrets. Keep distinct authenticated role wallets and never expose wallet RPC
outside literal loopback. Independent cryptographic review is mandatory.

