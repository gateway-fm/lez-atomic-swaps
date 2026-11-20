# Provenance — full regression at `86bdb8a4`, 2026-11-20

Everything in this folder was produced on one host by the Docker devnet of `deploy/`: every
test of `scripts/regress.sh full` (160: `regress.sh list full`), the Node scenarios by
`scripts/node-e2e.py` and the desk journeys by `scripts/ui-e2e.sh`, three at a time on three
regression lanes, with the Monero regtest chain up (`LEZ_MONERO=1`).

Nothing here is private. `results.tsv` and `regress.log` carry test names, verdicts, times,
lanes and LEZ heights. No key, cookie, capability file, database, `runtime/` or `market/`
content is included. The per-test logs, the swap records under the run's `zec/` and the Nodes'
`node.log` files read for the README are not committed; the three Maker `node.log` files were
copied out of the containers to a scratch directory, searched and deleted. No local path is in
any file. Scanned before commit with the repository's gitleaks configuration.

## Source

`86bdb8a4c7766eee34df0f7b30090a606fd49d29` (`Merge the fixes for #180, #181, #130 and #151
into feat/m4-delivery`), parents `8d65f151` (the delivery work with `main` at `c87497a9` merged,
`a5228d9f`, and the Monero documents) and `b9855e04` (the issue fixes `b6882609`, `f324f445`,
`34b794d7`, `b9855e04`). `git diff --stat b9855e04 86bdb8a4` touches documents,
`deploy/README.md` and `packaging/systemd/lez-maker-node-monero.conf` (a comment) only.

The checkout the stack ran from was clean and detached at `86bdb8a4` for the image build and the
run (`regress.log` prints "86bdb8a4 (tree dirty: 0)"). Trees at `86bdb8a4`
(`git rev-parse 86bdb8a4:<directory>`):

| directory | tree |
|---|---|
| `crates` | `aa31775b32cde47c228af477927a7e915a360768` |
| `compat` | `afc0a9c02a3b9ca76ece050cf55902b87f148b1e` |
| `apps` | `a16144e6185ebe5b1fcee64739dbc3474b1cae7d` |
| `modules` | `739d2c871f0f5c2b4fcfd96ea0bf07be5f4aa455` |
| `deploy` | `a9f61d9019876520c0e223796a3f27624d48e1ec` |
| `packaging` | `57e67123c96fff77d35b041e4eb5aac5a9015cad` |

`crates`, `compat`, `apps` and `modules` are the trees of `b9855e04`; `deploy` and `packaging`
differ from it by the documentation lines above.

## Images

Local linux/amd64 images built on this host; none pulled from a registry, no release built from
this commit. From the clean checkout at `86bdb8a4`, the build's console shows the `rust` step of
`scripts/from-scratch.sh` (19:05:09Z to 19:05:27Z; the Node binaries in a development profile),
the role modules packaged as `lez_maker_node` and `lez_taker_node` 0.4.0, the `stage` step (to
19:09:27Z), then the compose build of `basecamp-ui`, `network`, `maker-node` and `taker-node`.
Ids read with `docker image inspect` after the run:

| image | id | created (UTC) |
|---|---|---|
| `lez-maker-node:local` | `sha256:1beb2f304224031fe90f2f15657e0317ef3fe51e3b385c806bae5303570013ad` | 2026-11-20 19:09 |
| `lez-taker-node:local` | `sha256:b5209386af6daaecbbdc405499b00e501bbe63629fcbbadaa6ad63b7ab0daa1c` | 2026-11-20 19:09 |
| `lez-network-module:local` | `sha256:aa9ac0a6db3cdaec754b3ef8d98b8b8056662d56804d18d64472181c3913909f` | 2026-11-20 19:09 |
| `lez-basecamp-ui:local` | `sha256:7542bfa526ecb5d2a8ac1628d28caa98bd38f54fe778da97fdf8a4d29a436db7` | 2026-11-20 18:05 |
| `lez-wallet-module:local` | `sha256:6303be19b16a52356b757f55635d0a6106ab961140d6db13ee858e3473201c68` | 2026-11-20 17:47 |
| `lez-services:local` | `sha256:021a7928e64602ce785ce464a83df7e9c27588391c24f6fd3d048489f7d52bef` | 2026-11-13 18:33 |
| `lez-zebra:local` | `sha256:42d64e9a9e85dbd24911f26b2676f4ca07f3c7d910e80dfb99dfb4ca49ecc101` | 2026-11-11 11:50 |
| `lez-bitcoin-core:local` | `sha256:dadb2fd79b64a6e20612440c90146d6501a1b3d2039408760e93d48c33619cc5` | 2026-11-11 15:01 |
| `lez-btc-miner:local` | `sha256:4a33449ce116b239641c7932c63845e2d7c9a54e0d00f39d19688e7c3e403473` | 2026-11-11 15:01 |
| `lez-monerod:local` | `sha256:db4f4b5620a34a5634bd31d4ab9fc8f92b3ce00139f586828287f41e3ce1d9cf` | 2026-11-13 15:13 |

The Node images were built before the run and not rebuilt during it; the Node containers of all
three lanes ran the two Node ids above (`docker inspect`, read after the run). The compose build
reported `lez-basecamp-ui:local` built from its cache: its creation time is that of the earlier
build with the same `apps` tree. `lez-wallet-module:local` runs the finality heartbeat.

Build source: `/usr/local/share/lez/build-source.txt` read
`86bdb8a4c7766eee34df0f7b30090a606fd49d29` in the lane-1 Maker and Taker right after `up.sh` and
before the run's start at 19:12:49Z, and after the run in the four running Node containers
(lanes 1 and 2; lane 3's two had stopped, and their containers ran the two Node ids above).
Bitcoin Core: the arguments of the three lanes' Cores (`docker inspect`) held
`-checkblockindex=0`, checked right after `up.sh` and after the run.

## The run

- Host: Linux 6.8 x86_64, 8 cores, Docker 29.1.3. Other projects' containers share it.
- From the checkout's `deploy/` at `86bdb8a4`, with the images above:
  `SKIP_UI_VERIFY=1 LEZ_NO_BANNER=1 LEZ_MONERO=1 LEZ_LANES=3 bash scripts/up.sh --fresh-lez`
  (from 19:10:11Z; new LEZ chain, genesis 2026-11-20 19:10:34Z; lanes 2 and 3 up at 19:12:41Z;
  desks skipped), then at once `LEZ_MONERO=1 LEZ_LANES=3 bash scripts/regress.sh full`
  (`20261120T191249Z`, 19:12:49Z to 22:57:55Z, 225 min, exit 0). Its console output is not
  committed: it holds host paths.
- The Bitcoin actors' trace (`LEZ_BTC_ACTOR_TRACE=1`) was on in the devnet's `runtime.env`.
- Lanes: lane 1 is the stack; lanes 2 and 3 are one more Maker/Taker pair each with their own
  Bitcoin Core, miner, Zebra, `monerod` and wallet-rpc, on the stack's one LEZ network.
  `results.tsv` names the lane of every try.
- LEZ chains: the chain `up.sh` made served every test of the run but two; both
  `zec-balance-refusal` tests ran at once on one new chain (lanes 1 and 2, the last two tests).
  0 frozen tries.
- Groups (`regress.log`): `fast` from 19:13:25Z, `zec-v6` from 21:59:28Z, `refund` from
  22:32:23Z, the own-chain pair from 22:52:39Z.
- Every try ended PASSED; the harness kept no Node logs for a failed or not-exercised try.
- Read after the run for the README: the per-test logs of the run, the swap records under the
  run's `zec/`, and each lane's Maker `node.log` in its socket volume, copied out of the
  container, searched and deleted.
