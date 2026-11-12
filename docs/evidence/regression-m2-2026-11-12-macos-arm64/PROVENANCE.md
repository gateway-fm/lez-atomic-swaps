# Provenance — full regression on macOS arm64, 2026-11-12

Everything in this folder was produced on one Mac by the Docker devnet of
`deploy/`: every test of `scripts/regress.sh full` (93: `regress.sh list
full`), the Node scenarios by `scripts/node-e2e.py` and the desk journeys by
`scripts/ui-e2e.sh`, three at a time on three regression lanes. It is the
macOS counterpart of `docs/evidence/regression-m2-2026-11-11/` (Linux
x86_64).

Nothing here is private. The ZEC exports carry swap views, durable actor
status, zebra heights and transaction ids; the BTC summaries carry timing,
swap ids and claim transaction ids; the logs carry step lines and captions.
No key, cookie, capability file, `runtime/` or `market/` content is
included. Local paths in the logs are replaced by `<checkout>`,
`<workspace>`, `<home>` and `<scratch>`. Scanned before commit with the
repository's gitleaks configuration and for key, cookie, password and
preimage patterns.

## Source

Tests ran from `adffc106` (branch `docs/release-0.3.0-gates`, PR #127), tree
clean: `logs/regress.log` prints the commit and "tree dirty: 0" at its start.
`adffc106` is `main` at `af3ac978` plus seven commits that change only
`deploy/` scripts, one Dockerfile, the Compose file and `docs/`
(`git diff --stat af3ac978 adffc106`); crates, sidecar, desks and modules are
`main`'s.

## Images

| image | id |
|---|---|
| `lez-maker-node:local` | `sha256:a57e350769e865a5477f53848c65069ef3b0f61e67223b995bb8a2b76fa85d77` |
| `lez-taker-node:local` | `sha256:67553262903ad5b1b14210a894d06274e900f83bd9205a7f5eb3c1ea8251123e` |
| `lez-network-module:local` | `sha256:1cb8b50cb3a6dfdd393b55cc9521066476a33885992b6953e723387c9ddcdd8b` |
| `lez-services:local` (sequencer) | `sha256:3d27929115178d3d0b0539261385b71519abb7209d86b12fa87a0ac287f0881d` |
| `lez-zebra:local` (Zebra 7.0.0) | `sha256:45f432d2e46059e96efc610bd9b3126b12fce0ea5df92a1e0e68dd0935bba77b` |
| `lez-bitcoin-core:local` | `sha256:0eff51035e1f6508b2600be2fab559068d06957cdd49ebf0862694861aab9443` |
| `lez-btc-miner:local` | `sha256:db9152efc62ff95bbd3d4a09d9142be7026095cc98f879c30e9fce59f6c436c7` |

All are local `linux/arm64` builds of this host; nothing was pulled from a
registry and no release was built from this commit.

The payloads (Node binaries, actors, sidecar, Nix packages) were built and
staged from `af3ac978` by `from-scratch.sh` on 2026-11-11 at 20:57Z–21:02Z.
The images were built from them on 2026-11-12 at 01:43Z by `up.sh --build`.
The two Node images and the network image are new; the other four came out
of the build cache with the ids they had, their contents being the same.

Each Node image carries its build source in
`/usr/local/share/lez/build-source.txt`, and each installed Node package in
its `payload.json`. `logs/rebuild-summary.log` records both for the two
roles right after the image build, and `logs/run-summary.log` the build
source of all six Node containers (three lanes) before the first test:
`af3ac978` everywhere. The wrapper refuses to start the run otherwise.

## The run

- Host: MacBook Pro, Apple M1 Max, 10 cores, 64 GB; macOS 26.7.3; Docker
  Desktop, engine 29.9.2, 10 CPUs and 32 GB given to its VM. Nothing else ran
  on the stack; the host was otherwise idle.
- `lanes.sh down 3 --wipe`, then `up.sh --fresh-lez` (LEZ_LANES=3) at 01:48Z
  made the chain the run started on; `LEZ_REGRESS_RECORD=1 LEZ_LANES=3
  scripts/regress.sh full` ran from 01:50:00Z to 03:16:07Z (86 min).
- Lanes: lane 1 is the stack; lanes 2 and 3 are one more Maker/Taker pair
  each with their own Bitcoin Core, miner and Zebra, on the stack's one LEZ
  network. `results.tsv` names the lane of every test.
- LEZ chains: the starting chain served every test but two;
  `zec-balance-refusal` in each direction starts a chain of its own (the last
  two tests). No chain was replaced for a finality freeze: 0 frozen tries.
- Timing profiles: `fast` -> `refund` at 01:50Z, `refund` -> `fast` at 02:52Z.
- Swaps are reset between tests by `reset-swaps.sh --abandon-open` on the
  test's lane (`logs/regress.log`).

## The first run, and why it is not the result

A first full run ended at 00:47Z with 89 of 93. While its provenance was
being written the Node containers were found to report build source
`8bdf0749`, the tip of PR #124's branch before #120's last commits, although
the packages staged on the host were `af3ac978`'s.

Cause: `from-scratch.sh` started the stack through `up.sh` without `--build`,
and `up.sh` builds only when an image is missing. The host had images from
the `8bdf0749` build of 19:13Z, so the newly staged packages never reached
the stack. Fixed in `adffc106` (`from-scratch.sh` passes `--build`). It is
not specific to macOS.

What those Nodes lacked (`git diff 8bdf0749 af3ac978 -- crates modules`):
`crates/zec-swap-sdk` (`refund.rs`, `sdk.rs`, `lifecycle.rs`: a refund is
refused once a revealing claim is recorded) and `modules/lez-swap-node`
(`node_stats`), with the two roles' `.lidl` files.

The first run and its reruns are kept in `first-run/` for what they found in
the test harness (`RESULTS.txt`); its per-test logs and recordings were
removed by the wipe that preceded this run. `first-run/summary-run1.log`
names `18a440f1` and the runner's own log `9fba9da9`: a documentation commit
was made between the two lines.

The same stale images served `docs/evidence/macos-arm64-release-gates-2026-11-11/`;
that set says what was redone.

## Recordings

Every desk test was recorded on its lane's two Basecamp displays
(`ui-e2e.sh --record`); a recording is a by-product of the test that passed.
32 recordings were made; the two Bitcoin `node-offline` desk tests have no
recorded desk step and produced none. Seven are in `videos/`, the same ZEC
set as the Linux evidence: `zec-happy`, `zec-maker-refund` and
`zec-concurrent` in both directions and `zec-stale-window` selling ZEC. The
other 25 are kept on the host, not in git.

## Not covered by this set

- Any public network: see `docs/m2-acceptance-checklist.md`.
- A released bundle: the images are local builds. The release workflow's run
  on `main` (43106618999) built every image and failed in its smoke job on
  the `libpcsclite` fault that PR #127 fixes.
- macOS on Intel, and Linux arm64 outside Docker Desktop's VM.
- The Logos-native package acceptance on arm64:
  `docs/evidence/logos-native-2026-11-11-arm64/` (packages of `8bdf0749`).
