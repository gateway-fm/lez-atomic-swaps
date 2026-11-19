# Provenance — full regression at `9a15d94b`, 2026-11-19

Everything in this folder was produced on one host by the Docker devnet of `deploy/`: every
test of `scripts/regress.sh full` (159: `regress.sh list full`), the Node scenarios by
`scripts/node-e2e.py` and the desk journeys by `scripts/ui-e2e.sh`, three at a time on three
regression lanes, with the Monero regtest chain up (`LEZ_MONERO=1`).

Nothing here is private. `results.tsv` and `regress.log` carry test names, verdicts, times,
lanes and LEZ heights. No key, cookie, capability file, database, `runtime/` or `market/`
content is included. The per-test logs and the Nodes' `node.log` files read for the README are
not committed; the three Maker `node.log` files (and the three Taker ones) were copied out of
the containers to a scratch directory, searched and deleted. No local path is in any file.
Scanned before commit with the repository's gitleaks configuration.

## Source

`9a15d94b38f13fc0c82614c5bba499e2efaf75e9` (`docs(evidence): the full regression at 10a785ad,
159 of 159 in 206 min`). Its code is that of `8271d630` (`fix(sidecar): a submission refused
before any broadcast can be tried again under its request id`): `git diff --stat 8271d630
9a15d94b` touches only `docs/evidence/regression-m4-2026-11-18-c/` and
`docs/evidence/regression-m4-2026-11-18-d/`. The history from `main` is:

- `f94b5018`, the merge of `main` at `8903241b` into the Monero delivery work at `7807ce3b`
  (version 0.4.0 kept in the release-version files);
- `5a938e9c`, `9976f9a3` and `8271d630`, the same changes as `234c17db`, `15b1dc84` and
  `10a785ad`;
- `347de2de` and `9a15d94b`, the two evidence notes of 2026-11-18 (`-c` and `-d`) unchanged,
  docs only.

The checkout the stack ran from was clean and detached at `9a15d94b` for the image build and the
run (`regress.log` prints "9a15d94b (tree dirty: 0)"). Trees at `8271d630` (`git rev-parse
8271d630:<directory>`; the same at `9a15d94b`):

| directory | tree |
|---|---|
| `crates` | `a3242a3632f0880d7057b599b57bdd017a44c15c` |
| `compat` | `c3cd7cad15a84a5317ca69fbf4cd0c790a6838a2` |
| `apps` | `5ba454f517f805edd5327439730c339c8f2cfeaf` |
| `modules` | `94e405f1338f4b29fb9f3bbf402c97a48c59ad70` |
| `deploy` | `0d22a77b5611989e515d15688d7615f97b38b4f8` |
| `packaging` | `d3990b01664590d0ee2e6eca22b1c7ff185a880b` |

