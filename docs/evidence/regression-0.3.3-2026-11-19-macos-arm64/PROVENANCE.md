# Provenance — full regression of `fab36d52` on macOS arm64, 2026-11-19

Everything in this folder was produced on one Mac by the Docker devnet of
`deploy/`: every test of `scripts/regress.sh full` (97: `regress.sh list
full`), the Node scenarios by `scripts/node-e2e.py` and the desk journeys by
`scripts/ui-e2e.sh`, three at a time on three regression lanes, and a rerun
of three of them. The tested commit is `fab36d52` on
`zec/taker-sells-lez-order`: ZEC selling LEZ in the Taker-reveals-first
order, the new ZEC timing profiles and the Maker's LEZ claim queue, version
0.3.3. The run on `0a42823f` (the fix for #156) is
`docs/evidence/regression-0.3.3-2026-11-16-macos-arm64/`, and is the
comparison in `RESULTS.txt`.

This folder is committed on top of `fab36d52` in a docs-only commit. When it
was prepared, `fab36d52` was the head of the branch: no commit lies between
the tested commit and the commit that adds this folder.

Nothing here is private. The ZEC exports carry swap views, durable actor
status, zebra heights and transaction ids; the BTC summaries carry timing,
swap ids and claim transaction ids; the logs carry step lines, captions and
the stack's start-up lines; `taker-hang/` carries one container's log, state
and thread list. No key, cookie, capability file, `runtime/` or `market/`
content is included. Local paths in the logs are replaced by `<checkout>`,
`<workspace>`, `<home>` and `<scratch>`. Scanned before commit for local
paths, user and host names, and key, cookie, password and preimage patterns;
the one password in the logs is the devnet's documented VNC password
(`deploy/README.md`).

## Source

Tests ran from a detached worktree of `fab36d52` that nothing else wrote to
during the run, tree clean, submodules at the commit's pins:
`logs/regress.log` prints the commit and "tree dirty: 0" at its start, and
`logs/run-summary.log` the same before anything was torn down. The rerun ran
from the same worktree.

What `fab36d52` changes against `0a42823f` (52 first-parent commits; the
merges of PRs #157 and #160 among them):

- ZEC follows the common flow in both directions: selling LEZ, the Taker
  now reveals the secret by claiming the Zcash first and the Maker follows on
  LEZ (`swap-core`, `zec-swap-sdk`, `zec-reference-actor`,
  `zebra-node-adapter`, ADR 0216).
- New ZEC profiles: each profile admits only the direction it was sized
  for; `deterministic-local-devnet-v8` has a LEZ refund delay of 480 s.
- The Maker Node's LEZ claims run in refund order with a capacity and a
  bounded admission; a Maker whose LEZ claim was lost refunds its Zcash
  (`maker-node`, `lez-bridge-adapter`, the actor).
- The Taker Node refunds its LEZ when its Zcash claim was lost to the
  Maker's refund (`taker-node`).
- The harness: ZEC TakerSellsLez provisioning and scenarios in the new order
  (`deploy/scripts/node-e2e.py`, `ui-e2e.sh`, `zec-provision.sh`).
- The explorer images on Node.js 24.22.3.

## Packages

`logs/from-scratch.log` prints the version of every package it staged. This
project's components read 0.3.3: `lez_maker_node` and `lez_taker_node`
(packaged `linux-arm64` for the Node images and for the Basecamp image),
`lez_atomic_swap_maker`, `lez_atomic_swap_taker`, `lez_network` and
`lez_swap_node`. Upstream components keep their own versions: `lez_core`
0.4.1-sign.4, `blockchain_module` 0.2.4, `lez_indexer_module` 1.1.1-rpc.4,
`chat_module` 0.2.2, `delivery_module` 0.2.0.

## Images

| image | id | this run |
|---|---|---|
| `lez-maker-node:local` | `sha256:361365e1d8df8eb28a1c8427847569907b45a4e0094204481ab9f1bdf6078a67` | built, 2026-11-19 07:07Z |
| `lez-taker-node:local` | `sha256:b2bc1e427fddd13a4e99e80ef48ae11aafddada43caad0c87d716c070b3d54ea` | built, 2026-11-19 07:07Z |
| `lez-basecamp-ui:local` | `sha256:1df1279b092d927f41b921ec729d16ae2996b2cdce0d85c091e350ffbd8f2acc` | built, 2026-11-19 07:07Z |
| `lez-network-module:local` | `sha256:aa7d29de82d54b555e4fbd14c91592dff28020f0b9b7689b9ebf3c9966c58754` | built, 2026-11-19 07:07Z |
| `lez-services:local` (sequencer) | `sha256:f12005f4ce46374689d5a247e75a178db961be1fd460eeaf2958238b8056c0ee` | cache (2026-11-18) |
| `lez-zebra:local` (Zebra 7.0.0) | `sha256:de894d2d6a2ae38d9b6c0d98fe8360127240aa23597fd97fefcac48d39fe5736` | cache (2026-11-18) |
| `lez-bitcoin-core:local` | `sha256:39e939eabe1cc7024b140b36464587f72e8b97d528162cf3849cb1479d653691` | cache (2026-11-18) |
| `lez-btc-miner:local` | `sha256:c00b0cecc587f01984d7f27527644c10619cec42c9674b7f40d201fba0e5d379` | cache (2026-11-18) |

Ids and creation times are the ones `logs/run-summary.log` records after
`from-scratch.sh`. All are local `linux/arm64` builds of this host; nothing
was pulled from a registry, no proxy was set for the build, and no release
was built from this commit. `from-scratch.sh --workspace <workspace>` ran
from 07:03:25Z to 07:09:31Z and staged every payload from `fab36d52`; its
stack phase ran `up.sh --build`, incremental on the build cache of the
day's earlier builds of the branch.

Each Node image carries its build source in
`/usr/local/share/lez/build-source.txt`. `logs/run-summary.log` records the
build source of both images right after the build, and the build source and
installed package source of all six Node containers (three lanes) before
the smoke: `fab36d525c4c9514e9b9b518f3317d7fd024c6ef` everywhere. The
wrapper stops before the run when any of them reports another commit. Before
the smoke it also checked that no `lez*` container carries a proxy variable,
and that the stack's timing profile was `fast`.

## The run

- Host: MacBook Pro, Apple M1 Max, 10 cores, 64 GB; macOS 26.7.3; Docker
  Desktop, engine 29.9.2, 10 CPUs and 32 GB given to its VM. The same host
  as the 0a42823f run. Other work ran on the host at the same time; its
  1-minute load average ranged from about 9 to 60.
- In order (`logs/run-summary.log`): `lanes.sh down 3 --wipe` and `down.sh
  --wipe` (07:02Z to 07:03Z); `from-scratch.sh` (07:03Z to 07:09Z);
  `lanes.sh down 3 --wipe` and `lanes.sh mint 3`; `up.sh --fresh-lez` with
  LEZ_LANES=3 (`logs/fresh-lez.log`, done at 07:11Z), which made the chain
  the run started on; `regress.sh smoke`, 6 of 6 in 3 min
  (`logs/smoke-regress.log`, `logs/smoke-results.tsv`); then
  `LEZ_REGRESS_RECORD=1 LEZ_LANES=3 scripts/regress.sh full` from 07:14:58Z
  to 08:54:43Z (99 min).
- Lanes: lane 1 is the stack; lanes 2 and 3 are one more Maker/Taker pair
  each with their own Bitcoin Core, miner and Zebra, on the stack's one LEZ
  network. `results.tsv` names the lane of every test (31, 33 and 31 on
  lanes 1, 2 and 3; the two balance-refusal tests ran on lane 1 alone).
- LEZ chains: the starting chain served every test but two;
  `zec-balance-refusal` in each direction starts a chain of its own (the
  last two tests, `logs/chain-1.log` and `logs/chain-2.log`). No chain was
  replaced for a finality freeze: 0 frozen tries. `chain.tsv` samples the
  finalized height and the tip every 30 s; the widest gap between them was
  15 blocks on the starting chain and 25 on the balance-refusal chains.
- Timing profiles: `fast` from the start, `fast` -> `refund` at 08:31:19Z
  (`logs/profile-1.log`); the run ended on `refund`.
- Before each test that does not start a chain of its own, the runner
  resets the lane's swaps with `reset-swaps.sh --abandon-open`; those
  per-lane reset logs are not in this set.
- The per-test logs (`logs/node-*.log`, `logs/ui-*.log`) print the host's
  local time (CET, UTC+1). The runner's and the wrapper's logs, the
  start-up step lines, `results.tsv` and `chain.tsv` are in UTC.

## The three failures and the rerun

`ui:zec:zec-happy:TakerSellsLez`, `ui:zec:zec-survivor:TakerSellsLez` and
`ui:zec:zec-node-offline:TakerSellsLez` failed on lane 1 between 07:44Z and
07:51Z. The cause is one start-up hang of the lane-1 Taker Node's container,
described in `RESULTS.txt`: Basecamp in `lez-taker-node`, recreated at
07:44:46Z by `zec-provision.sh`, loaded its first three modules and then
none of the rest, so the Taker Node of this project never started.
`taker-hang/` holds what was captured before the container was touched:

- `docker-logs.log`: the container's log, which ends at 07:44:47Z after
  "Module loaded: package_downloader".
- `state.json`: the container's state, health log (24 failed readiness
  probes) and health check.
- `threads.txt`: every thread of Basecamp and of the three module hosts,
  with its kernel wait channel and state: all sleeping in poll, futex or
  pipe reads; and the container's `/tmp` and `/run/lez/taker` (no Node
  socket).

