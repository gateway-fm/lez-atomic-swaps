# Provenance: full regression at `23e54486` (Zebra module), 2026-11-25/26, macOS arm64

One operator ran both parties, Maker and Taker, on one host. Everything in this folder was
produced on that host by the Docker devnet of `deploy/`: every test of
`scripts/regress.sh full` (180: `regress.sh list full`), the Node scenarios by
`scripts/node-e2e.py` and the desk journeys by `scripts/ui-e2e.sh`. They ran four at a time on
four lanes, with the Monero regtest chain up (`LEZ_MONERO=1`) and the devnet test hooks built
in (`LEZ_TEST_HOOKS=1`).

## Source

- **Invocation 1** ran on `23e54486f9338db78e38565a50f1292a0121cf9c` (`fix(vm): the systemd VM
  run sends the devnet Zebra's cookie`) on `feat/zebra-basecamp-module`.
- **Invocation 2** ran on `f259e791b47f9cda1ee64344790bf977950babcc` (`devnet: the Taker Node
  keeps the Zebra cookie mount on the Logos transport`).
- `git diff --name-only 23e54486 f259e791` lists exactly `deploy/compose.logos.yaml` and
  `deploy/tests/test_image_contexts.py` (`diff-23e54486-f259e791.txt`). No image input
  differs, so invocation 2 ran on the images built from `23e54486`.
- The checkout was a detached worktree with its four submodules at their pinned commits, among
  them `third_party/logos-zebrad-module` at `ab43ad34`. It was clean for the image build and
  for both invocations: their `regress.log` files print "(tree dirty: 0)".
- The first attempt (`first-attempt-83b89df4/`) ran on images built from `83b89df4`.
  `git diff --name-only 83b89df4 23e54486` lists seven files (`diff-83b89df4-23e54486.txt`).

This note is committed on top of `f259e791`.

## Images

All images are local linux/arm64 images, built on this host. None was pulled from a registry,
and no release was built from this commit.

- **Build.** `scripts/from-scratch.sh --workspace <workspace> --only <phase>` ran the phases
  `nix`, `rust`, `build`, `stage` and `stack`, in that order, with `LEZ_MONERO=1
  LEZ_TEST_HOOKS=1 LEZ_TIMING_PROFILE=fast` and with `LEZ_WORKSPACE`, `LEZ_PROVISION`,
  `LEZ_MARKET_ROOT` and `LEZ_WALLET_IDENTITIES` naming that workspace. It ran from 2026-11-25
  22:00:19Z to 22:09:27Z, after `lanes.sh down 4 --wipe` and `down.sh --wipe` of the first
  attempt's stack.
- **The module.** `build:zebrad-module` produced `zebrad_module.lgx` with sha256
  `59047d1e8c0141d2e36a83144f59cd2d121355b031816df58293f66ebcea486c`, its pin.

The ids below were read with `docker image inspect` after the run:

| image | id | created (UTC) |
|---|---|---|
| `lez-maker-node:local` | `sha256:20c134f7ec50b4605fca7901c8c6ff98ce9601c4aa01d80d21e6c4c3c9e6fb9a` | 2026-11-25 22:06 |
| `lez-taker-node:local` | `sha256:35663343d39ccdd6f0fbbb29a415217fcd485f1401f8eb0c553f4d638d318662` | 2026-11-25 22:06 |
| `lez-zebra-module:local` | `sha256:77b38a41d36c869c8b75b157812c36dcb4268242661e09ee2dd258b0e0adc437` | 2026-11-25 19:56 |
| `lez-network-module:local` | `sha256:35a0626274258bc9aa374db8c2ed3b80218365b25bd9bd8478f330a4625c5efb` | 2026-11-25 19:56 |
| `lez-basecamp-ui:local` | `sha256:cc0e5c21a205f080e147fb34b3702d8685c622880917079b680589ebf97c3b83` | 2026-11-25 19:56 |
| `lez-services:local` | `sha256:03303dbfa14873822d5c4b698a3cd9f15619689ebc41bbb1ef9527f5178bd698` | 2026-11-25 19:56 |
| `lez-wallet-module:local` | `sha256:fc91ca78b3a9f518c33a621fcfe90f8156a2cd5dfdbf975c6252e706de4226a0` | 2026-11-11 21:52 |
| `lez-bitcoin-core:local` | `sha256:39e939eabe1cc7024b140b36464587f72e8b97d528162cf3849cb1479d653691` | 2026-11-18 16:42 |
| `lez-btc-miner:local` | `sha256:7dee784df4e5f1b1b8f44e12c092e239f95e4f8243f9fd42ae9622ff9917093a` | 2026-11-24 18:07 |
| `lez-monerod:local` | `sha256:798dfa37a106ab62eaa5d2a17b70cce40cce0330f2bc48441899880a4d2727f8` | 2026-11-24 18:16 |

