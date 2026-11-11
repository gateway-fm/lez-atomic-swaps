# Provenance — the stack built from nothing and smoke-tested on an Apple-silicon Mac

- Host: Apple M1 Max (10 cores, 64 GB), macOS 26.7.3, Docker Desktop with
  engine 29.9.2 (10 CPUs and 31.5 GB given to its VM), Homebrew bash 5.3 and
  coreutils. Nothing of this branch had been run on arm64 or macOS before.
- Code: PR #120's head `c1d6f813` plus the three fix commits of this branch;
  the smoke tier ran on the committed tree at `6744d32b` (`regress.log`: "tree
  dirty: 0"). The fixes change only `deploy/scripts`, `deploy/tests` and two
  README paragraphs: no Rust, module, Dockerfile or Compose source differs from
  `c1d6f813`.
- Build: `deploy/scripts/from-scratch.sh --workspace <empty directory>` on
  2026-11-11, 17:28Z to 19:27Z, in a workspace that held no sources, provision
  data, market or identities. Two Docker volumes from older work on this host
  were present and reused as caches: `lez-nix-store-arm` (Nix store) and
  `lez-build-target` (Cargo targets). The run was restarted after each of the
  failures the fix commits describe, and the `rust` and `build` phases were
  run beside the `nix` phase to save time (`--only rust`, `--only build`), so
  the two hours are not the wall time of one uninterrupted run.
  `first-run/from-scratch-phases.log` lists the phases as they were logged.
- Images, all built in that run, all `linux/arm64`:

  | image | created (UTC) | platform |
  |---|---|---|
  | lez-services:local | 2026-11-11T19:12:35Z | linux/arm64 |
  | lez-bitcoin-core:local | 2026-11-11T19:12:33Z | linux/arm64 |
  | lez-zebra:local | 2026-11-11T19:12:33Z | linux/arm64 |
  | lez-btc-miner:local | 2026-11-11T19:12:37Z | linux/arm64 |
  | lez-basecamp-ui:local | 2026-11-11T19:13:17Z | linux/arm64 |
  | lez-network-module:local | 2026-11-11T19:13:19Z | linux/arm64 |
  | lez-maker-node:local | 2026-11-11T19:13:29Z | linux/arm64 |
  | lez-taker-node:local | 2026-11-11T19:13:30Z | linux/arm64 |
  | lez-wallet-module:local | 2026-11-11T19:25:37Z | linux/arm64 |

  Sidecar source tree: `1decb58b7e8c428c92067365758a9db073ac280b`. The escrow
  guest ELF was built under amd64 emulation and matched the pinned digest.
- Stack: the local devnet (LEZ v0.2.4, Bitcoin regtest, Zebra regtest), one
  lane. One LEZ chain served everything below; none was recreated.
- Contents: `RESULTS.txt` (every test that ran, in order), `results.tsv` and
  `logs/` (the smoke tier as `regress.sh` wrote them), `e2e/` and `zec/` (its
  scenario evidence), `first-run/` (the bring-up's own verification and the
  two desk swaps run before the tier). Host paths are replaced by `<repo>` and
  `<workspace>` and terminal colour codes are removed; nothing else is edited.
  Public chain data only; no keys, cookies or capability files.

## What this does not show

- No refund, concurrent, restart or finality-stall scenario ran, and no desk
  test other than the two named in `RESULTS.txt`: this is the smoke tier, not
  the regression.
- The release bundle path (`start.sh` pulling published images) did not run;
  no bundle exists for this commit.
- `up.sh --fresh-lez` and `down.sh --wipe` did not run on this host.
- The role packages are `linux-arm64` and ran in Docker Desktop. A native
  darwin-arm64 package (docs/submission-115.md, R14) is not built and not
  shown.
