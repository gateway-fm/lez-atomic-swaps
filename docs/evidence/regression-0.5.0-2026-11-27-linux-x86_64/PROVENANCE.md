# Provenance: full regression at `6ad741bf` (0.5.0), 2026-11-27/28, Linux x86_64

One operator ran both parties, Maker and Taker, on one host. Everything in this folder was
produced on that host by the Docker devnet of `deploy/`: every test of
`scripts/regress.sh full` (200: `regress.sh list full`), the Node scenarios by
`scripts/node-e2e.py` and the desk journeys by `scripts/ui-e2e.sh`. They ran three at a time on
three lanes, with the Monero regtest chain up (`LEZ_MONERO=1`) and the devnet test hooks built
in (`LEZ_TEST_HOOKS=1`).

## Source

- **Invocation 1** ran on `6ad741bfd7d06fa980dcc2051970f9c2fd204822` (`test(deploy): three
  harness unit tests follow the take-time Zcash swap`).
- **Invocation 2** ran on `f98908ec782d1b939b97420fa07575356b604063` (`test(systemd-vm): the VM
  run finds a stack under its LEZ_STACK_PREFIX`).
- `git diff --name-only 6ad741bf f98908ec` lists fourteen files
  (`diff-6ad741bf-f98908ec.txt`): `deploy/scripts/node-e2e.py`,
  `scripts/run-maker-node-systemd-vm.sh` and twelve documents. No image input differs, so
  invocation 2 ran on the images built from `6ad741bf`.
- The checkout was a clone with its submodules at their pinned commits. It was clean for the
  image build and for both invocations: their `regress.log` files print "(tree dirty: 0)".

This set is committed after `f98908ec`. The one commit between them changes `.gitleaks.toml`
only.

## Images

All images are local linux/amd64 images, built on this host. None was pulled from a registry,
and no release was built from this commit.

- **Build.** `scripts/from-scratch.sh --workspace <workspace> --only <phase>` ran the phases
  `build:escrow`, `rust`, `build:sidecar`, `build:role-node-modules`, `build:swap-node-module`,
  `nix`, `build:identities`, `stage` and `stack`, in that order, with `LEZ_MONERO=1
  LEZ_TEST_HOOKS=1 LEZ_TIMING_PROFILE=fast`, with `LEZ_WORKSPACE`, `LEZ_PROVISION`,
  `LEZ_MARKET_ROOT` and `LEZ_WALLET_IDENTITIES` naming that workspace, and with the stack's own
  names: `LEZ_IMAGE_PREFIX=lezm5 LEZ_STACK_PREFIX=lezm5 LEZ_STACK_PORT_OFFSET=2000`. It ran from
  2026-11-27 18:59:55Z to 19:13:35Z, every phase with exit 0.
- **Names.** The images carry the prefix `lezm5` instead of `lez`, and the containers and
  networks too, because the host also holds another stack. The port offset moves every
  published port by 2000.

The ids below were read with `docker image inspect` before invocation 1. The same command after
it printed the same table.

| image | id | created (UTC) |
|---|---|---|
| `lezm5-maker-node:local` | `sha256:da246712554df0b5519444d35dd217411b421e656ef546a280533d8039c0c4c2` | 2026-11-27 19:11 |
| `lezm5-taker-node:local` | `sha256:9bfd4061bda851665887f5f966ac50b30f1b542e86ae5b2fda605485bc7a368c` | 2026-11-27 19:11 |
| `lezm5-zebra-module:local` | `sha256:70b73137545855508afee7e589866f750361d6f168558197269265eb52462e83` | 2026-11-27 19:10 |
| `lezm5-network-module:local` | `sha256:1a255865ff56bfc5844d0390657cda1e4e355b0a543afc021d8e8c9ae7d47697` | 2026-11-27 19:10 |
| `lezm5-basecamp-ui:local` | `sha256:ba97958056d697512eddd2fa797932330e858fdb67e4f6f7468f93dd9029b21a` | 2026-11-27 19:10 |
| `lezm5-services:local` | `sha256:9d1108ab53d8ff0ea81ebca0bfdac7a0254759e6dfec3fd0b2b150e3ae1d633b` | 2026-11-27 16:03 |
| `lezm5-wallet-module:local` | `sha256:275469bb4749594e275145cb7a0c6d5871934204753124b40aeed976df3702c1` | 2026-11-27 16:07 |
| `lezm5-bitcoin-core:local` | `sha256:5830869747e37a9483f54713be993b396fe9a279ff65feb41ce822297edab16d` | 2026-11-27 16:03 |
| `lezm5-btc-miner:local` | `sha256:51a41cea2ff6652f4f148fa9fe2790aec17e2037aea872a9a6f3a3ecc5155d0a` | 2026-11-27 16:03 |
| `lezm5-monerod:local` | `sha256:c70a543f18a50e3753a55908c763ca48ffb1ccbcce6e91ee769ec7ece557d2d4` | 2026-11-27 16:06 |
| `lezm5-explorer:local` | `sha256:3ff6a89c27f65d0c957f86e0347c626b0d91764d19ecf7fe0ddd3839c4e23d7d` | 2026-11-27 16:10 |
| `lezm5-btc-explorer:local` | `sha256:86ef2562043066e02f0b36081eb72ef664bf6e810bade7bbf2c45503ff6af5a5` | 2026-11-27 16:11 |

