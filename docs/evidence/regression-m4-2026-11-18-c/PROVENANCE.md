# Provenance — full regression at `15b1dc84`, 2026-11-18

Everything in this folder was produced on one host by the Docker devnet of `deploy/`: every
test of `scripts/regress.sh full` (159: `regress.sh list full`), the Node scenarios by
`scripts/node-e2e.py` and the desk journeys by `scripts/ui-e2e.sh`, three at a time on three
regression lanes, with the Monero regtest chain up (`LEZ_MONERO=1`).

Nothing here is private. `results.tsv` and `regress.log` carry test names, verdicts, times,
lanes and LEZ heights; the failed test's log carries step lines with swap, offer and transaction
ids and paths inside the Node containers; the Maker Node excerpt carries the actor's drive lines
for one swap and the sidecar errors the Node printed, whose messages the Node had already
redacted. No key, cookie, capability file, `runtime/` or `market/` content is included. The Node
logs the harness kept for the tries that did not pass are not committed beyond the 74-line
excerpt. The reset archive and the actor databases read for the diagnosis were copied out, read
and deleted, and nothing from them is here. No local path is in any file. Scanned before commit
with the repository's gitleaks configuration.

## Source

`15b1dc84666199cebb8e2bc1bfa97ace5406659a` (`fix(deploy): the regtest Bitcoin Core skips its
per-block index check`). It is `44e39081` plus `234c17db`, `ef0f1299` and `15b1dc84`. The
checkout the stack ran from was clean and detached at that commit for the image build and the run
(`regress.log` prints "15b1dc84 (tree dirty: 0)"). Trees at that commit
(`git rev-parse 15b1dc84:<directory>`):

| directory | tree |
|---|---|
| `crates` | `ff30c1391a8e5d6e967ab9ca588ed5e581f4d6b3` |
| `compat` | `b64e1d8235d4896da57be65c0c7b7fe799d3f612` |
| `apps` | `5ba454f517f805edd5327439730c339c8f2cfeaf` |
| `modules` | `94e405f1338f4b29fb9f3bbf402c97a48c59ad70` |
| `deploy` | `d9ef4711f1e7e4c0bbb97320fbae0a0835d0acf0` |
| `packaging` | `d3990b01664590d0ee2e6eca22b1c7ff185a880b` |

`compat`, `apps`, `modules` and `packaging` are the trees of `44e39081`; `crates` and `deploy`
differ by the three commits.

## Images

Local linux/amd64 images built on this host; none pulled from a registry, no release built from
this commit. The Node binaries were built in `rust:1.96.0-bookworm` (development profile,
10:06Z) and packaged as `lez_maker_node` and `lez_taker_node` 0.4.0. Ids read with
`docker image inspect` after the run:

| image | id | created (UTC) |
|---|---|---|
| `lez-maker-node:local` | `sha256:20e6a5c9c863a5c78a232a570ccdc7cb0b61f7e3aae34dec5f4685e6457f61f1` | 2026-11-18 10:11 |
| `lez-taker-node:local` | `sha256:bc7e12cdd4d21d6a7d498e5388ed2c0835c4d33edb29f6ce2e89a73b3e13bc20` | 2026-11-18 10:11 |
| `lez-network-module:local` | `sha256:6209302a81c183351ee1c69a931f14d6a8bd98f8af697b208f4a0d23b0de9e3e` | 2026-11-16 16:27 |
| `lez-basecamp-ui:local` | `sha256:83a81887e9462ce88a1f9607c4bbb627016ab8f66673c1838eaf09e741f2e34b` | 2026-11-16 16:27 |
| `lez-services:local` | `sha256:021a7928e64602ce785ce464a83df7e9c27588391c24f6fd3d048489f7d52bef` | 2026-11-13 18:33 |
| `lez-zebra:local` | `sha256:42d64e9a9e85dbd24911f26b2676f4ca07f3c7d910e80dfb99dfb4ca49ecc101` | 2026-11-11 11:50 |
| `lez-bitcoin-core:local` | `sha256:dadb2fd79b64a6e20612440c90146d6501a1b3d2039408760e93d48c33619cc5` | 2026-11-11 15:01 |
| `lez-btc-miner:local` | `sha256:4a33449ce116b239641c7932c63845e2d7c9a54e0d00f39d19688e7c3e403473` | 2026-11-11 15:01 |
| `lez-monerod:local` | `sha256:db4f4b5620a34a5634bd31d4ab9fc8f92b3ce00139f586828287f41e3ce1d9cf` | 2026-11-13 15:13 |

