# Contributing

This is security-sensitive cross-chain software. Every behavioral change starts
with a failing test that describes the affected maker, taker, operator, or
recovery journey. Keep `docs/implementation-plan.md` and the requirements
traceability matrix current in the same change.

Before opening a pull request, run:

    cargo fmt --all --check
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    cargo test --workspace --all-targets

Architecture choices that affect protocol safety, persistence, cryptographic
primitives, external interfaces, or operations require an ADR in
`docs/architecture/`. Never add a cryptographic construction without published
test vectors and an independent review plan.

Do not commit keys, wallet seeds, testnet credentials, chain data, generated
proofs, or Docker volumes.

