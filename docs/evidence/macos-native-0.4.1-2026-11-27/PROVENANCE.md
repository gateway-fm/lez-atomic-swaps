# Provenance: native macOS role Node, 0.4.1 candidate

The evidence in this directory is about what 0.4.1 ships. Everything it ran came from the candidate builds of `main` at `2373e28735b732d2183bb4ce2b494c95ff8d42a1`, the merge of PR #204.

## Chains and the Docker role: candidate release bundle

- **Bundle:** `lez-swap-stack-sha-2373e28735b7-arm64.tar.gz`, artifact `lez-swap-stack-bundle` of GitHub run 45887200123 (`release-images.yml` on `main`).
  - sha256 `64cc333eff1d3702b9db9feb9843201955e3eebec430a9a1c5972d2150c2a31d`, which matches the `.sha256` file shipped with it.
  - `release.env`: `LEZ_IMAGE_TAG=sha-2373e28735b7`, `LEZ_RELEASE_COMMIT=2373e28735b732d2183bb4ce2b494c95ff8d42a1`.
- **Started:** `LEZ_STACK_PREFIX=lez041n LEZ_MONERO=1 LEZ_TIMING_PROFILE=fast ./scripts/start.sh` (log: `logs/B-start-lez041n.log`).
- **Role images:** build source read from each image's `/usr/local/share/lez/build-source.txt`.

| Image | Build source | Digest |
|---|---|---|
| `lez-maker-node:sha-2373e28735b7` | `2373e28735b732d2183bb4ce2b494c95ff8d42a1` | `sha256:5ae98bfb80d5afa39bf7cb03390306975148cb9458fa7b86bb2817d00b300480` |
| `lez-taker-node:sha-2373e28735b7` | `2373e28735b732d2183bb4ce2b494c95ff8d42a1` | `sha256:8504d90c08ace6813485555862b54948230c2b647bee4b054fe62da95fca6885` |
| `lez-monerod` | Monero `v0.18.5.1` | `sha256:fdb240113fc3a795e820f0f13eaac1b331a99044bab4c811d6b623d7b1c66e3d` |
| `lez-services` (sequencer) | | `sha256:3537c5d982c4b717036853ccf693fb72a22ec55477fa745d1bee6e800a6c0501` |
| `lez-network-module` | | `sha256:b38573e7e8e67bc232f87a87051759b18c47d38df3f2cff0552ca3b66701bf21` |
| `lez-wallet-module` | | `sha256:febffef6a858742ac5f44e4908e4d86bb8b1eba70f200be555341e17f1b29587` |
| `lez-bitcoin-core` | | `sha256:d8891a6877d36dd0894d37d65b419ef53080feb3b93701dafe2f7f37c62e52e3` |
| `lez-zebra-module` | | `sha256:58637277db21be5c4835c772d30155c5b8d351c2fb44ed64402ecfa898cf8941` |
| `lez-btc-miner` | | `sha256:14036fa2ea034e9372897f8449def56a763bbf9f81bf9411ffde98eb2e5e2a60` |

## The native role: candidate catalog packages

- **Source:** artifact `catalog-packages-test-2373e28735b732d2183bb4ce2b494c95ff8d42a1` of GitHub run 45887203856 (`release-packages.yml` on `main`). Its darwin-arm64 variants were built on a macos-15 runner, then merged, pinned and verified.
- **Signature:** this test artifact is not signed, so `logosctl` reports each package as `"signature":"unsigned"`. Only the release tag's packages are signed.

| Package | Version | sha256 | Variants |
|---|---|---|---|
| `lez_maker_node` | 0.4.1 | `900e4ada819f633379978be69525df8abef6a4687f855294f2b7e0e006bf7d96` | darwin-arm64, linux-amd64, linux-arm64 |
| `lez_taker_node` | 0.4.1 | `ce80f6b1e532a4862243888d885ed34e431b982ad12386415ec72b4ccbc3d4f3` | darwin-arm64, linux-amd64, linux-arm64 |
| `lez_swap_node` | 0.4.1 | `9ca47a9ac6ce3df2af4f546732e1f3a1dcbf11b548e3c6f026ef1ca936dc6e86` | darwin-arm64, linux-amd64, linux-arm64 |
| `lez_core` | 0.4.1-sign.4 | `af28f4b5299392aae9c0d26a1d6bc22d5e764b53d5e283b23a1eac8c1f2c3b48` | darwin-arm64, linux-amd64, linux-arm64 |

- **The role packages' darwin-arm64 variant:** `bin/payload.json` reads `build_source` and `sidecar_source` `2373e28735b732d2183bb4ce2b494c95ff8d42a1`, `variant` `darwin-arm64`.
  - The Maker's variant carries `lez-xmr-maker-actor`, `lez-v02-xmr-stage-a-plan` and the six Monero effect tools: `xmr-reference-monero-fund`, `-monero-verify`, `-monero-refund`, `-tag15`, `-tag15-finalized` and `-tag17`.
  - The Taker's variant carries `lez-v02-xmr-stage-a-plan`.
- **Logos host:** `logosctl` 0.3.2 for macOS (aarch64), commit `96e704df1a8140f069bbf6d5a1cdeab8886c6e41`. Its daemon ran with `PATH=/usr/bin:/bin:/usr/sbin:/sbin`, so the launcher ran under macOS's `/bin/bash` 3.2.

## Monero on the Mac: official 0.18.5.1 release for macOS

- **Archive:** `monero-mac-armv8-v0.18.5.1.tar.bz2` from `downloads.getmonero.org/cli/`, 70 224 787 bytes, sha256 `dba08921841e675384ce019fd7c93b59fe7b1e6edaa0a3cf0e3253e263f61864`.
- **How it was checked:** the way `scripts/verify-monero-release.sh` checks the Linux archives, with the pins in `tests/e2e/monero/provenance.env` (script: `scripts/verify-monero-mac.sh`, log: `logs/monero-mac-verify.log`).
  - The repository's snapshot of the signed `hashes.txt` is at its pinned sha256 `a6a7afab…2afb`.
  - The signing key snapshot is at its pin, with fingerprint `81AC591FE9C4B65C5806AFC3F0AF4D462A0BDF92`.
  - gpg gives a VALIDSIG by that fingerprint in a throwaway keyring.
  - The archive's sha256 is the one on the manifest's `monero-mac-armv8-v0.18.5.1.tar.bz2` line. That line is in the repository's snapshot, but `provenance.env` pins no macOS archive.
- **Kept from the archive:** only `monerod` and `monero-wallet-rpc`.
  - `monero-wallet-rpc` sha256 `4770eae94217cfde90628d607332c45a78a9dfad95a406e73adb7ec3efe17af0`, a Mach-O arm64 executable, `v0.18.5.1-release`. This is the binary that ran.
  - `monerod` sha256 `99f861ae7bc5b48fe032e3c9eec4ddb016cd447b43794eca9c4168c8beb8e4eb`. It never ran: the chain is the stack's `lez-monerod` container.

## Harness

The stack's own `scripts/node-e2e.py` and `scripts/monero-lane.py` from the candidate bundle ran unchanged. Two wrappers (`scripts/mixed-e2e.py` and `scripts/xmr-lane-mixed.py`) load them as modules for the one layout neither knows: one role native, the other in its container (`README.md`, "Setup").
