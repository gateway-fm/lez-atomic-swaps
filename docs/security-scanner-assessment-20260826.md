# Security scanner assessment — 2026-08-26

This assessment covers the Aikido findings raised when the direct source tree
was first published. It records code-path verification; it is not a waiver for
future production use or a replacement for rescanning after dependency changes.

## Fixed findings

- The three new workflows now grant only `contents: read` instead of
  `permissions: read-all`.
- Every `actions/checkout` invocation sets `persist-credentials: false`.
- A checksum-pinned ripgrep bootstrap makes repository checks independent of
  GitHub runner image package drift.
- Gitleaks now has path-and-line-shape allowlists for reviewed deterministic
  local fixtures and public cryptographic identifiers. The default rules remain
  enabled, so another value shape or location is still reported.
- The public tree now includes the evidence packet required by its CI contract.

## False positives

### Path traversal

The reported Rust paths are local command-line destinations or inputs, not
remote request parameters:

- `compat/lez-standalone-e2e/src/lib.rs` creates a random same-directory
  mode-0600 temporary and publishes it with `persist_noclobber`.
- `compat/lez-v0_2-sidecar/src/bin/lez-v02-bridge-poc.rs` and
  `lez-v02-native-escrow-poc.rs` accept operator-selected local paths and
  validate file type, size, permissions, link count, and/or the 0700 state
  directory before use.

The binaries are neither setuid nor a remote file-serving boundary. A caller
who can choose these CLI arguments already has the same filesystem authority as
the process. Reassess this classification if any path is later accepted from a
remote peer or a more privileged service account.

### Exposed secrets

The reported values are deterministic Regtest-only keys, public
Taproot/program identifiers, SHA-256 digests, or deliberately invalid fixture
run IDs. They are not production credentials and the evidence files explicitly
exclude private run material. Exact Gitleaks allowlists live in
`.gitleaks.toml`; the full reachable-history scan passes with the default
rules enabled.

### JavaScript file inclusion

`scripts/check-m6-basecamp-package-contract.mjs` has no user-controlled path
input. It derives the repository root from `import.meta.url` and reads a fixed
set of repository-tracked files for a build contract check.

### `r-efi` licence

`r-efi` offers `MIT OR Apache-2.0 OR LGPL-2.1-or-later`; this project uses
the permissive option. The deployment cargo-deny graphs also target Linux GNU,
where the EFI-only dependency is not compiled. No LGPL-only artefact is shipped.

## Present upstream dependencies with unreachable PoC paths

These are accurate inventory findings, not secret or code-scanning false
positives. They are constrained upstream risks rather than exploitable paths in
this release:

- `hickory-proto 0.25.0-alpha.5` is forced by the pinned LEZ v0.2.0
  Logos/libp2p graph. The high advisory requires DNSSEC features, which are not
  enabled; the medium encoding path is not invoked because the sidecar uses
  explicit loopback HTTP RPC and does not instantiate the transitive libp2p
  service. See [GHSA-3v94-mw7p-v465](https://github.com/advisories/GHSA-3v94-mw7p-v465)
  and [GHSA-q2qq-hmj6-3wpp](https://github.com/advisories/GHSA-q2qq-hmj6-3wpp).
- `lru 0.12.5` is forced by that graph through `libp2p-swarm`. Repository
  code does not call `lru::IterMut` or start the transitive network service.
  See [GHSA-rhfx-m35p-ff5j](https://github.com/advisories/GHSA-rhfx-m35p-ff5j).

Do not use these exceptions for a production LEZ v0.2 network-service path.
Updating the exact upstream LEZ/Logos dependency set is required before enabling
DNS/libp2p execution; a local semver-incompatible override would invalidate the
compatibility evidence.

## Reproduction

```sh
./scripts/test-ci-hardening-policy.sh
./scripts/check-github-action-pins.sh
gitleaks git . --no-banner --redact --exit-code 1
```