The seven images created at 16:03Z to 16:11Z are from the build of `6a5535db` the same
afternoon. Under `deploy/images/`, `git diff 6a5535db 6ad741bf` changes only the Taker Node's
and the Zebra module's image inputs, and both of those images were built again at 19:10Z.

**Build source.** `/usr/local/share/lez/build-source.txt` read
`6ad741bfd7d06fa980dcc2051970f9c2fd204822` in all six Node containers (lanes 1 to 3) before
invocation 1, in the four of lanes 1 and 2 after it (lane 3 was stopped then: the run's last two
tests use two lanes on a chain of their own), and in all six after invocation 2. The lane-1
Maker's `sidecar-source.txt` reads the same commit. The six containers ran the two Node ids
above.

**Hooks.** The scenarios that pass only with the test hooks built in passed, among them
`claim-send-fails` (`fail-claim-submissions`) and `zec-maker-claim-collision`
(`claim-nonce-thefts`).

**Escrow.** The escrow the market bootstrap deployed (`bootstrap/deployment.json` of the market
root) has ImageID `ca66e0dc4118597e0a9010670fd49aec923bba9c332e9e6a26d9208dbdf6af98`, the
ImageID of the 0.4.0 sets.

## The run

- **Host.** A virtual machine with 8 cores (x86_64) and 31 GB of memory, Ubuntu 24.04.5 LTS,
  kernel 6.8.0-147-generic, Docker Engine 29.1.3 without emulation. The disk had 126 GB free
  at the start and 125 GB at the end.
- **Stack.** Before invocation 1, `lanes.sh mint 3` and `LEZ_LANES=3 bash scripts/up.sh
  --fresh-lez` made a new LEZ chain (genesis 2026-11-27 19:14:16Z). The Bitcoin, Zcash and
  Monero regtest chains are those the stack phase made at 19:10Z. Profile `fast`. Each lane's
  Zebra is the `zebra` service: `zebrad_module` in a headless Basecamp, Regtest.
- **Lanes.** Lane 1 is the stack. Lanes 2 and 3 are one more Maker/Taker pair each, with their
  own Bitcoin Core, miner, Zebra module, `monerod` and wallet-rpcs, on the stack's one LEZ
  network. `regress.sh` brought them up itself (`lanes.sh up 3`). Three lanes is what the
  host's 8 cores and 31 GB carry.
- **Invocation 1.** From the checkout's `deploy/`, with the workspace variables and names above:
  `LEZ_MONERO=1 LEZ_TEST_HOOKS=1 LEZ_LANES=3 bash scripts/regress.sh full`, 19:16:59Z to
  00:12:17Z, exit 1.
- **Between them.** The checkout moved from `6ad741bf` to `f98908ec` (a fast-forward). `LEZ_LANES=3
  bash scripts/up.sh --fresh-lez` ran 00:12:58Z to 00:15:06Z, exit 0.
- **Invocation 2.** The same environment: `bash scripts/regress.sh run
  node:zec:zec-maker-claim-collision:TakerSellsLez`, 00:15:06Z to 00:17:34Z, exit 0.
- **Earlier runs, not counted.** During development the same afternoon, the new scenarios ran
  on images of `6a5535db`. Those runs are not in this set and are not counted: every count here
  is the first try of a test on the images of `6ad741bf`.
- **Not committed.** The runners' console output holds host paths and repeats `regress.log`.
  The Node logs the runner kept for the failed try are kept outside git
  (`OUTSIDE-GIT.sha256`); the test log in `run-6ad741bf-full/logs/` carries the Node lines the
  failure's reading rests on.

## Privacy

Nothing here is private. The logs carry:

- step lines, swap and transaction ids, account ids, balances and LEZ heights;
- the stack's start-up lines, and the public SHA-256 hash-lock digests of swaps.

No key, wallet seed, RPC password, Zebra cookie, capability file, database, `runtime/` or
`market/` content is included. Local paths are replaced by `<checkout>`, `<workspace>`, `<home>`
and `<out>`. Paths under `/tmp/` in the logs are paths inside containers. The folder was scanned
before commit with gitleaks 8.30.1 (the repository's `.gitleaks.toml`), and searched for key,
seed, password, cookie, mnemonic and preimage patterns and for local paths.