The runner's swap reset restarted that container at 07:51:49Z, and it was
restarted once more by hand at 07:51:52Z; it was healthy at 07:51:56Z, and
every lane-1 test after it passed. The hang did not occur again in the run
or the rerun.

The rerun: after the run, `up.sh --fresh-lez` with three lanes made a new
chain (the run had ended on a balance-refusal chain; `rerun/fresh-lez.log`),
no `lez*` container carried a proxy variable, and `LEZ_REGRESS_RECORD=1
LEZ_LANES=3 scripts/regress.sh run` ran the three tests from 08:57Z to
09:01:31Z on the same images: 3 of 3 passed on their first try
(`rerun/results.tsv`, `rerun/regress.log`, `rerun/logs/`, `rerun/zec/`).

## Recordings

Every desk test was recorded on its lane's two Basecamp displays
(`ui-e2e.sh --record`); a recording is a by-product of the test that passed.
The run made 29 recordings (the three failed desk tests made none, and the
two Bitcoin `node-offline` desk tests have no recorded desk step), the rerun
3. Seven are in `videos/`, the same ZEC set as the other sets: `zec-happy`,
`zec-maker-refund` and `zec-concurrent` in both directions and
`zec-stale-window` selling ZEC. `TakerSellsLez-zec-happy.mp4` is the
rerun's; the other six are the run's. All 32 are kept on the host, not in
git; `videos-kept-outside-git.sha256` lists them, the rerun's under
`rerun/`.

## Not covered by this set

- Latency to the chains: the chains ran on the same host as the Nodes.
- Any public network: see `docs/m2-acceptance-checklist.md`.
- Linux x86_64: no regression of this commit ran there.
- A released bundle or the package install path: the images are local
  builds, and the Nodes and desks ran from packages staged into the images,
  not installed from a release bundle or a module catalog.
- macOS on Intel, and Linux arm64 outside Docker Desktop's VM.
- Native darwin Nodes: the Nodes ran as `linux/arm64` containers in Docker
  Desktop's VM.
