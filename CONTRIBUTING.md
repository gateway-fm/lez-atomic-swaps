# Contributing

This is security-sensitive cross-chain software. Every behavioral change starts
with a failing test that describes the affected maker, taker, operator, or
recovery journey. Keep `docs/implementation-plan.md` and the requirements
traceability matrix current in the same change.

Before opening a pull request, run:

    cargo fmt --all --check
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    cargo test --workspace --all-targets
    npm run test:mermaid
    ./scripts/check-requirements-traceability.sh
    ./scripts/check-architecture-diagrams.sh

Architecture choices that affect protocol safety, persistence, cryptographic
primitives, external interfaces, or operations require an ADR in
`docs/architecture/`. Never add a cryptographic construction without published
test vectors and an independent review plan. Every ADR and Milestone 1 design
document includes an up-to-date Mermaid component or flow diagram; update the
diagram in the same commit whenever the described architecture changes. Every
tracked Mermaid block must pass the conservative GitHub-host compatibility
policy and pinned renderer; do not add host directives, beta/new-shape syntax,
or interactive links and callbacks.

Do not commit keys, wallet seeds, testnet credentials, chain data, generated
proofs, or Docker volumes.
