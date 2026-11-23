# Provenance — full regression at `27526e2f`, 2026-11-22/23, macOS arm64

One operator ran both parties, Maker and Taker, on one host. Everything in this folder was
produced on that host by the Docker devnet of `deploy/`: every test of `scripts/regress.sh full`
(176: `regress.sh list full`), the Node scenarios by `scripts/node-e2e.py` and the desk journeys
by `scripts/ui-e2e.sh`, three at a time on three regression lanes, with the Monero regtest chain
up (`LEZ_MONERO=1`) and the devnet test hooks built in (`LEZ_TEST_HOOKS=1`).

Nothing here is private. `results.tsv` and `regress.log` carry test names, verdicts, times, lanes
and LEZ heights. No key, login, wallet seed, cookie, capability file, database, `runtime/` or
`market/` content is included. The per-test logs, the swap records under the run's `zec/`, the
Logos phases `ui-e2e.sh` kept and the Nodes' `node.log` files read for the README are not
committed; the three Maker `node.log` files were copied out of the containers to a scratch
directory, searched and deleted. No local path is in any file. The folder was searched before
commit for local paths and for key, password, seed, cookie and login material.

## Source

`27526e2f6644890fdb5b70f0631b4c98c96e2a38` (`Merge the Monero Stagenet evidence and docs
(m4/node-stagenet)`), parents `5a7688a9` (`feat/m4-delivery` with the sidecar's in-doubt
submission fixes and the `xmr-kill-maker` holds) and `0d8d17af` (the Stagenet documents on top of
`5c9694fd`, the Stagenet evidence set).

The checkout the stack ran from was clean at `27526e2f` for the image build and the run
(`regress.log` prints "27526e2f (tree dirty: 0)"). Trees at `27526e2f`
(`git rev-parse 27526e2f:<directory>`):

| directory | tree | at `7b4e46da` |
|---|---|---|
| `crates` | `d9d3701fbc9c8b33bb472221b62be2aedbe30238` | `10646923119432704c6c2e07c0e67b0592182dc3` |
| `compat` | `a5d3806501ee1e06e262ed4aa0fce41f22174ff5` | `a9d06187ed70a705040c41e167739b6aaeacfc23` |
| `apps` | `98dc561634a68592d06dc5c07ed5081f13723e9a` | the same |
| `modules` | `1833a53d89f904ecf0b529ab343185f6b95d9e7d` | the same |
| `deploy` | `dd7b2ed54d9b82f3ddce6d8d5cf17ab563942c16` | `3909c163dc836e1cb8ee01424c8538c428661312` |
| `packaging` | `57e67123c96fff77d35b041e4eb5aac5a9015cad` | the same |

This note is committed on top of `27526e2f`.

## Images

Local linux/arm64 images built on this host from the clean checkout at `27526e2f`; none pulled
from a registry, no release built from this commit. `scripts/from-scratch.sh --only rust`, `build`
(the sidecar, the identity tool and the escrow claim and custody-credit probes, with the test
hooks), `stage` and `stack` with `LEZ_MONERO=1 LEZ_TEST_HOOKS=1`, first for the runs before this
one on 2026-11-22 from 19:33Z, then again after the wipe from 21:11:55Z (the compose build reused
the Node images of the first build, unchanged inputs), `up.sh` done at 21:18:42Z. Ids read with
`docker image inspect` after the run:

| image | id | created (UTC) |
|---|---|---|
| `lez-maker-node:local` | `sha256:35615cf38e8b41e4299e6e11c84257f956adbde33bd66e629141f6d46ed48f0b` | 2026-11-22 19:39 |
| `lez-taker-node:local` | `sha256:5c93745886043bd770404663d6c40b373dc314b91b575ace04e8fb693558033c` | 2026-11-22 19:39 |
| `lez-network-module:local` | `sha256:07dcc842b3b177beed22080c37c3040091eb86f33c2cf5aa7254b01856786ff0` | 2026-11-21 15:51 |
| `lez-basecamp-ui:local` | `sha256:516a55b01566b80de2cf75d813deba3841f9369b6db27ad12053df68845af842` | 2026-11-21 15:51 |
| `lez-wallet-module:local` | `sha256:fc91ca78b3a9f518c33a621fcfe90f8156a2cd5dfdbf975c6252e706de4226a0` | 2026-11-11 21:52 |
| `lez-services:local` | `sha256:0a2359de44f324e57efe189e1222782b59d689e2eb737a0715477f03ef875a92` | 2026-11-21 10:54 |
| `lez-zebra:local` | `sha256:66274a23447eeb7208882bb4e30390d8ff9065ab4ba5896dcd05676650c66848` | 2026-11-21 10:54 |
| `lez-bitcoin-core:local` | `sha256:39e939eabe1cc7024b140b36464587f72e8b97d528162cf3849cb1479d653691` | 2026-11-18 16:42 |
| `lez-btc-miner:local` | `sha256:c00b0cecc587f01984d7f27527644c10619cec42c9674b7f40d201fba0e5d379` | 2026-11-18 16:43 |
| `lez-monerod:local` | `sha256:df7f6caf341e1b052c09605908b3bd87f2905b5625995a146ab4bffb72173c77` | 2026-11-21 10:58 |

The Node containers of all three lanes ran the two Node ids above (`docker inspect`, read after
the run). The compose build took `lez-basecamp-ui:local` and `lez-network-module:local` from its
cache: their creation time is that of an earlier build of the same inputs.

Build source: `/usr/local/share/lez/build-source.txt` and `sidecar-source.txt` read
`27526e2f6644890fdb5b70f0631b4c98c96e2a38` in the lane-1 Maker and Taker right after `up.sh` and
before the run's start at 21:19:02Z, and `build-source.txt` the same after the run in all six
Node containers (`docker exec` for lanes 1 and 2, `docker cp` for lane 3's two stopped ones). The
escrow the market bootstrap deployed on the run's chain (`bootstrap/deployment.json` of the
market root) has ImageID `ca66e0dc4118597e0a9010670fd49aec923bba9c332e9e6a26d9208dbdf6af98`.

## The run

- Host: Apple-silicon Mac, macOS 26.7.3, 10 cores; Docker Desktop 29.9.2 with 10 CPUs and about
  32 GB of memory for its VM. Other projects' containers share it; a Monero Stagenet stack of this
  project (`lez-m4-stagenet-*`) ran beside it. Docker had 25.1 GB free at the start and 23.0 GB
  or more at every check during the run.
- State: the stack and its lanes 2 and 3 were taken down with `down.sh --wipe` and
  `lanes.sh down 3 --wipe` (stack and lane volumes, `runtime/`; the escrow deployment of the old
  chain retired in the market root, the wallet identities kept). The stack phase started a new LEZ
  chain (genesis time 2026-11-22 21:15:46Z in `runtime.env`) and new Bitcoin, Zcash and Monero
  regtest chains and deployed the escrow; `up.sh` with `LEZ_MONERO=1 LEZ_TIMING_PROFILE=fast`
  kept them.
- From the checkout's `deploy/` at `27526e2f`: `LEZ_MONERO=1 LEZ_LANES=3 bash scripts/regress.sh
  full` (`20261122T211901Z`, 21:19:02Z to 01:42:08Z, 263 min, exit 0). Lanes 2 and 3 were brought
  up by `regress.sh` itself (`lanes.sh up 3`). Its console output is not committed: it holds host
  paths.
- The Bitcoin actors' trace (`LEZ_BTC_ACTOR_TRACE=1`) was on in the devnet's `runtime.env`.
- Lanes: lane 1 is the stack; lanes 2 and 3 are one more Maker/Taker pair each with their own
  Bitcoin Core, miner, Zebra, `monerod` and wallet-rpcs, on the stack's one LEZ network.
  `results.tsv` names the lane of every try.
- LEZ chains: the chain of the stack phase served every test of the run but two; both
  `zec-balance-refusal` tests ran at once on one new chain (lanes 1 and 2, the last two tests).
  0 frozen tries.
- Groups (`regress.log`): `fast` from 21:19:45Z, `zec-v6` from 00:38:59Z, `refund` from 01:16:47Z,
  the own-chain pair from 01:38:07Z.
- Every try ended PASSED; the harness kept no Node logs for a failed or not-exercised try.
- Read after the run for the README: the per-test logs of the run, the swap records under the
  run's `zec/`, the Logos phases under each lane's `evidence/logos/`, and each lane's Maker
  `node.log` in its socket volume, copied out of the container, searched and deleted.
