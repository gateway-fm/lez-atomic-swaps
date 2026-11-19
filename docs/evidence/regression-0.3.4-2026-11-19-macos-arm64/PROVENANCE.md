# Provenance — full regression of `3b73e65b` (0.3.4) on macOS arm64, 2026-11-19

Everything in this folder was produced on one Mac by the Docker devnet of
`deploy/`: every test of `scripts/regress.sh full` (98: `regress.sh list
full`), the Node scenarios by `scripts/node-e2e.py` and the desk journeys by
`scripts/ui-e2e.sh`, three at a time on three regression lanes; and, before
it, the first runs of the new scenario `zec-maker-early-claim`, once per
lane. One operator ran both parties, Maker and Taker, on one host. The tested
commit is `3b73e65b` on `fix/zec-claim-recovery-0.3.4`, the release
candidate of 0.3.4.

## The count

- The full run: 98 tests, 98 passed, each on its first try, in one run of
  115 minutes. No test was rerun.
- Before it, on the chain the full run then started on:
  `node:zec:zec-maker-early-claim:TakerSellsLez` once on each lane, 3 of 3
  passed. These are the scenario's first runs on any stack; they are not
  part of the 98 and are kept apart in `early-claim-first-runs/`.

## Where the `zec-maker-early-claim` output is

The complete output of the scenario (the listing of every file the search
read, the result for each form of the preimage and the digest, the control
search over the Taker's files, both claim attempts with the probe's JSON
before and after, the escrow's state, the Taker's refund and both Nodes'
terminal states):

- `early-claim-first-runs/node-zec-zec-maker-early-claim-TakerSellsLez-lane1.log`,
  `-lane2.log` and `-lane3.log`: the three first runs (14:28Z to 14:46Z),
  with each run's summary in `early-claim-first-runs/lane<k>/zec-maker-early-claim.json`;
- `logs/node-zec-zec-maker-early-claim-TakerSellsLez-1.log`: its run in the
  full run (lane 3, 16:09Z to 16:17Z), with its summary in
  `e2e/zec-maker-early-claim.json`.

`RESULTS.txt` says what each of them shows. The swap's preimage is never
printed: the harness reads it from the Taker's file and logs only its
SHA-256.

## Privacy

Nothing here is private. The ZEC exports carry swap views, durable actor
status, zebra heights and transaction ids; the BTC summaries carry timing,
swap ids and claim transaction ids; the logs carry step lines, captions, the
stack's start-up lines and, for `zec-maker-early-claim`, the paths and sizes
of the files it searched and the probe's account ids and balances. No key,
cookie, capability file, `runtime/` or `market/` content is included. Local
paths in the logs are replaced by `<checkout>`, `<workspace>`, `<home>` and
`<scratch>`. Scanned before commit for local paths, user and host names, and
key, cookie, password and preimage patterns; the one password in the logs is
the devnet's documented VNC password (`deploy/README.md`).

## Source

Tests ran from a detached worktree of `3b73e65b` that nothing else wrote to,
tree clean, submodules at the commit's pins: `logs/regress.log` prints the
commit and "tree dirty: 0" at its start, and `logs/run-summary.log` the same
before anything was torn down.

When this folder was prepared, `3b73e65b` was the head of the branch: no
commit lies between the tested commit and the docs-only commit that adds
this folder.