The Node images were built before the run and not rebuilt during it or for the reruns; the Node
containers of all three lanes ran the two Node ids above (`docker inspect`, read for the reruns).
`lez-network-module` and `lez-basecamp-ui` carry the creation time of a cached build. The
`lez-bitcoin-core` image is unchanged: `15b1dc84` changes the Core's command line in
`deploy/compose.yaml`, not the image.

Build source: `/usr/local/share/lez/build-source.txt` read
`15b1dc84666199cebb8e2bc1bfa97ace5406659a` for the Maker and the Taker right before the run (after
up.sh's end, before the run's start at 10:14:19Z), and again in all six Node containers before
the reruns. Bitcoin Core: the arguments of the three lanes' Cores (`docker inspect`) held
`-checkblockindex=0`, checked at the same two moments.

## The run

- Host: Linux 6.8 x86_64, 8 cores, Docker 29.1.3. Other projects' containers share it.
- From the checkout's `deploy/`: the Rust build, staging and Node images (10:06Z to 10:11Z),
  `scripts/up.sh --fresh-lez` with Monero and three lanes (new LEZ chain, genesis 2026-11-18
  10:12:04Z; lanes 2 and 3 up at 10:14:12Z; desks skipped), the two checks above, then
  `LEZ_MONERO=1 LEZ_LANES=3 scripts/regress.sh full` (`20261118T101419Z`, 10:14:19Z to
  13:55:56Z, 221 min). Its console output is not committed: it holds host paths.
- The Bitcoin actors' trace (`LEZ_BTC_ACTOR_TRACE=1`) was on in the devnet's `runtime.env`.
- Lanes: lane 1 is the stack; lanes 2 and 3 are one more Maker/Taker pair each with their own
  Bitcoin Core, miner, Zebra, `monerod` and wallet-rpc, on the stack's one LEZ network.
  `results.tsv` names the lane of every try.
- LEZ chains: the chain `up.sh` made served every test of the run but two; both
  `zec-balance-refusal` tests ran at once on one new chain (lanes 1 and 2, the last two tests).
  0 frozen tries.
- Groups (`regress.log`): `fast` from 10:14:49Z, `zec-v6` from 12:52:59Z, `refund` from
  13:29:49Z, the own-chain pair from 13:50:21Z.
- The harness kept both Nodes' logs for the failed try and the two NOT-EXERCISED tries under
  the run's `logs/<try>.nodes/`. The excerpt in `logs/` is lines 19,677 to 19,750 of the lane-3
  Maker's `maker-node.log` of the failed try (its last 20,000 lines), unchanged; the 250 lines
  after it repeat its last line.
- The diagnosis read, after the run: the lane-3 Maker's reset archive `20261118T133055Z` in its
  state volume (`maker.sqlite3` and the swap's sidecar directory) and the swap's Zcash actor
  directory in the lane's Zcash state volume (`actor.sqlite3`, `bridge.sqlite3`), copied to a
  scratch directory, read with `sqlite3` and `json`, and deleted; and each lane's Maker
  `node.log` in its socket volume, searched in place.
- The reruns: a new LEZ chain (`up.sh --fresh-lez` at 14:02Z, same images, same three lanes),
  then five times `LEZ_MONERO=1 LEZ_LANES=3 scripts/regress.sh run` of the three tests named in
  the README: runs `20261118T140515Z`, `20261118T141226Z`, `20261118T141853Z`,
  `20261118T142536Z` and `20261118T143218Z`, each printing "15b1dc84 (tree dirty: 0)". Their run
  directories are not committed.
