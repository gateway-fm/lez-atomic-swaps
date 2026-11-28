# Maker and Taker Nodes under systemd in an isolated VM, version 0.5.0, 2026-11-28

`scripts/run-maker-node-systemd-vm.sh` boots a stock Ubuntu 24.04 cloud image under QEMU/KVM
inside an `ubuntu:24.04` container given only `--device /dev/kvm` (no `--privileged`, no unit on
the host), installs the packages and follows
[docs/maker-systemd-install.md](../../maker-systemd-install.md) as root. This set is the four
runs of the script on the 0.5.0 tree. The set before it,
[maker-systemd-2026-11-12](../maker-systemd-2026-11-12/README.md), is from commits before
ADR 0216 and before 0.3.5.

All four runs exited 0, each on its first try with the script as committed at `013add5a`.

| File | Run | Result |
|---|---|---|
| `transcript.log` | no chains: the Maker unit from `node.json`, then the Taker unit beside it | unit `active`, READY by `sd_notify`, exposure 1.6; `kill -9` of the main process, `NRestarts=1`; a world-readable Delivery key refuses the start |
| `swap-bitcoin.log`, `swap-bitcoin-balances.txt` | `LEZ_STACK_DEPLOY_DIR` set: one Bitcoin swap, Taker sells BTC (1 000 000 sat for 1 000 LEZ) | `kill -9` of the Maker between the Taker's lock and its claim, `NRestarts=1`; both sides completed, the Maker's history reads `Completed` |
| `swap-zcash.log`, `swap-zcash-balances.txt` | `LEZ_SWAP_PAIR=zcash`: one Zcash swap, Taker sells LEZ (1 ZEC for 50 000 LEZ), agreed at take time (`zec_reserve_v1`) | `kill -9` of the Maker between the locks and the claim, `NRestarts=1`; both sides completed, the Maker's history reads `Completed` |
| `route-monero.log`, `route-monero-balances.txt` | `LEZ_SWAP_PAIR=monero`: the Maker unit gets its Monero role from the `monero.conf` drop-in | the Monero route healthy and a Monero offer published, before and after a `kill -9` (`NRestarts=1`); no Monero swap is executed |

## Source

- **Binaries.** Debug binaries without debug info (`CARGO_PROFILE_DEV_DEBUG=0`), built on the
  host from `f98908ec782d1b939b97420fa07575356b604063` by the script's own `cargo build`
  commands. The run without chains built them and ran at that commit. The three runs against
  the stack took the same files through `SOURCE_BIN_DIR` and ran the script at
  `013add5aae2a7c4bf78de157c52904407c572bf7`, which their transcripts name as the source
  commit. `git diff --name-only f98908ec 013add5a` lists the script itself, `.gitleaks.toml`,
  documents and the regression evidence set, and no source of a binary.
- **Why two commits.** At `f98908ec` the three runs against the stack stopped before the VM
  started: the script read the parties' balances on the default host ports, and this stack
  publishes its ports 2000 higher (`LEZ_STACK_PORT_OFFSET=2000`). `013add5a` makes the script
  follow the offset. Those three stopped starts produced no transcript.
- **The sidecar.** `lez-v02-bridge-poc`, and for the Monero run `lez-v02-xmr-stage-a-plan`, are
  the builder image's, from the stack's `provision/data/sidecar`, built from `6ad741bf`. Each
  transcript's first lines list every binary's sha256 and the launcher's.
- **The image.** `ubuntu-24.04-server-cloudimg-amd64.img`, checked against Ubuntu's
  `SHA256SUMS` inside the container in each run ("OK").

## The stack

The devnet stack of
[regression-0.5.0-2026-11-27-linux-x86_64](../regression-0.5.0-2026-11-27-linux-x86_64/PROVENANCE.md),
on its images of `6ad741bf` (the transcripts list the image ids), after that regression had
ended, with lane 1 idle. The stack runs under the prefix `lezm5`. The VM reaches the chains
through QEMU `guestfwd` from a container on the stack's network: `bitcoin-core:18443`,
`sequencer:3040`, `indexer:8779` and `zebra:18232`, and for the Monero run `monerod:18081` and
`maker-xmr-wallet:18083`. Identities, Core wallets, cookies, market constants and timing profile
are lane 1's. Lane 1's two Node containers were stopped for each run and started again
afterwards.

## Balances

Read from the chains before and after each run, as `node-e2e.py`'s `party_balances` does.

- **Bitcoin swap.** The Maker gains 999 000 sat (the lot less its 1 000-sat claim fee) and pays
  1 000 LEZ. The Taker pays 1 003 100 sat (the lot and a 3 100-sat lock fee) and gains 1 000
  LEZ.
- **Zcash swap.** The Maker gains 50 000 LEZ and the Taker pays 50 000 LEZ. The Taker gains
  99 990 000 zatoshis (1 ZEC less 10 000 zatoshis of fee). The Maker's Zcash address is
  also the address the devnet's Zebra mines to, so its balance rises by block rewards during
  the run and does not show the lot leaving.
- **Monero route.** No balance moves.

## What this does not show

- One host and one operator ran both parties. The chains are the private devnet's.
- No Monero swap ran under the units, and no swap in which the Taker sells ZEC.
- The Nodes ran as debug builds from the host, not from a release package.

## Privacy

The transcripts carry commands, unit states, public keys, account ids, transaction ids and
balances. Keys reach the units as encrypted systemd credentials and are not printed. The VM's
console logs are not committed. The folder was scanned with gitleaks 8.30.1 (the repository's
`.gitleaks.toml`) and searched for local paths.
