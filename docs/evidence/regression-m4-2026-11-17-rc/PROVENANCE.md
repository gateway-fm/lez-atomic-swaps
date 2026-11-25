# Provenance — full regression at `32c8bfb4`, 2026-11-17

Everything in this folder was produced on one host by the Docker devnet of `deploy/`: every
test of `scripts/regress.sh full` (159: `regress.sh list full`), the Node scenarios by
`scripts/node-e2e.py` and the desk journeys by `scripts/ui-e2e.sh`, three at a time on three
regression lanes, with the Monero regtest chain up (`LEZ_MONERO=1`); and, after it, five reruns
of three survivor tests on the same images.

Nothing here is private. `results.tsv` and `regress.log` carry test names, verdicts, times,
lanes and LEZ heights; the logs carry step lines with swap, offer and transaction ids. No key,
cookie, capability file, `runtime/` or `market/` content is included; the reset archive read for
the diagnosis was copied out, read and deleted, and nothing from it is here. No local path is
in any file. Scanned before commit with the repository's gitleaks configuration.

## Source

`32c8bfb443de73005eb81d592437e2fbbe11364f` (`fix(deploy): the Monero windows count a Maker drive
at 80 s`). It carries `7d09351a`, the commit of the morning run. The checkout the stack ran from
was clean and detached at that commit for the image build, the run and the reruns (`regress.log`
and each rerun's runner log print "32c8bfb4 (tree dirty: 0)"). Trees at that commit
(`git rev-parse 32c8bfb4:<directory>`):

| directory | tree |
|---|---|
| `crates` | `d48676f594f15b54e1e305f513ac8c6588367f4c` |
| `compat` | `b64e1d8235d4896da57be65c0c7b7fe799d3f612` |
| `apps` | `5ba454f517f805edd5327439730c339c8f2cfeaf` |
| `modules` | `94e405f1338f4b29fb9f3bbf402c97a48c59ad70` |
| `deploy` | `378ed796b2dd6b2d1058b1c6bea5e4d75cc6e2d5` |
| `packaging` | `d3990b01664590d0ee2e6eca22b1c7ff185a880b` |

`compat`, `apps`, `modules` and `packaging` are the morning run's trees; `crates` and `deploy`
differ by the commits the README lists.

## Images

Local linux/amd64 images built on this host; none pulled from a registry, no release built from
this commit. The Node binaries were built in `rust:1.96.0-bookworm` (development profile) and
packaged as `lez_maker_node` and `lez_taker_node` 0.4.0. Ids read with `docker image inspect`
after the run:

| image | id | created (UTC) |
|---|---|---|
| `lez-maker-node:local` | `sha256:2b1d9698be09c1deabc434e4b0739eb1ce150bc63dd54412a23a2a116da81989` | 2026-11-17 13:21 |
| `lez-taker-node:local` | `sha256:0b9c524d15ea2d35ec227cd39222b10283eddf4de30a05b14ba9b37ad147c16d` | 2026-11-17 13:21 |
| `lez-network-module:local` | `sha256:0b464d09c3bd74f9aa0faef522a0cb8ee3304b8ecc85de349069221a47fe9772` | 2026-11-16 16:27 |
| `lez-basecamp-ui:local` | `sha256:1388765fb86e72808655367994ad5c67b239bc16087a77226611a993f2dc5d57` | 2026-11-16 16:27 |
| `lez-services:local` | `sha256:021a7928e64602ce785ce464a83df7e9c27588391c24f6fd3d048489f7d52bef` | 2026-11-13 18:33 |
| `lez-zebra:local` | `sha256:42d64e9a9e85dbd24911f26b2676f4ca07f3c7d910e80dfb99dfb4ca49ecc101` | 2026-11-11 11:50 |
| `lez-bitcoin-core:local` | `sha256:dadb2fd79b64a6e20612440c90146d6501a1b3d2039408760e93d48c33619cc5` | 2026-11-11 15:01 |
| `lez-btc-miner:local` | `sha256:4a33449ce116b239641c7932c63845e2d7c9a54e0d00f39d19688e7c3e403473` | 2026-11-11 15:01 |
| `lez-monerod:local` | `sha256:db4f4b5620a34a5634bd31d4ab9fc8f92b3ce00139f586828287f41e3ce1d9cf` | 2026-11-13 15:13 |

The Node images were built before the run (13:21Z) and were not rebuilt until after the
reruns; the containers of the reruns (`lez-maker-node`, `lez-l3-maker-node`,
`lez-l2-taker-node`, read with `docker inspect`) ran the two Node ids above. `lez-network-module`
and `lez-basecamp-ui` were built from cache to their ids of 2026-11-16 (their creation time is
that of the cached build). The chain images are the morning run's.

Build source: `/usr/local/share/lez/build-source.txt` read
`32c8bfb443de73005eb81d592437e2fbbe11364f` in both Node images right before the run (between up.sh's end at 14:12:47Z and the run's start), and
in the running lane-1, lane-2 and lane-3 Node containers before the reruns (18:21Z).

## The run

- Host: Linux 6.8 x86_64, 8 cores, Docker 29.1.3. Other projects' containers share it.
- From the checkout's `deploy/`: `SKIP_UI_VERIFY=1 LEZ_NO_BANNER=1 LEZ_MONERO=1 LEZ_LANES=3
  scripts/up.sh --fresh-lez` (new LEZ chain, genesis 2026-11-17 14:10:28Z; lanes 2 and 3 up at
  14:12:42Z), the build-source check above, then `LEZ_MONERO=1 LEZ_LANES=3 scripts/regress.sh
  full` (`20261117T141249Z`, 14:12:49Z to 18:14:22Z, 241 min). Its console output is not
  committed: it holds host paths.
- Lanes: lane 1 is the stack; lanes 2 and 3 are one more Maker/Taker pair each with their own
  Bitcoin Core, miner, Zebra, `monerod` and wallet-rpc, on the stack's one LEZ network.
  `results.tsv` names the lane of every try.
- LEZ chains: the chain `up.sh` made served every test of the run but two; both
  `zec-balance-refusal` tests ran at once on one new chain (lanes 1 and 2, the last two tests).
  0 frozen tries.
- Groups (`regress.log`): `fast` from 14:13:23Z, `zec-v6` from 17:18:33Z, `refund` from
  17:45:07Z, the own-chain pair from 18:08:57Z.
- The diagnosis read the lane-3 Maker's and Taker's reset archive `20261117T171942Z` in their
  state volumes (the reset after the failed test), copied to a scratch directory, read with
  `sqlite3` and deleted. The lane-3 Node containers had been recreated at 17:45Z by a later
  test, so their logs of the failed test were gone.

## The reruns

The five reruns (`20261117T182106Z`, `20261117T182340Z`, `20261117T182615Z`,
`20261117T182843Z`, `20261117T183118Z`, 18:21:06Z to 18:34:01Z) ran from the same clean checkout
at `32c8bfb4` on the same images, after the build-source check above, on a new LEZ chain made by
`SKIP_UI_VERIFY=1 LEZ_NO_BANNER=1 LEZ_MONERO=1 LEZ_LANES=3 scripts/up.sh --fresh-lez` (finished
at 18:20:49Z), with `LEZ_MONERO=1 LEZ_LANES=3`. `logs/reruns/` holds each rerun's runner log,
table and the two Bitcoin tests' logs; the Zcash test's logs are not committed.
