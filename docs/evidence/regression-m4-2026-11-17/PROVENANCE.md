# Provenance — full regression at `7d09351a`, 2026-11-17

Everything in this folder was produced on one host by the Docker devnet of `deploy/`: every
test of `scripts/regress.sh full` (159: `regress.sh list full`), the Node scenarios by
`scripts/node-e2e.py` and the desk journeys by `scripts/ui-e2e.sh`, three at a time on three
regression lanes, with the Monero regtest chain up (`LEZ_MONERO=1`); and, right before it, the
four Taker-refund-offline tests on the same images and chain.

Nothing here is private. `results.tsv` and `regress.log` carry test names, verdicts, times,
lanes and LEZ heights; the logs carry step lines with swap, offer and transaction ids and paths
inside the Node containers. No key, cookie, capability file, `runtime/` or `market/` content is
included. No local path is in any file. Scanned before commit with the repository's gitleaks
configuration.

## Source

`7d09351a974cde542b7be3190affa8e0369c0311` (`test(node-e2e): a stopped Maker's missing Chat
socket also counts as cut`). It carries `main` at `6d87d958` (merged by `fdaf8264`) and
`39e60679`, the commit of the 2026-11-16 run. The checkout the stack ran from was clean and
detached at that commit for the image build, the four tests before the run and the run
(`regress.log` and `logs/before/20261116T235559Z-regress.log` print "7d09351a (tree dirty: 0)").
Trees at that commit (`git rev-parse 7d09351a:<directory>`):

| directory | tree |
|---|---|
| `crates` | `42e638cc1c2064fbe6479c09c3e75d876d5ea552` |
| `compat` | `b64e1d8235d4896da57be65c0c7b7fe799d3f612` |
| `apps` | `5ba454f517f805edd5327439730c339c8f2cfeaf` |
| `modules` | `94e405f1338f4b29fb9f3bbf402c97a48c59ad70` |
| `deploy` | `9a3e11b79e14f5d012fb3cbe8d77b93096b65ff9` |
| `packaging` | `d3990b01664590d0ee2e6eca22b1c7ff185a880b` |

## Images

Local linux/amd64 images built on this host; none pulled from a registry, no release built from
this commit. The Node binaries were built in `rust:1.96.0-bookworm` (development profile) and
packaged as `lez_maker_node` and `lez_taker_node` 0.4.0. The staged Nix-built modules (the LEZ
v0.2 sidecar, `lez_swap_node`, the desks, `lez_network`) are this host's earlier build of
`4a4a7fe6`, whose `crates`, `compat`, `apps`, `modules` and `packaging` trees are those of
`7d09351a` (the commits between change the root `Cargo.toml`'s development profile, `deploy/`
and docs). Ids read with `docker image inspect` after the run:

| image | id | created (UTC) |
|---|---|---|
| `lez-maker-node:local` | `sha256:10fe5eb7648345666eb580c4828829258f1b50c6697dfd3b18ec0ba028a7d88a` | 2026-11-16 23:52 |
| `lez-taker-node:local` | `sha256:4e851630d7c33f7d99754f7ca7e206d5d866b828a07ac79ba435aa1e0874fa27` | 2026-11-16 23:52 |
| `lez-network-module:local` | `sha256:9cb9322fc35bf784975d602c28cddc8ae9466fa88e4a26fd55283914d37593b3` | 2026-11-16 16:27 |
| `lez-services:local` | `sha256:021a7928e64602ce785ce464a83df7e9c27588391c24f6fd3d048489f7d52bef` | 2026-11-13 18:33 |
| `lez-zebra:local` | `sha256:42d64e9a9e85dbd24911f26b2676f4ca07f3c7d910e80dfb99dfb4ca49ecc101` | 2026-11-11 11:50 |
| `lez-bitcoin-core:local` | `sha256:dadb2fd79b64a6e20612440c90146d6501a1b3d2039408760e93d48c33619cc5` | 2026-11-11 15:01 |
| `lez-btc-miner:local` | `sha256:4a33449ce116b239641c7932c63845e2d7c9a54e0d00f39d19688e7c3e403473` | 2026-11-11 15:01 |
| `lez-monerod:local` | `sha256:db4f4b5620a34a5634bd31d4ab9fc8f92b3ce00139f586828287f41e3ce1d9cf` | 2026-11-13 15:13 |

The maker and taker ids are the ones the image build printed before the run, and the two tags
were last set at 23:53Z, before it. `lez-network-module` was rebuilt from cache to the same id
(its creation time is that of the cached build). `lez-basecamp-ui` (build only, the base of the
Node and network images; it runs no container) was built from cache to
`sha256:48d107511dc27720fe9f22bfe992e5b5088b093ed9b0968319ab72020105a753` before the run; its tag
was set again at 02:32:53Z, during the run, and now reads
`sha256:db511de4ab747e20470685c7d47b1adf1f22cdf081ecfe57fdc9bf70e21e028f`. Which call set it is
not recorded; at that time the Zcash catalog fixture recreated lane Nodes with a `compose up`
that does not pass `--no-build`, which builds the `basecamp-ui` service again from cache. The
Node images' tags were not set again, so every Node of the run ran from the ids above. `lez-services`, `lez-zebra`,
`lez-bitcoin-core`, `lez-btc-miner` and `lez-monerod` are the 2026-11-16 run's images.

Build source: `/usr/local/share/lez/build-source.txt` in both Node images read
`7d09351a974cde542b7be3190affa8e0369c0311` before the four tests and the run (23:55:57Z), read
with `docker run --rm --entrypoint cat` on `lez-maker-node:local` and `lez-taker-node:local`.

## The run

- Host: Linux 6.8 x86_64, 8 cores, Docker 29.1.3. Other projects' containers share it.
- One shell chain from the checkout's `deploy/`, its output in one console log (not committed:
  it holds the image build and host paths), in this order:
  `scripts/from-scratch.sh --only rust`; `scripts/from-scratch.sh --only stage`;
  `docker compose --env-file runtime/runtime.env --profile tools build basecamp-ui network
  maker-node taker-node`; `SKIP_UI_VERIFY=1 LEZ_NO_BANNER=1 LEZ_MONERO=1 LEZ_LANES=3
  scripts/up.sh --fresh-lez` (new LEZ chain, genesis 2026-11-16 23:53:51Z; lanes 2 and 3 up at
  23:55:52Z); the build-source check above; `LEZ_MONERO=1 LEZ_LANES=3 scripts/regress.sh run`
  with the four Taker-refund-offline tests (`20261116T235559Z`, 23:55:59Z to 00:08:55Z, 4 of 4);
  `LEZ_MONERO=1 LEZ_LANES=3 scripts/regress.sh full` (`20261117T000855Z`, 00:08:55Z to
  03:43:21Z, 214 min). Each step ran only if the one before succeeded; the chain ended with exit
  code 0.
- Lanes: lane 1 is the stack; lanes 2 and 3 are one more Maker/Taker pair each with their own
  Bitcoin Core, miner, Zebra, `monerod` and wallet-rpc, on the stack's one LEZ network.
  `results.tsv` names the lane of every try.
- LEZ chains: the chain `up.sh` made served the four tests and every test of the run but two;
  both `zec-balance-refusal` tests ran at once on one new chain (lanes 1 and 2, the last two
  tests). 0 frozen tries.
- Groups (`regress.log`): `fast` from 00:09:29Z, `zec-v6` from 02:49:43Z, `refund` from
  03:15:36Z, the own-chain pair from 03:37:34Z.
