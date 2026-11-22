# Provenance — full regression at `7b4e46da`, 2026-11-22, macOS arm64

One operator ran both parties, Maker and Taker, on one host. Everything in this folder was
produced on that host by the Docker devnet of `deploy/`: every test of `scripts/regress.sh full`
(175: `regress.sh list full`), the Node scenarios by `scripts/node-e2e.py` and the desk journeys
by `scripts/ui-e2e.sh`, three at a time on three regression lanes, with the Monero regtest chain
up (`LEZ_MONERO=1`), and two reruns of one test alone on lane 1.

Nothing here is private. `results.tsv`, `regress.log` and `reruns/*/` carry test names, verdicts,
times, lanes and LEZ heights; `diagnostics/` holds the test log of the three tries of
`ui:btc:logos-happy:TakerSellsForeign`, which carry offer and swap ids, the Maker's public
Delivery key and Chat byte counts. No key, login, wallet seed, cookie, capability file, database,
`runtime/` or `market/` content is included. The other per-test logs, the swap records under the
run's `zec/` and the Nodes' `node.log` files read for the README are not committed; the three
Maker `node.log` files were copied out of the containers to a scratch directory, searched and
deleted. No local path is in any file. The folder was searched before commit for local paths and
for key, password, seed, cookie and login material.

## Source

`7b4e46da33d0004e1701c0ea8c479b96f13bcbcd` (`fix(node-e2e): the XMR escrow probe reads the
stack's manifest on every lane`), parent `e1fbecea` (`test(maker-node): the XMR fixture's fake
monerod answers get_info`), on top of `a5e8e64a`, `84062b80` (documents) and `ee66628d`.

The checkout the stack ran from was clean at `7b4e46da` for the image build, the run and the
reruns (`regress.log` prints "7b4e46da (tree dirty: 0)"). Trees at `7b4e46da`
(`git rev-parse 7b4e46da:<directory>`):

| directory | tree | at `d4475300` |
|---|---|---|
| `crates` | `10646923119432704c6c2e07c0e67b0592182dc3` | `24472d7fbaae852a0e4ee6e2dea4da0cad4e416c` |
| `compat` | `a9d06187ed70a705040c41e167739b6aaeacfc23` | `afc0a9c02a3b9ca76ece050cf55902b87f148b1e` |
| `apps` | `98dc561634a68592d06dc5c07ed5081f13723e9a` | `f90082a92da14ea709a9c6486d8c9947e5b4c048` |
| `modules` | `1833a53d89f904ecf0b529ab343185f6b95d9e7d` | `739d2c871f0f5c2b4fcfd96ea0bf07be5f4aa455` |
| `deploy` | `3909c163dc836e1cb8ee01424c8538c428661312` | `9ca1143ab2c245d222593a623712bbb211169251` |
| `packaging` | `57e67123c96fff77d35b041e4eb5aac5a9015cad` | `57e67123c96fff77d35b041e4eb5aac5a9015cad` |

This note is committed on top of `7b4e46da`; it changes nothing outside this folder.

## Images

Local linux/arm64 images built on this host from the clean checkout at `7b4e46da`; none pulled
from a registry, no release built from this commit. The build's console shows
`scripts/from-scratch.sh --only rust` from 05:34:35Z (the Node binaries in a development profile
with the devnet test hooks, `LEZ_TEST_HOOKS=1`, staged at 05:34:48Z), `--only build` (the
sidecar, identity tool and the escrow claim and custody credit probes, with the test hooks) to
05:37:20Z, `--only stage` to 05:39:31Z, then `--only stack`: the compose build of the stack's
images, the market bootstrap and the Nodes' restart, then `up.sh` (done at 05:42:49Z). Ids read
with `docker image inspect` after the run:

| image | id | created (UTC) |
|---|---|---|
| `lez-maker-node:local` | `sha256:4f856c2aa22db660ba7ba7aa249875c6af3d787f2fea5a9a9913b21a73b42295` | 2026-11-22 05:38 |
| `lez-taker-node:local` | `sha256:e8e061ea73e2e6588bd293cd0a6b5019dd5a7ab79ca983269ef8197e3a52ce86` | 2026-11-22 05:38 |
| `lez-network-module:local` | `sha256:07dcc842b3b177beed22080c37c3040091eb86f33c2cf5aa7254b01856786ff0` | 2026-11-21 15:51 |
| `lez-basecamp-ui:local` | `sha256:aa8a228b08b613c9b3d2ab1c99880312d2ba97a5af33430798b886220a18b851` | 2026-11-21 15:51 |
| `lez-wallet-module:local` | `sha256:fc91ca78b3a9f518c33a621fcfe90f8156a2cd5dfdbf975c6252e706de4226a0` | 2026-11-11 21:52 |
| `lez-services:local` | `sha256:0a2359de44f324e57efe189e1222782b59d689e2eb737a0715477f03ef875a92` | 2026-11-21 10:54 |
| `lez-zebra:local` | `sha256:66274a23447eeb7208882bb4e30390d8ff9065ab4ba5896dcd05676650c66848` | 2026-11-21 10:54 |
| `lez-bitcoin-core:local` | `sha256:39e939eabe1cc7024b140b36464587f72e8b97d528162cf3849cb1479d653691` | 2026-11-18 16:42 |
| `lez-btc-miner:local` | `sha256:c00b0cecc587f01984d7f27527644c10619cec42c9674b7f40d201fba0e5d379` | 2026-11-18 16:43 |
| `lez-monerod:local` | `sha256:df7f6caf341e1b052c09605908b3bd87f2905b5625995a146ab4bffb72173c77` | 2026-11-21 10:58 |

The Node images were built before the run and not rebuilt before or during it, nor before the
reruns; the Node containers of all three lanes ran the two Node ids above (`docker inspect`,
read after the run). The compose build took `lez-basecamp-ui:local` and
`lez-network-module:local` from its cache: their creation time is that of an earlier build of the
same inputs.

Build source: `/usr/local/share/lez/build-source.txt` read
`7b4e46da33d0004e1701c0ea8c479b96f13bcbcd` in the lane-1 Maker and Taker right after `up.sh` and
before the run's start at 05:43:05Z, and after the run in all six Node containers (`docker exec`
for lanes 1 and 2, `docker cp` for lane 3's two stopped ones). The escrow the market bootstrap
deployed on the run's chain (`bootstrap/deployment.json` of the market root) has ImageID
`ca66e0dc4118597e0a9010670fd49aec923bba9c332e9e6a26d9208dbdf6af98`, and the Maker's escrow
probe named the same program id in the run's logs. Bitcoin Core: the arguments of the three
lanes' Cores (`docker inspect`) held `-checkblockindex=0`, read after the run.

## The run

- Host: Apple-silicon Mac, macOS 26.7.3, 10 cores; Docker Desktop 29.9.2 with 10 CPUs and about 32 GB
  of memory for its VM. Other projects' containers share it.
- State: the earlier stack and its lanes 2 and 3 were taken down with `down.sh --wipe` and
  `lanes.sh down 3 --wipe` (stack and lane volumes, `runtime/`; the escrow deployment of the old
  chain retired in the market root, the wallet identities kept). The stack phase then started a
  new LEZ chain (genesis time 2026-11-22 05:38:06Z in `runtime.env`) and new Bitcoin, Zcash and
  Monero regtest chains, and deployed the escrow; `up.sh` with `LEZ_MONERO=1
  LEZ_TIMING_PROFILE=fast` kept them.
- From the checkout's `deploy/` at `7b4e46da`: `LEZ_MONERO=1 LEZ_LANES=3 bash scripts/regress.sh
  full` (`20261122T054305Z`, 05:43:05Z to 09:51:50Z, 248 min, exit 1: one test not passed).
  Lanes 2 and 3 were brought up by `regress.sh` itself (`lanes.sh up 3`). Its console output is
  not committed: it holds host paths.
- The Bitcoin actors' trace (`LEZ_BTC_ACTOR_TRACE=1`) was on in the devnet's `runtime.env`.
- Lanes: lane 1 is the stack; lanes 2 and 3 are one more Maker/Taker pair each with their own
  Bitcoin Core, miner, Zebra, `monerod` and wallet-rpcs, on the stack's one LEZ network.
  `results.tsv` names the lane of every try.
- LEZ chains: the chain of the stack phase served every test of the run but two; both
  `zec-balance-refusal` tests ran at once on one new chain (lanes 1 and 2, the last two tests),
  which also served both reruns. 0 frozen tries.
- Groups (`regress.log`): `fast` from 05:43:45Z, `zec-v6` from 08:49:37Z, `refund` from
  09:27:14Z, the own-chain pair from 09:47:56Z.
- Reruns, on lane 1, after the run, with `LEZ_MONERO=1 bash scripts/regress.sh run
  ui:btc:logos-happy:TakerSellsForeign`: `rerun1-20261122T095202Z` (FAILED, 60 s) and
  `rerun2-20261122T095352Z` (PASSED, 79 s), in `reruns/1/` and `reruns/2/`.
- Read after the run for the README: the per-test logs of the run, the swap records under the
  run's `zec/`, and each lane's Maker `node.log` in its socket volume, copied out of the
  container, searched and deleted.