`compat`, `apps`, `modules` and `packaging` are the trees of `10a785ad`; `crates` and `deploy`
differ by what `main` brought (the fix for #156, the explorer images, the release-bundle test).

## Images

Local linux/amd64 images built on this host; none pulled from a registry, no release built from
this commit. The Node binaries were built in a development profile and packaged as
`lez_maker_node` and `lez_taker_node` 0.4.0, with `scripts/from-scratch.sh --only` for
`build:sidecar`, `build:swap-node-module`, `build:role-node-modules`, `rust`, `nix` and `stage`
(the two module steps and `nix` found their outputs current), then the compose build of
`basecamp-ui`, `network`, `maker-node` and `taker-node`. Ids read with `docker image inspect`
after the run:

| image | id | created (UTC) |
|---|---|---|
| `lez-maker-node:local` | `sha256:b0b9e326f4359ab2389bea53460e0db547b4bf3aadb1a6728a672253e7f6de46` | 2026-11-19 03:51 |
| `lez-taker-node:local` | `sha256:ba6017610deec6e0de6792a217110fdbc3d2cefa389c2f19d95c1b90e94019b9` | 2026-11-19 03:51 |
| `lez-network-module:local` | `sha256:cc90a4c1ea375b01b1165b43eb34e76c071f41b4105e12d385381d71b5f443fa` | 2026-11-16 16:27 |
| `lez-basecamp-ui:local` | `sha256:ad42cbc539c78d0f2909831533a2eee93c556b6549174269af9573e479c44659` | 2026-11-19 06:49 |
| `lez-services:local` | `sha256:021a7928e64602ce785ce464a83df7e9c27588391c24f6fd3d048489f7d52bef` | 2026-11-13 18:33 |
| `lez-zebra:local` | `sha256:42d64e9a9e85dbd24911f26b2676f4ca07f3c7d910e80dfb99dfb4ca49ecc101` | 2026-11-11 11:50 |
| `lez-bitcoin-core:local` | `sha256:dadb2fd79b64a6e20612440c90146d6501a1b3d2039408760e93d48c33619cc5` | 2026-11-11 15:01 |
| `lez-btc-miner:local` | `sha256:4a33449ce116b239641c7932c63845e2d7c9a54e0d00f39d19688e7c3e403473` | 2026-11-11 15:01 |
| `lez-monerod:local` | `sha256:db4f4b5620a34a5634bd31d4ab9fc8f92b3ce00139f586828287f41e3ce1d9cf` | 2026-11-13 15:13 |

The Node images were built before the run and not rebuilt during it; the Node containers of all
three lanes ran the two Node ids above (`docker inspect`, read after the run).
`lez-network-module` carries the creation time of a cached build. `lez-basecamp-ui:local` was
tagged anew at 06:49 during the run, from the same clean checkout; which step of the run
re-checked its build is not recorded. No container of the run used that image: the desks run in
the Node containers, whose images were not rebuilt.

Build source: `/usr/local/share/lez/build-source.txt` read
`9a15d94b38f13fc0c82614c5bba499e2efaf75e9` in all six Node containers right after `up.sh` (before
the smoke run), in the lane-1 Maker and Taker right before the full run's start at 04:25:57Z,
and after the run in the four running Node containers (lane 3's two had stopped; their
containers ran the two Node ids above). Bitcoin Core: the arguments of the three lanes' Cores
(`docker inspect`) held `-checkblockindex=0`, checked after `up.sh` and after the run.

## The run

- Host: Linux 6.8 x86_64, 8 cores, Docker 29.1.3. Other projects' containers share it.
- From the checkout's `deploy/` at `9a15d94b`, with the images above:
  `SKIP_UI_VERIFY=1 LEZ_NO_BANNER=1 LEZ_MONERO=1 LEZ_LANES=3 bash scripts/up.sh --fresh-lez`
  (new LEZ chain, genesis 2026-11-19 03:52:41Z; lanes 2 and 3 up at 03:54:50Z; desks skipped),
  then `regress.sh smoke` (`20261119T035504Z`, 6 of 6) and a targeted run of eleven tests
  (`20261119T035813Z`, 11 of 11) on the same chain, then
  `LEZ_MONERO=1 LEZ_LANES=3 bash scripts/regress.sh full` (`20261119T042557Z`, 04:25:57Z to
  08:01:17Z, 215 min, exit 0). Its console output is not committed: it holds host paths.
- The Bitcoin actors' trace (`LEZ_BTC_ACTOR_TRACE=1`) was on in the devnet's `runtime.env`.
- Lanes: lane 1 is the stack; lanes 2 and 3 are one more Maker/Taker pair each with their own
  Bitcoin Core, miner, Zebra, `monerod` and wallet-rpc, on the stack's one LEZ network.
  `results.tsv` names the lane of every try.
- LEZ chains: the chain `up.sh` made served every test of the run but two; both
  `zec-balance-refusal` tests ran at once on one new chain (lanes 1 and 2, the last two tests).
  0 frozen tries.
- Groups (`regress.log`): `fast` from 04:26:28Z, `zec-v6` from 07:09:20Z, `refund` from
  07:35:14Z, the own-chain pair from 07:55:33Z.
- One try ended other than PASSED (`node:zec:zec-claim-collision:TakerSellsForeign`, try 1,
  NOT-EXERCISED); the harness kept both Nodes' logs for it under the run's `logs/`. They are not
  committed.
- Read after the run for the README: the per-test logs of the run, and each lane's Maker
  `node.log` in its socket volume, copied out of the container, searched and deleted.
