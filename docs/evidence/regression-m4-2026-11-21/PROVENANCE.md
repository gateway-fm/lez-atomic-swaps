# Provenance — full regression at `d4475300`, 2026-11-21

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

`d44753003b39a0157df9a2936f3c263154232105` (`Merge the desk caption, punish wording, recording
and test follow-ups into feat/m4-delivery`), parents `dd87b5c6` (the DLEQ cross-check
`35696dcd`, the proof-encoding fix `3e2d984b` and their documents, on top of `86bdb8a4` and its
two evidence notes) and `6799c9f5` (`bb4273bd`, `c77298db`, `6320be53`, `6799c9f5`).

The checkout the stack ran from was clean and detached at `d4475300` for the image build and the
run (`regress.log` prints "d4475300 (tree dirty: 0)"). Trees at `d4475300`
(`git rev-parse d4475300:<directory>`):

| directory | tree |
|---|---|
| `crates` | `24472d7fbaae852a0e4ee6e2dea4da0cad4e416c` |
| `compat` | `afc0a9c02a3b9ca76ece050cf55902b87f148b1e` |
| `apps` | `f90082a92da14ea709a9c6486d8c9947e5b4c048` |
| `modules` | `739d2c871f0f5c2b4fcfd96ea0bf07be5f4aa455` |
| `deploy` | `9ca1143ab2c245d222593a623712bbb211169251` |
| `packaging` | `57e67123c96fff77d35b041e4eb5aac5a9015cad` |

`compat`, `modules` and `packaging` are the trees of `86bdb8a4`; `crates`, `apps` and `deploy`
differ from it by the commits the README lists.

This note is committed on top of `24875fae`, which is `d4475300` with one line of
`docs/architecture/system-architecture.md` changed (a semicolon in a Mermaid note, README "Lint
and unit tests"). The run and the checks were made on `d4475300`.

## Images

Local linux/amd64 images built on this host; none pulled from a registry, no release built from
this commit. They were built from the clean checkout at `d4475300` for the smoke run of that
commit, and the full run used them unchanged. The build's console shows the `rust` step of
`scripts/from-scratch.sh` (from 01:22:42Z; the Node binaries in a development profile, staged at
01:25:04Z), the `nix` step (done at 01:25:05Z), the role modules packaged as `lez_maker_node` and
`lez_taker_node` 0.4.0, the `stage` step (to 01:29:05Z), then the compose build of
`basecamp-ui`, `network`, `maker-node` and `taker-node`. Ids read with `docker image inspect`
after the run:

| image | id | created (UTC) |
|---|---|---|
| `lez-maker-node:local` | `sha256:098afd44d20ead1e8fa2add64d8d79da36e145065a938eb577e297e7163b3ac5` | 2026-11-21 01:29 |
| `lez-taker-node:local` | `sha256:39894c5aca1d8d8ff9ab5b4bfb16797009c8a6e61f234a6ef70bf8747e189c4a` | 2026-11-21 01:29 |
| `lez-network-module:local` | `sha256:05c7e25d23d11779b5ac2e09ce8bb57383a87043a15be16d78c2a0fa3754c717` | 2026-11-21 00:47 |
| `lez-basecamp-ui:local` | `sha256:3b9d6e96cb0370f08ed1cf803c5eb0561a82b3f780ba07afa97235326b2d24e9` | 2026-11-21 00:47 |
| `lez-wallet-module:local` | `sha256:6303be19b16a52356b757f55635d0a6106ab961140d6db13ee858e3473201c68` | 2026-11-20 17:47 |
| `lez-services:local` | `sha256:021a7928e64602ce785ce464a83df7e9c27588391c24f6fd3d048489f7d52bef` | 2026-11-13 18:33 |
| `lez-zebra:local` | `sha256:42d64e9a9e85dbd24911f26b2676f4ca07f3c7d910e80dfb99dfb4ca49ecc101` | 2026-11-11 11:50 |
| `lez-bitcoin-core:local` | `sha256:dadb2fd79b64a6e20612440c90146d6501a1b3d2039408760e93d48c33619cc5` | 2026-11-11 15:01 |
| `lez-btc-miner:local` | `sha256:4a33449ce116b239641c7932c63845e2d7c9a54e0d00f39d19688e7c3e403473` | 2026-11-11 15:01 |
| `lez-monerod:local` | `sha256:db4f4b5620a34a5634bd31d4ab9fc8f92b3ce00139f586828287f41e3ce1d9cf` | 2026-11-13 15:13 |

The Node images were built before the smoke run and not rebuilt before or during the full run;
the Node containers of all three lanes ran the two Node ids above (`docker inspect`, read after
the run). The compose build reported `lez-basecamp-ui:local` and `lez-network-module:local` built
from its cache: their creation time is that of an earlier build of the same inputs. The Maker
desk package staged for that build carries `bb4273bd`'s punish wording (read from its plugin
after the run). `lez-wallet-module:local` runs the finality heartbeat.

