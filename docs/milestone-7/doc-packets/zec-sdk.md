# Journey: integrate the LEZ-ZEC SDK

<!-- logos-docs-template-commit: 63ecf397ca5dae4b81de85a578ec839a78fec1c0 -->

## What the user achieves

A Rust developer runs the complete transparent-Zcash SDK lifecycle through
agreement, taker-first locks, claim, ordered refunds, persistence and replay.

## Why it matters

Applications receive a typed BIP-199/LEZ lifecycle and canonical evidence
boundary instead of constructing scripts, transactions or recovery state ad hoc.

## Key components

- `lez-zec-swap-sdk`: agreement, HTLC transactions and durable lifecycle.
- `lez-zebra-node-adapter`: bounded canonical Zebra RPC observations.
- `zec-reference-actor`: one-shot role-fixed effect process.
- LEZ bridge adapter/sidecar: exact LEZ Vault preparation and finality facts.

## Repository

https://github.com/mandrigin/lez-atomic-swaps @ `main` (use the reviewed M7 candidate commit when published)

## Runtime target

local

## Prerequisites

Linux x86_64; Rust 1.96.0; Git; approximately 8 GB RAM and 20 GB free disk.
Docker is required for the isolated Zebra/LEZ actor journey.

## Commands and expected outputs

```sh
git clone https://github.com/mandrigin/lez-atomic-swaps.git
cd lez-atomic-swaps
cargo test --locked -p lez-zec-swap-sdk --all-targets
cargo test --locked -p zec-reference-actor --all-targets
```

The lifecycle, BIP-199 interpreter, transaction, canonical observation,
restart/replay and actor targets finish without failures. The ignored explicit
Zebra gate remains opt-in and is exercised by the isolated actual-node runner.

## Success command

`cargo test --locked -p lez-zec-swap-sdk --test sdk_lifecycle`

## Expected result

The complete deterministic role lifecycle target finishes with zero failures.

## Configuration details

Deterministic tests need no variables. Actual-node runs create run-specific
Zebra, LEZ, actor, wallet and state paths and dynamic literal-loopback ports.
Use `docs/manual-user-flows.md` Flow 0G or the service-driven Flow 1Y.

## Failure modes and limits

- Shielded addresses are unsupported; use transparent test destinations only.
- Wrong network/genesis, stale tips, deadline inversion or insufficient margin
  fail before a permitted effect.
- Public testnet funds/RPCs are not part of retained local evidence.

## GitHub point of contact

@mandrigin

## Discord point of contact

mandrigin.eth

## Existing docs or specs

ADRs 0014-0023, `docs/manual-user-flows.md` Flow 0G/Flow 2, and the canonical
system/deployment architecture documents.

## Hardware requirements

SDK-only: 2 CPU, 8 GB RAM, 20 GB disk. Local Zebra/LEZ composition: 4 CPU,
16 GB RAM and 100 GB temporary disk recommended.

## Estimated time to complete

10-20 minutes warm for SDK/actor tests; 30-120 minutes for cold local nodes.

## Security notes

Protect preimages, signing keys, exact signed transactions and recovery-store
keys. Check stable tips and containing blocks before branch authority; do not
retry unknown submissions with changed bytes. Transparent addresses expose the
accepted on-chain activity.

