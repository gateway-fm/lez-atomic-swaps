# Contributing

Thank you for helping improve LEZ ⇄ Bitcoin. This is security-sensitive
cross-chain software: describe the affected Maker, Taker, operator, or recovery
journey and add a regression test for every behavioural change.

## Repository topology

The default `main` branch contains the complete buildable source tree. Rust
workspace manifests, crates, compatibility packages, Basecamp applications,
tests, and build scripts are all checked in at their conventional paths. A
fresh clone can inspect and build the implementation directly:

```sh
cargo metadata --locked --no-deps
./scripts/run-public-offline-e2e.sh
```

Implementation changes must edit this tree normally and include tests and
documentation in the same pull request. Do not submit generated build outputs,
private evidence roots, or a patch-only substitute for the changed source.
The owner-process hardening uses Linux-only APIs; on macOS, use the isolated
Docker wrapper above instead of expecting the complete workspace to compile
natively.

## Developer Certificate of Origin

All new contributions must certify the
[Developer Certificate of Origin 1.1](DCO). Add a sign-off with Git:

```sh
git commit --signoff
```

The sign-off certifies that you have the right to submit the contribution under
this repository's licence. See https://developercertificate.org/. Commits must
also be cryptographically signed once protected-branch enforcement is enabled.
Automation must use an approved, attributable bot identity capable of the same
signing and DCO checks; ad-hoc local machine identities are not accepted for
new contributions.

## Before opening a pull request

From the repository root, run:

```sh
./scripts/verify-public-repository.sh
./scripts/run-public-offline-e2e.sh
```

The offline E2E wrapper uses a cached Rust image and dependency registry in an
auto-removed Docker container with no network. It never starts, stops, or joins
the demo Compose stack.

Also run the relevant formatting, Clippy, unit, integration, documentation,
dependency, and architecture checks. Never commit keys, wallet seeds, testnet
credentials, chain data, generated proofs, Docker volumes, or private evidence
roots.

Architecture changes affecting protocol safety, persistence, cryptography,
external interfaces, or operations require an ADR. If AI-assisted tooling was
material to a contribution, disclose that in the pull request and review the
result for ownership, licence provenance, correctness, and security exactly as
you would handwritten code.

## Review

Pull requests require Gateway code-owner and security review. Use neutral,
public-safe branch names, commit messages, test fixtures, and discussion. Send
suspected vulnerabilities through the private process in SECURITY.md.
