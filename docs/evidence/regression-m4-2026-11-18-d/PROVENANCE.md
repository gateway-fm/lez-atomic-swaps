# Provenance — full regression at `10a785ad`, 2026-11-18

Everything in this folder was produced on one host by the Docker devnet of `deploy/`: every
test of `scripts/regress.sh full` (159: `regress.sh list full`), the Node scenarios by
`scripts/node-e2e.py` and the desk journeys by `scripts/ui-e2e.sh`, three at a time on three
regression lanes, with the Monero regtest chain up (`LEZ_MONERO=1`).

Nothing here is private. `results.tsv` and `regress.log` carry test names, verdicts, times,
lanes and LEZ heights. No key, cookie, capability file, database, `runtime/` or `market/`
content is included. The per-test logs and the Maker Nodes' `node.log` files read for the README
are not committed; the three `node.log` files (and the three Taker ones) were copied out of the
containers to a scratch directory, searched and deleted. No local path is in any file. Scanned
before commit with the repository's gitleaks configuration.

## Source

`10a785ada37bd00dfdafe20e1359dab30cfeda3e` (`fix(sidecar): a submission refused before any
broadcast can be tried again under its request id`). It is `44e39081` plus `234c17db`,
`ef0f1299`, `15b1dc84`, the docs-only `9c958b06` and `10a785ad`. The checkout the stack ran from
was clean and detached at that commit for the image build and the run (`regress.log` prints
"10a785ad (tree dirty: 0)"). Trees at that commit (`git rev-parse 10a785ad:<directory>`):

| directory | tree |
|---|---|
| `crates` | `ff30c1391a8e5d6e967ab9ca588ed5e581f4d6b3` |
| `compat` | `c3cd7cad15a84a5317ca69fbf4cd0c790a6838a2` |
| `apps` | `5ba454f517f805edd5327439730c339c8f2cfeaf` |
| `modules` | `94e405f1338f4b29fb9f3bbf402c97a48c59ad70` |
| `deploy` | `d9ef4711f1e7e4c0bbb97320fbae0a0835d0acf0` |
| `packaging` | `d3990b01664590d0ee2e6eca22b1c7ff185a880b` |

`crates`, `apps`, `modules`, `deploy` and `packaging` are the trees of `15b1dc84`; `compat`
differs by `10a785ad`.

## Images

Local linux/amd64 images built on this host; none pulled from a registry, no release built from
this commit. The Node binaries were built in a development profile and packaged as
`lez_maker_node` and `lez_taker_node` 0.4.0, with `scripts/from-scratch.sh --only` for
`build:sidecar`, `rust` and `stage`, then the compose build of the Node images. Ids read with
`docker image inspect` after the run:

| image | id | created (UTC) |
|---|---|---|
| `lez-maker-node:local` | `sha256:f9334eb2d7bd250232d6ccb990a1dbda7bf1e665619e1f8251f1df239239afce` | 2026-11-18 16:50 |
| `lez-taker-node:local` | `sha256:2723dbd297405a1038e78f396cf47778a14e9783a29eac17c411b2ff54d2f491` | 2026-11-18 16:50 |
| `lez-network-module:local` | `sha256:3bc1597481409bb301e261321142b857ea1209605bf5af24866388d02f77d2e1` | 2026-11-16 16:27 |
| `lez-basecamp-ui:local` | `sha256:c940c40f7220f3f52a81deea67119b8774b6a231b6d56f2d3950cdc656e319f5` | 2026-11-16 16:27 |
| `lez-services:local` | `sha256:021a7928e64602ce785ce464a83df7e9c27588391c24f6fd3d048489f7d52bef` | 2026-11-13 18:33 |
| `lez-zebra:local` | `sha256:42d64e9a9e85dbd24911f26b2676f4ca07f3c7d910e80dfb99dfb4ca49ecc101` | 2026-11-11 11:50 |
| `lez-bitcoin-core:local` | `sha256:dadb2fd79b64a6e20612440c90146d6501a1b3d2039408760e93d48c33619cc5` | 2026-11-11 15:01 |
| `lez-btc-miner:local` | `sha256:4a33449ce116b239641c7932c63845e2d7c9a54e0d00f39d19688e7c3e403473` | 2026-11-11 15:01 |
| `lez-monerod:local` | `sha256:db4f4b5620a34a5634bd31d4ab9fc8f92b3ce00139f586828287f41e3ce1d9cf` | 2026-11-13 15:13 |

The Node images were built before the run and not rebuilt during it; the Node containers of all
three lanes ran the two Node ids above (`docker inspect`, read after the run).
`lez-network-module` and `lez-basecamp-ui` carry the creation time of a cached build.

Build source: `/usr/local/share/lez/build-source.txt` read
`10a785ada37bd00dfdafe20e1359dab30cfeda3e` for the Maker and the Taker right before the run
(after `up.sh`'s end, before the run's start at 18:38:39Z), and again in all six Node containers
after the run (`docker cp` of the file). Bitcoin Core: the arguments of the three lanes' Cores
(`docker inspect`) held `-checkblockindex=0`, checked at the same two moments.

## The run

- Host: Linux 6.8 x86_64, 8 cores, Docker 29.1.3. Other projects' containers share it.
- From the checkout's `deploy/` at `10a785ad`, with the images above:
  `SKIP_UI_VERIFY=1 LEZ_NO_BANNER=1 LEZ_MONERO=1 LEZ_LANES=3 bash scripts/up.sh --fresh-lez`
  (new LEZ chain, genesis 2026-11-18 18:36:28Z; lanes 2 and 3 up at 18:38:31Z; desks skipped),
  the two checks above, then `LEZ_MONERO=1 LEZ_LANES=3 bash scripts/regress.sh full`
  (`20261118T183839Z`, 18:38:39Z to 22:05:28Z, 206 min, exit 0). Its console output is not
  committed: it holds host paths.
- The Bitcoin actors' trace (`LEZ_BTC_ACTOR_TRACE=1`) was on in the devnet's `runtime.env`.
- Lanes: lane 1 is the stack; lanes 2 and 3 are one more Maker/Taker pair each with their own
  Bitcoin Core, miner, Zebra, `monerod` and wallet-rpc, on the stack's one LEZ network.
  `results.tsv` names the lane of every try.
- LEZ chains: the chain `up.sh` made served every test of the run but two; both
  `zec-balance-refusal` tests ran at once on one new chain (lanes 1 and 2, the last two tests).
  0 frozen tries.
- Groups (`regress.log`): `fast` from 18:39:09Z, `zec-v6` from 21:14:15Z, `refund` from
  21:39:45Z, the own-chain pair from 21:59:53Z.
- No try ended other than PASSED, so the harness kept no Node logs under the run's `logs/`.
- Read after the run for the README: the per-test logs of the run, and each lane's Maker
  `node.log` in its socket volume, copied out of the container, searched and deleted.
