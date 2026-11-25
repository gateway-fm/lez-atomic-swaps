# Provenance: full regression at `957c8016`, 2026-11-24/25, macOS arm64

One operator ran both parties, Maker and Taker, on one host. Everything in this folder was
produced on that host by the Docker devnet of `deploy/`:

- every test of `scripts/regress.sh full` (179: `regress.sh list full`);
- the Node scenarios, by `scripts/node-e2e.py`;
- the desk journeys, by `scripts/ui-e2e.sh`.

They ran two at a time on two regression lanes, with the Monero regtest chain up
(`LEZ_MONERO=1`) and the devnet test hooks built in (`LEZ_TEST_HOOKS=1`).

## Source

`957c8016a57500476ead4c12fce11a059463222f` (`devnet: a new chain's unreachable indexer is
polled, not fatal`), a detached checkout with the three submodules at their pinned commits. The
checkout was clean for the image build and for both runner invocations: each `regress.log`
prints "957c8016 (tree dirty: 0)".

`git diff --name-only 27ea6231 957c8016` lists 18 files (`diff-27ea6231-957c8016.txt`). None of
them is under a shipped crate's `src/`, under `compat/*/src` or under `apps/`. The first
attempt (`first-attempt-27ea6231/`) ran on images built from `27ea6231`.

This note is committed on top of `957c8016`.

## Images

All images are local linux/arm64 images, built on this host from the clean checkout. None was
pulled from a registry, and no release was built from this commit. They were built by
`scripts/from-scratch.sh --workspace <workspace> --only <phase>` for the phases `host`,
`sources`, `nix`, `rust`, `build`, `stage` and `stack`, in that order, with `LEZ_MONERO=1
LEZ_TEST_HOOKS=1 LEZ_TIMING_PROFILE=fast`, from 2026-11-24 21:36:39Z to 21:53:43Z. They were
first built for `27ea6231` from 17:12Z, after the stack of an earlier checkout had been taken
down with `down.sh --wipe` and `lanes.sh down 2 --wipe`. The ids below were read with `docker
image inspect` after the run:

| image | id | created (UTC) |
|---|---|---|
| `lez-maker-node:local` | `sha256:3f0376f8dfe75a5e9e21e9c4d7d36aa1c264a2f846aa5c029085d9b19aeb9507` | 2026-11-24 21:46 |
| `lez-taker-node:local` | `sha256:f5578d21a19333f4200e91bbddb11d956f6e71d6f816864a5bf1873fc333dbad` | 2026-11-24 21:46 |
| `lez-network-module:local` | `sha256:6f573b6e6a56b8ccb3ea82f5d3aa5f3e8745c74049003d3f3468cddd9fa8c088` | 2026-11-24 18:13 |
| `lez-basecamp-ui:local` | `sha256:fe7dc01a1f7b873a368608d0416539bf8f9cf824ba6ddb14d3847873047ead92` | 2026-11-24 18:13 |
| `lez-wallet-module:local` | `sha256:fc91ca78b3a9f518c33a621fcfe90f8156a2cd5dfdbf975c6252e706de4226a0` | 2026-11-11 21:52 |
| `lez-services:local` | `sha256:c0fc24772fc913660ed52f44c0184f6affd82c8d288ddf3616ee0009c11ebc37` | 2026-11-24 18:06 |
| `lez-zebra:local` | `sha256:f73eccf3087d5d8873c3fb911b852407fc9e6fbd8af5ebffcc74ee5897d93d6a` | 2026-11-24 18:05 |
| `lez-bitcoin-core:local` | `sha256:39e939eabe1cc7024b140b36464587f72e8b97d528162cf3849cb1479d653691` | 2026-11-18 16:42 |
| `lez-btc-miner:local` | `sha256:7dee784df4e5f1b1b8f44e12c092e239f95e4f8243f9fd42ae9622ff9917093a` | 2026-11-24 18:07 |
| `lez-monerod:local` | `sha256:798dfa37a106ab62eaa5d2a17b70cce40cce0330f2bc48441899880a4d2727f8` | 2026-11-24 18:16 |

The compose build took the images other than the two Nodes from its cache. Their inputs are
unchanged between `27ea6231` and `957c8016`.

