# Provenance — full regression of `feat/m4-monero` at `39e60679`, 2026-11-16

Everything in this folder was produced on one host by the Docker devnet of `deploy/`: every
test of `scripts/regress.sh full` (159: `regress.sh list full`), the Node scenarios by
`scripts/node-e2e.py` and the desk journeys by `scripts/ui-e2e.sh`, three at a time on three
regression lanes, with the Monero regtest chain up (`LEZ_MONERO=1`); then three runs of the one
failed test on the same images.

Nothing here is private. `results.tsv` and `regress.log` carry test names, verdicts, times,
lanes and LEZ heights; the logs carry step lines with swap, offer and transaction ids. No key,
cookie, capability file, `runtime/` or `market/` content is included; the Maker's swap state the
README quotes was read on the host and is not committed. No local path is in any file. Scanned
before commit with the repository's gitleaks configuration.

## Source

`39e60679` (`perf(xmr-reference-actor): the Maker's Monero funding opens the role wallet once`),
branch `feat/m4-monero`, on `feat/m5-gaps` at `e65626bc`. The checkout the stack ran from was
clean and detached at that commit for the run and the reruns (`regress.log` and each rerun's
runner output print "39e60679 (tree dirty: 0)"). Trees at that commit
(`git rev-parse 39e60679:<directory>`):

| directory | tree |
|---|---|
| `crates` | `31aaaf5c63674e857b5d39b25cb56d9fe375b8cf` |
| `compat` | `558d7179bc5773d902030c62d3aa78f2a68b8854` |
| `apps` | `c434c3d87ceaa23293012962d5d7ffe794590e65` |
| `modules` | `fb20ff5b490faac40a1ff718cc8d932d8e1c06a1` |
| `deploy` | `eff7818ea659a170683ffccfbdadd430dfe3702f` |
| `packaging` | `d3990b01664590d0ee2e6eca22b1c7ff185a880b` |

## Images

Local linux/amd64 images built on this host; none pulled from a registry, no release built from
this commit. Read with `docker image inspect` after the run; not rebuilt during the run or the
reruns.

| image | id | created (UTC) |
|---|---|---|
| `lez-maker-node:local` | `sha256:0dde6675ac061696f952b727d4e7cf811a29c2bc0ad47a57bf854633bd13a11c` | 2026-11-16 08:13 |
| `lez-taker-node:local` | `sha256:e22d27661316db70bce157598c4c9f9e8fbee2bd5a558381d16ae459cafbef98` | 2026-11-16 08:14 |
| `lez-basecamp-ui:local` | `sha256:0076d5796726e5813b8368f2d53d7a25315c5df5053e0bcca34647fdc9f194e0` | 2026-11-14 11:55 |
| `lez-network-module:local` | `sha256:a12a1be04a4ca995067a30683251d6b17f902c22311a645d432f0a8cff4beee5` | 2026-11-14 13:37 |
| `lez-services:local` | `sha256:021a7928e64602ce785ce464a83df7e9c27588391c24f6fd3d048489f7d52bef` | 2026-11-13 18:33 |
| `lez-zebra:local` | `sha256:42d64e9a9e85dbd24911f26b2676f4ca07f3c7d910e80dfb99dfb4ca49ecc101` | 2026-11-11 11:50 |
| `lez-bitcoin-core:local` | `sha256:dadb2fd79b64a6e20612440c90146d6501a1b3d2039408760e93d48c33619cc5` | 2026-11-11 15:01 |
| `lez-btc-miner:local` | `sha256:4a33449ce116b239641c7932c63845e2d7c9a54e0d00f39d19688e7c3e403473` | 2026-11-11 15:01 |
| `lez-monerod:local` | `sha256:db4f4b5620a34a5634bd31d4ab9fc8f92b3ce00139f586828287f41e3ce1d9cf` | 2026-11-13 15:13 |

Build source: both Node containers' `/usr/local/share/lez/build-source.txt` read
`39e60679086190866676df3ad16d0c668e753afb` before the run (the supervisor's check at
09:23Z) and again before the reruns (15:03Z). The `basecamp-ui` and `network` images carry no
build-source file; they keep the creation time of an earlier build (their ids differ from the
2026-11-15 run's). `lez-services`, `lez-zebra`, `lez-bitcoin-core`, `lez-btc-miner` and
`lez-monerod` are the 2026-11-15 run's images.

## The run

- Host: Linux 6.8 x86_64, 8 cores, Docker 29.1.3. Other projects' containers share it.
- `up.sh --fresh-lez` (`LEZ_MONERO=1`, `LEZ_LANES=3`) made the chain the run started on
  (genesis 2026-11-16 09:24:15Z); `LEZ_MONERO=1 LEZ_LANES=3 scripts/regress.sh full` ran from
  09:27:10Z to 14:57:02Z (329 min).
- Lanes: lane 1 is the stack; lanes 2 and 3 are one more Maker/Taker pair each with their own
  Bitcoin Core, miner, Zebra, `monerod` and wallet-rpc, on the stack's one LEZ network.
  `results.tsv` names the lane of every try.
- LEZ chains: the starting chain served every test but two; `zec-balance-refusal` in each
  direction starts a chain of its own (the last two tests). 0 frozen tries.
- Before the run, on the same images and another chain, three runs of `mixed-three-pair` with
  both `xmr-concurrent` tests on three lanes (`20261116T081719Z`, `20261116T082555Z`,
  `20261116T083457Z`, 3 of 3 each) and five Monero scenarios (`20261116T084334Z`, 5 of 5)
  passed; those runs are not committed here.


## The reruns

The three reruns of `node:btc:restart-maker:TakerSellsForeign` (`20261116T150357Z`,
`20261116T150645Z`, `20261116T150946Z`, 15:03:57Z to 15:12:37Z) ran from the same clean checkout
at `39e60679` on the same images, after the build-source check above, on the chain the run had
ended on (the second `zec-balance-refusal` chain), with `LEZ_MONERO=1 LEZ_LANES=3`.