Build source: `/usr/local/share/lez/build-source.txt` read
`d44753003b39a0157df9a2936f3c263154232105` in the lane-1 Maker and Taker right after `up.sh` and
before the run's start at 02:01:33Z, and after the run in all six Node containers (`docker cp`;
lanes 1 and 2 running, lane 3's two stopped). Bitcoin Core: the arguments of the three lanes'
Cores (`docker inspect`) held `-checkblockindex=0`, checked right after `up.sh` and after the run.

## The smoke run that preceded it

On the same images and another chain (`up.sh --fresh-lez` from 01:29:48Z, genesis
01:30:10Z), with the build source read as `d4475300` in the lane-1 Maker and Taker:
`regress.sh smoke` (`20261121T013219Z`), 6 of 6 in 3 minutes, then `regress.sh run` of five
Monero tests (`20261121T013537Z`), 5 of 5 in 14 minutes: `node:xmr:xmr-happy`,
`node:xmr:mixed-three-pair`, `ui:xmr:xmr-happy`, `node:xmr:xmr-concurrent` and
`node:xmr:xmr-taker-refund`, each TakerSellsLez, each on its first try. Those runs' files are not
committed.

## The run

- Host: Linux 6.8 x86_64, 8 cores, Docker 29.1.3. Other projects' containers share it.
- From the checkout's `deploy/` at `d4475300`, with the images above: `up.sh --fresh-lez` with
  the Monero chain and three lanes (from 01:58:50Z; new LEZ chain, genesis 2026-11-21 01:59:15Z;
  lanes 2 and 3 up at 02:01:26Z; desks skipped), then at once `LEZ_MONERO=1 LEZ_LANES=3 bash
  scripts/regress.sh full` (`20261121T020133Z`, 02:01:33Z to 05:42:36Z, 221 min, exit 0). Its
  console output is not committed: it holds host paths.
- The Bitcoin actors' trace (`LEZ_BTC_ACTOR_TRACE=1`) was on in the devnet's `runtime.env`
  (read after the run).
- Lanes: lane 1 is the stack; lanes 2 and 3 are one more Maker/Taker pair each with their own
  Bitcoin Core, miner, Zebra, `monerod` and wallet-rpc, on the stack's one LEZ network.
  `results.tsv` names the lane of every try.
- LEZ chains: the chain `up.sh` made served every test of the run but two; both
  `zec-balance-refusal` tests ran at once on one new chain (lanes 1 and 2, the last two tests).
  0 frozen tries.
- Groups (`regress.log`): `fast` from 02:02:22Z, `zec-v6` from 04:43:21Z, `refund` from
  05:16:55Z, the own-chain pair from 05:37:04Z.
- Every try ended PASSED; the harness kept no Node logs for a failed or not-exercised try.
- Read after the run for the README: the per-test logs of the run, the swap records under the
  run's `zec/`, and each lane's Maker `node.log` in its socket volume, copied out of the
  container, searched and deleted.