What `3b73e65b` changes against `fab36d52`, the commit of
`docs/evidence/regression-0.3.3-2026-11-19-macos-arm64/`: the merge of PR
#168 (that branch) and of PRs #166, #167, #169 and #170 to `main`; the
version 0.3.4; a Maker LEZ claim whose nonce keeps being taken is re-planned
while it can land (#172); a Maker's Zcash refund after a lost LEZ claim ends
the swap refunded (#178); the devnet scenario `zec-maker-early-claim` with
the test-only tool `lez-v02-escrow-claim-probe`, which the Maker image
carries at `/usr/local/bin/` and no role package contains; `check-all.sh`
and CI changes.

## Packages

`logs/from-scratch.log` prints the version of every package it staged. This
project's components read 0.3.4: `lez_maker_node` and `lez_taker_node`
(packaged `linux-arm64` for the Node images and for the Basecamp image),
`lez_atomic_swap_maker`, `lez_atomic_swap_taker`, `lez_network` and
`lez_swap_node`. `logs/run-summary.log` records that the Node package
installed in each of the six Node containers reads 0.3.4. Upstream
components keep their own versions: `lez_core` 0.4.1-sign.4,
`blockchain_module` 0.2.4, `lez_indexer_module` 1.1.1-rpc.4, `chat_module`
0.2.2, `delivery_module` 0.2.0.

## Images

| image | id | this run |
|---|---|---|
| `lez-maker-node:local` | `sha256:76262ae6629372ce7a3fef28621e16813363bd780e6bc32aa133ba1f6c23bdfb` | built, 2026-11-19 14:16Z |
| `lez-taker-node:local` | `sha256:4a46cc52cb6d4f56ee2ab94d73019d18306547f0132c3e63e360db2adb0957a3` | built, 2026-11-19 14:16Z |
| `lez-network-module:local` | `sha256:12f8c126c794c5161cdbbe7e17da14078f22b96e4a558e972ff0edd3beda0664` | built, 2026-11-19 14:16Z |
| `lez-basecamp-ui:local` | `sha256:108cde52fcf901cf8b71f3dc0b130b947c8a6090492c7fde3323b8fbff0ef153` | cache (built 13:39Z from the same inputs) |
| `lez-services:local` (sequencer) | `sha256:f12005f4ce46374689d5a247e75a178db961be1fd460eeaf2958238b8056c0ee` | cache (2026-11-18) |
| `lez-zebra:local` (Zebra 7.0.0) | `sha256:de894d2d6a2ae38d9b6c0d98fe8360127240aa23597fd97fefcac48d39fe5736` | cache (2026-11-18) |
| `lez-bitcoin-core:local` | `sha256:39e939eabe1cc7024b140b36464587f72e8b97d528162cf3849cb1479d653691` | cache (2026-11-18) |
| `lez-btc-miner:local` | `sha256:c00b0cecc587f01984d7f27527644c10619cec42c9674b7f40d201fba0e5d379` | cache (2026-11-18) |

Ids and creation times are the ones `logs/run-summary.log` records after
`from-scratch.sh`. The Basecamp image came from the build cache: it was built
at 13:39Z in an earlier build of this branch's previous candidate, which
stopped at the Maker image, and its inputs as staged from `3b73e65b` were
the same. All are local `linux/arm64` builds of this host; nothing was pulled
from a registry, no proxy was set for the build, and no release was built
from this commit. `from-scratch.sh --workspace <workspace>` ran from
14:12:40Z to 14:19:21Z and staged every payload from `3b73e65b`.

The Maker image carries `lez-v02-escrow-claim-probe` (14.5 MB) at
`/usr/local/bin/`, checked in the image and in all three Maker containers
(`logs/run-summary.log`); the image is 14.6 MB larger than the `fab36d52`
Maker image.

Each Node image carries its build source in
`/usr/local/share/lez/build-source.txt`. `logs/run-summary.log` records the
build source of both images right after the build, and the build source,
installed package source and package version of all six Node containers
(three lanes) before the smoke: `3b73e65b3773d94388e91c73e657741935207489`
and 0.3.4 everywhere. The wrapper stops when any of them reports another
commit or version. Before the smoke it also checked that no `lez*` container
carries a proxy variable and that the stack's timing profile was `fast`, and
it checked both again before the full run.

## The run

- Host: MacBook Pro, Apple M1 Max, 10 cores, 64 GB; macOS 26.7.3; Docker
  Desktop, engine 29.9.2, 10 CPUs and 32 GB given to its VM. The same host
  as the earlier sets. Other work ran on the host during the run (a test
  suite of another checkout in its first part); the host's 1-minute load
  average ran from 3 to 85, median 20. No test failed.
- In order (`logs/run-summary.log`): `lanes.sh down 3 --wipe` and `down.sh
  --wipe` (14:12Z); `from-scratch.sh` (14:12Z to 14:19Z); `lanes.sh down 3
  --wipe` and `lanes.sh mint 3`; `up.sh --fresh-lez` with LEZ_LANES=3
  (`logs/fresh-lez.log`, done at 14:22Z), which made the chain the first
  runs and the full run ran on; `regress.sh smoke`, 6 of 6
  (`logs/smoke-regress.log`, `logs/smoke-results.tsv`); the three first runs
  of `zec-maker-early-claim` (14:28Z to 14:46Z), each after the lane's swaps
  were reset with `reset-swaps.sh --abandon-open` as the runner does; then
  `LEZ_REGRESS_RECORD=1 LEZ_LANES=3 scripts/regress.sh full` from 14:47:25Z
  to 16:42:40Z (115 min).
- Lanes: lane 1 is the stack; lanes 2 and 3 are one more Maker/Taker pair
  each with their own Bitcoin Core, miner and Zebra, on the stack's one LEZ
  network. `results.tsv` names the lane of every test (33, 30 and 33 on
  lanes 1, 2 and 3; the two balance-refusal tests ran on lane 1 alone).
- LEZ chains: the starting chain served every test but two;
  `zec-balance-refusal` in each direction starts a chain of its own (the
  last two tests, `logs/chain-1.log` and `logs/chain-2.log`). No chain was
  replaced for a finality freeze: 0 frozen tries. `chain.tsv` samples the
  finalized height and the tip every 30 s; the widest gap between them was
  24 blocks on the starting chain and 19 on the balance-refusal chains.
- Timing profiles: `fast` from the start, `fast` -> `refund` at 16:17:58Z
  (`logs/profile-1.log`); the run ended on `refund`.
- `e2e/` holds the summaries the runner keeps, which are lane 1's; the
  `zec-maker-early-claim` summary there is the full run's, copied from
  lane 3, where that test ran.
- Before each test that does not start a chain of its own, the runner
  resets the lane's swaps with `reset-swaps.sh --abandon-open`; those
  per-lane reset logs are not in this set.
- The per-test logs (`logs/node-*.log`, `logs/ui-*.log` and the first runs'
  logs) print the host's local time (CET, UTC+1). The runner's and the
  wrapper's logs, the start-up step lines, `results.tsv` and `chain.tsv` are
  in UTC.

## Recordings

Every desk test was recorded on its lane's two Basecamp displays
(`ui-e2e.sh --record`); a recording is a by-product of the test that passed.
32 recordings were made; the two Bitcoin `node-offline` desk tests have no
recorded desk step and produced none. Seven are in `videos/`, the same ZEC
set as the other sets: `zec-happy`, `zec-maker-refund` and `zec-concurrent`
in both directions and `zec-stale-window` selling ZEC. All 32 are kept on the
host, not in git; `videos-kept-outside-git.sha256` lists them, these seven
among them.

## Not covered by this set

- Two operators on two hosts: one operator ran both parties on one host.
- Latency to the chains: the chains ran on the same host as the Nodes.
- Any public network: see `docs/m2-acceptance-checklist.md`.
- Linux x86_64: no regression of this commit ran there.
- A released bundle or the package install path: the images are local
  builds, and the Nodes and desks ran from packages staged into the images,
  not installed from a release bundle or a module catalog.
- macOS on Intel, and Linux arm64 outside Docker Desktop's VM.
- Native darwin Nodes: the Nodes ran as `linux/arm64` containers in Docker
  Desktop's VM.