**Build source.** `/usr/local/share/lez/build-source.txt` and `sidecar-source.txt` read
`957c8016a57500476ead4c12fce11a059463222f` in the lane-1 Maker and Taker. They were read after
the stack phase, before the run started at 21:54:13Z, and again after the run in all four Node
containers (lanes 1 and 2), which ran the two Node ids above.

**Hooks.** The Maker Node's executable carries `hold-zec-activations`, read before the run.

**Escrow.** The escrow the market bootstrap deployed (`bootstrap/deployment.json` of the market
root) has ImageID `ca66e0dc4118597e0a9010670fd49aec923bba9c332e9e6a26d9208dbdf6af98`, on the
stack's chain and again on the chain made at 02:43Z.

## The run

- **Host.** Apple-silicon Mac, macOS 26.7.3, 10 cores. Docker Desktop 29.9.2 with 10 CPUs and
  about 32 GB of memory for its VM. Other projects' stacks shared the VM throughout and were
  busy. Docker had 52.2 GB free at the start, 47.8 GB or more at every check during the run (every
15 minutes), and 47.9 GB at the end.
- **Stack.** The stack phase started a new LEZ chain and new Bitcoin, Zcash and Monero regtest
  chains, and deployed the escrow. `LEZ_TIMING_PROFILE=fast`. The Bitcoin actors' trace
  (`LEZ_BTC_ACTOR_TRACE=1`) was on in `runtime.env`. Lane 1 is the stack. Lane 2 is one more
  Maker/Taker pair with its own Bitcoin Core, miner, Zebra, `monerod` and wallet-rpcs, on the
  stack's one LEZ network, brought up by `regress.sh` itself (`lanes.sh up 2`).
- **Invocation 1.** From the checkout's `deploy/`: `LEZ_MONERO=1 LEZ_LANES=2 bash
  scripts/regress.sh full` (`20261124T215412Z`), 21:54:13Z to 02:41:37Z, exit 1 after 150
  passed. `LEZ_REGRESS_DURATIONS` pointed at the `results.tsv` of
  `regression-0.4.0-2026-11-23-macos-arm64` (this only orders the tests). `LEZ_WORKSPACE` and
  `LEZ_PROVISION` were not set, so the first new chain's market bootstrap found no escrow
  deployer (README).
- **Recovery.** `LEZ_MONERO=1 LEZ_LANES=2 LEZ_TIMING_PROFILE=fast` with `LEZ_WORKSPACE`,
  `LEZ_PROVISION`, `LEZ_MARKET_ROOT` and `LEZ_WALLET_IDENTITIES` naming the build's workspace:
  `bash scripts/up.sh --fresh-lez`, 02:43:11Z to 02:44:31Z, exit 0. Its output is
  `run-2-remaining/recovery-up-fresh-lez.log`.
- **Invocation 2.** The same environment: `bash scripts/regress.sh run` with the 29 tests that
  had no try (`20261125T024443Z`), 02:44:44Z to 04:05:12Z, exit 0.
- **Not committed.** The runners' console output holds host paths and repeats `regress.log`.
- Every try ended PASSED, so the harness kept no Node logs in either invocation.

## Privacy

Nothing here is private. The logs carry:

- step lines, swap and transaction ids, account ids, balances and LEZ heights;
- the stack's start-up lines, and the published offers and swap views;
- the public SHA-256 hash-lock digests of swaps.

`zec-maker-early-claim` presents a wrong preimage on purpose (zeros, then the digest itself). No
key, wallet seed, RPC password, cookie, capability file, database, `runtime/` or `market/`
content is included. The VNC password that `up.sh` prints is the devnet's documented default.

Local paths are replaced by `<checkout>`, `<workspace>`, `<home>` and `<scratch>`.
`first-attempt-27ea6231/docker-desktop-backend-excerpt.txt` holds only Docker Desktop's lines
about this stack's containers and the moments named in its README.

The folder was scanned before commit with gitleaks 8.30.1 (`gitleaks dir`, the repository's
`.gitleaks.toml`). Its four findings are all `generic-api-key` on the public digest
`767dd59e…0ee3` of `zec-maker-early-claim`:

- `run-1-full/logs/node-zec-zec-maker-early-claim-TakerSellsLez-1.log`, lines 157 and 205;
- `run-1-full/e2e/zec-maker-early-claim.json`, line 32;
- `run-2-remaining/e2e/zec-maker-early-claim.json`, line 32.

The folder was also searched for key, seed, password, cookie, mnemonic and preimage patterns
and for local paths.
