# Provenance — full regression at `44e39081`, 2026-11-18

Everything in this folder was produced on one host by the Docker devnet of `deploy/`: every
test of `scripts/regress.sh full` (159: `regress.sh list full`), the Node scenarios by
`scripts/node-e2e.py` and the desk journeys by `scripts/ui-e2e.sh`, three at a time on three
regression lanes, with the Monero regtest chain up (`LEZ_MONERO=1`).

Nothing here is private. `results.tsv` and `regress.log` carry test names, verdicts, times,
lanes and LEZ heights; the failed test's log carries step lines with swap, offer and transaction
ids; the Maker Node excerpt carries the actor's drive lines and trace events for one swap. No
key, cookie, capability file, `runtime/` or `market/` content is included. The Node logs the
harness kept for the tries that did not pass (container, Node and Basecamp logs, health answers;
already filtered by the harness) are not committed beyond the eleven-line excerpt; the reset
archive read for the diagnosis was copied out, read and deleted, and nothing from it is here. No
local path is in any file. Scanned before commit with the repository's gitleaks configuration.

## Source

`44e39081c017d18b29bfca2b02d3b48b038f90ab` (`fix(maker-node): a Monero funding waits out a
monerod outage instead of expiring`). It carries `32c8bfb4`, the commit of the previous full run.
The checkout the stack ran from was clean and detached at that commit for the image build and the
run (`regress.log` prints "44e39081 (tree dirty: 0)"). Trees at that commit
(`git rev-parse 44e39081:<directory>`):

| directory | tree |
|---|---|
| `crates` | `8d52a16e053a3126d9a8dd75f36087430c3cb49b` |
| `compat` | `b64e1d8235d4896da57be65c0c7b7fe799d3f612` |
| `apps` | `5ba454f517f805edd5327439730c339c8f2cfeaf` |
| `modules` | `94e405f1338f4b29fb9f3bbf402c97a48c59ad70` |
| `deploy` | `675dd74db0233234de95af35fe5a61d2dd64f5fd` |
| `packaging` | `d3990b01664590d0ee2e6eca22b1c7ff185a880b` |

`compat`, `apps`, `modules` and `packaging` are the trees of `32c8bfb4`; `crates` and `deploy`
differ by the commits the README lists.

## Images

Local linux/amd64 images built on this host; none pulled from a registry, no release built from
this commit. The Node binaries were built in `rust:1.96.0-bookworm` (development profile) and
packaged as `lez_maker_node` and `lez_taker_node` 0.4.0. Ids read with `docker image inspect`
after the run:

| image | id | created (UTC) |
|---|---|---|
| `lez-maker-node:local` | `sha256:4f198fcdd856587af23a819cc490d5ca2191e3125658ccc7fd7bf35ed88278a3` | 2026-11-18 02:04 |
| `lez-taker-node:local` | `sha256:9a4dfa649c03b21b86cba748610d84d09581837b66b00e1be67acd07b2692d8b` | 2026-11-18 02:04 |
| `lez-network-module:local` | `sha256:f8503eac11b3aace8c78c9e9d36f13742ab58f7136cdeefc82302459a8298029` | 2026-11-16 16:27 |
| `lez-basecamp-ui:local` | `sha256:0a0d4fef7830a34dcbecd95a20eb63f947386d1778cbea0c6cfdb698c30c2d30` | 2026-11-16 16:27 |
| `lez-services:local` | `sha256:021a7928e64602ce785ce464a83df7e9c27588391c24f6fd3d048489f7d52bef` | 2026-11-13 18:33 |
| `lez-zebra:local` | `sha256:42d64e9a9e85dbd24911f26b2676f4ca07f3c7d910e80dfb99dfb4ca49ecc101` | 2026-11-11 11:50 |
| `lez-bitcoin-core:local` | `sha256:dadb2fd79b64a6e20612440c90146d6501a1b3d2039408760e93d48c33619cc5` | 2026-11-11 15:01 |
| `lez-btc-miner:local` | `sha256:4a33449ce116b239641c7932c63845e2d7c9a54e0d00f39d19688e7c3e403473` | 2026-11-11 15:01 |
| `lez-monerod:local` | `sha256:db4f4b5620a34a5634bd31d4ab9fc8f92b3ce00139f586828287f41e3ce1d9cf` | 2026-11-13 15:13 |

The Node images were built before the run (02:04Z) and not rebuilt during it; after the run the
Node containers of all three lanes (`lez-maker-node`, `lez-taker-node`, `lez-l2-*`, `lez-l3-*`,
read with `docker inspect`) ran the two Node ids above. `lez-network-module` and
`lez-basecamp-ui` carry the creation time of a cached build. The chain images are those of the
run of `32c8bfb4`.

Build source: `/usr/local/share/lez/build-source.txt` read
`44e39081c017d18b29bfca2b02d3b48b038f90ab` in both Node images right before the run (between
up.sh's end at 04:03:23Z and the run's start at 04:03:24Z).

## The run

- Host: Linux 6.8 x86_64, 8 cores, Docker 29.1.3. Other projects' containers share it.
- From the checkout's `deploy/`: `scripts/up.sh --fresh-lez` with Monero and three lanes (new
  LEZ chain, genesis 2026-11-18 04:01:09Z; lanes 2 and 3 up at 04:03:18Z; desks skipped), the
  build-source check above, then `LEZ_MONERO=1 LEZ_LANES=3 scripts/regress.sh full`
  (`20261118T040324Z`, 04:03:24Z to 07:51:43Z, 228 min). Its console output is not committed: it
  holds host paths.
- Just before (03:53Z), in a checkout of the same commit: the workspace's clippy (exit 0), the
  clippy of `lez-maker-node` and `lez-taker-node` with all features (exit 0), and the unit tests
  of the Monero Maker actor, 19 passed (exit 0).
- The Bitcoin actors' trace (`LEZ_BTC_ACTOR_TRACE=1`) was on in the devnet's `runtime.env`.
- Lanes: lane 1 is the stack; lanes 2 and 3 are one more Maker/Taker pair each with their own
  Bitcoin Core, miner, Zebra, `monerod` and wallet-rpc, on the stack's one LEZ network.
  `results.tsv` names the lane of every try.
- LEZ chains: the chain `up.sh` made served every test of the run but two; both
  `zec-balance-refusal` tests ran at once on one new chain (lanes 1 and 2, the last two tests).
  0 frozen tries.
- Groups (`regress.log`): `fast` from 04:03:56Z, `zec-v6` from 07:01:36Z, `refund` from
  07:24:14Z, the own-chain pair from 07:46:46Z.
- The harness kept both Nodes' logs for the failed try and the two NOT-EXERCISED tries under
  the run's `logs/<try>.nodes/` (582d6c47). The excerpt in `logs/` is the last eleven lines of
  the lane-2 Maker's `maker-node.log` of the failed try, unchanged.
- The diagnosis read the lane-2 Maker's reset archive `20261118T065643Z` in its state volume (the
  reset after the failed test): its `maker.sqlite3` copied to a scratch directory, read with
  `sqlite3` and deleted, and the file times of the swap's directory listed in place.