The images other than the two Nodes are from the build of `83b89df4` the same evening (19:56Z).
Their inputs are the same at `23e54486`.

**Build source.** `/usr/local/share/lez/build-source.txt` and `sidecar-source.txt` read
`23e54486f9338db78e38565a50f1292a0121cf9c`. They were read in the lane-1 Maker and Taker before
the run, and `build-source.txt` again after both invocations in all eight Node containers
(lanes 1 to 4), which ran the two Node ids above. The rust phase recompiled
`zec-reference-actor`, `lez-maker-node` and `lez-taker-node` from the tree of `23e54486`.

**Hooks.** The Maker Node's executable carries `hold-zec-activations`.

**Escrow.** The escrow the market bootstrap deployed (`bootstrap/deployment.json` of the market
root) has ImageID `ca66e0dc4118597e0a9010670fd49aec923bba9c332e9e6a26d9208dbdf6af98`. It was
read on the stack's chain and again on the chain made before invocation 2.

## The run

- **Host.** Apple-silicon Mac, macOS 26.7.3, 10 cores and 64 GB of memory. Docker Desktop
  29.9.2 with 10 CPUs and 32.3 GB of memory for its VM. Docker had 29 GB free at the start,
  26 GB or more at every 15-minute check, and 28.3 GB at the end.
- **Stack.** The stack phase started a new LEZ chain (genesis 2026-11-25 22:05:38Z), new
  Bitcoin, Zcash and Monero regtest chains, and deployed the escrow. Profile `fast`. The
  Bitcoin actors' trace was on. Each lane's Zebra is the `zebra` service: `zebrad_module` in a
  headless Basecamp, Regtest, NU7 from height 1. getblockchaininfo with the cookie answered
  next-block branch `77190ad9` on every lane before the run.
- **Lanes.** Lane 1 is the stack. Lanes 2 to 4 are one more Maker/Taker pair each, with their
  own Bitcoin Core, miner, Zebra module, `monerod` and wallet-rpcs, on the stack's one LEZ
  network. `regress.sh` brought them up itself (`lanes.sh up 4`). Lanes 3 and 4 settle as
  `maker-lane03`/`taker-lane03` and `maker-lane04`/`taker-lane04`, minted by `lanes.sh mint 4`
  and funded at the genesis of each chain.
- **Invocation 1.** From the checkout's `deploy/`, with the workspace variables above:
  `LEZ_MONERO=1 LEZ_LANES=4 bash scripts/regress.sh full` (`20261125T221005Z`), 22:10:05Z to
  01:24:43Z, exit 1. `LEZ_REGRESS_DURATIONS` named the `results.tsv` of
  `regression-0.4.0-2026-11-24-macos-arm64`; it only orders the tests.
- **Between them.** `LEZ_LANES=4 bash scripts/up.sh --fresh-lez` ran 01:26:09Z to 01:27:50Z,
  exit 0.
- **Invocation 2.** The same environment, with the worktree at `f259e791`: `bash
  scripts/regress.sh run ui:btc:logos-happy:TakerSellsForeign
  ui:zec:logos-happy:TakerSellsForeign ui:zec:logos-happy:TakerSellsLez`
  (`20261126T012807Z`), 01:28:07Z to 01:30:03Z, exit 0.
- **The reproduction.** At 01:31Z the lane-1 Taker was recreated with `compose.yaml` and the
  overlay of `23e54486` (`git show 23e54486:deploy/compose.logos.yaml`, kept as an untracked
  file for the minute it was used, then deleted). Its health was read, and it was recreated on
  `compose.yaml` again. No swap was open on lane 1.
- **Not committed.** The runners' console output holds host paths and repeats `regress.log`.
  The Node logs the runner kept for the three failed tries are kept outside git
  (`OUTSIDE-GIT.sha256`). The Logos phases `ui-e2e.sh` kept, with both Chat module logs, are
  not committed either.

## Privacy

Nothing here is private. The logs carry:

- step lines, swap and transaction ids, account ids, balances and LEZ heights;
- the stack's start-up lines, and the public SHA-256 hash-lock digests of swaps;
- in `overlay-cookie-check.log`, the cookie file's owner, mode and size, never its content.

No key, wallet seed, RPC password, Zebra cookie, capability file, database, `runtime/` or
`market/` content is included. Local paths are replaced by `<checkout>`, `<workspace>`, `<home>`
and `<scratch>`. The folder was scanned before commit with gitleaks 8.30.1 (the repository's
`.gitleaks.toml`), and searched for key, seed, password, cookie, mnemonic and preimage patterns
and for local paths. The three gitleaks findings are all `generic-api-key` on the public
digest `24c1c1f2…22c2db` of `zec-maker-early-claim`, which presents a wrong preimage on purpose:

- `run-23e54486-full/logs/node-zec-zec-maker-early-claim-TakerSellsLez-1.log`, lines 139 and
  187;
- `run-23e54486-full/e2e/zec-maker-early-claim.json`, line 32.
