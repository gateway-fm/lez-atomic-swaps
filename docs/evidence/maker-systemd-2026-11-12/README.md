# Maker and Taker Nodes under systemd in an isolated VM, 2026-11-12

`scripts/run-maker-node-systemd-vm.sh` boots a stock Ubuntu 24.04 cloud image under QEMU/KVM inside
an `ubuntu:24.04` container given only `--device /dev/kvm` (no `--privileged`, no unit on the
host), installs the packages and follows [docs/maker-systemd-install.md](../../maker-systemd-install.md)
as root. Recorded here: Bitcoin swaps and a Zcash swap executed between the two units against the
devnet's chains (below), and the earlier run without chains.

## A Bitcoin swap between the two units (`LEZ_STACK_DEPLOY_DIR` set)

| File | What |
|---|---|
| `swap-keyfile-signer.log`, `-balances.txt` | the swap with each role's LEZ signer key as a `lez-swap`-owned 0400 file, copied by the launcher into its state directory |
| `swap-credential-signer.log`, `-balances.txt` | the same swap with every key only as an encrypted credential: the LEZ signer keys are read in place under `/run/credentials/<unit>/` by the Node and its sidecars, and no copy of them exists on disk |

- **Source**: the credential run at `180a3976`, this branch on `feat/m5-gaps` `b6801ad3` with the
  Zcash reserve round (the Maker starts with its Zcash authority, `maker ZEC authority configured`);
  its code is this commit's. The key-file run at `153d3ac6`, an earlier rebase of this branch on
  `6ad39beb`, before the credential commits. The transcripts' first lines list every binary's
  sha256 and the launcher's. Debug binaries without debug info built on the host
  (`CARGO_PROFILE_DEV_DEBUG=0`); the LEZ sidecar from the builder image: the stack's
  `provision/data/sidecar` (tree `5486ebd9`, sha256 `e024dfe6…00a3`) for the key-file run, built
  from this branch's sidecar tree in `lez-builder:local` (sha256 `b3ced25d…123a`) for the
  credential run.
- **Chains**: the devnet stack (key-file run at `9e383dd6`, credential run at `2a228123`; images
  and their ids in the transcripts), under the
  stack lock. The VM reaches exactly three endpoints, through QEMU `guestfwd` (one `socat` per
  connection) from a container on the stack's network `lez-swap-chains`: `bitcoin-core:18443`,
  `sequencer:3040`, `indexer:8779`; `socat` units put them on the VM's loopback. Identities, Core
  wallets (`lez-maker`, `lez-taker`), cookie, market constants and timing profile are lane 1's;
  lane 1's two Node containers were stopped for each run and were healthy again afterwards.
- **What ran**: both units start through the role launcher (`Type=notify` READY,
  `systemd-analyze security` 1.6 for each); the Maker configures, prices and publishes a Bitcoin
  offer (Taker sells 1 000 000 sat for 1 000 LEZ); the Taker discovers, takes and locks Bitcoin;
  the Maker's main process is killed with `kill -9` and systemd restarts it (`NRestarts=1`); the
  Maker locks LEZ, the Taker reaches `claim_available`, claims, and both report the swap
  `completed` (the Maker's history: `Completed`).
- **Balances** (from the chains, as `node-e2e.py`'s `party_balances`): in each run the Maker
  gains 999 000 sat (the lot less its 1 000-sat claim fee) and pays 1 000 LEZ; the Taker pays
  1 003 100 sat (the lot and a 3 100-sat lock fee) and gains 1 000 LEZ.
- **Not run**: Zcash; a Taker-sells-LEZ swap; route-health probes against a failing chain.

## A Zcash swap between the two units (`LEZ_SWAP_PAIR=zcash`)

| File | What |
|---|---|
| `swap-zcash.log`, `-balances.txt` | one Zcash swap, Taker sells 50 000 LEZ for 1 ZEC, formed at take time by `zec_reserve_v1` (no dealer) |

- **Source**: `0939c244` on `feat/m5-gaps` `e7cae1fa` (binary sha256s in the transcript's first
  lines); the LEZ sidecar is the stack's `provision/data/sidecar` of this branch's sidecar tree
  `98e9f1c3`. Stack at `713ea269` under the lock, lane 1's identities with its Node containers
  stopped and healthy again afterwards. The VM also reaches `zebra:18232` (a fourth `guestfwd`).
- **Keys**: the Delivery and LEZ signer keys as credentials, the Maker's Delivery credential being
  lane 1's Maker Zcash key (Zebra's miner address, so the Maker holds mature outputs to fund
  from); the Taker's Zcash key a `lez-swap`-owned file the launcher copies.
- **What ran**: both units READY through the launcher, the Maker with its Zcash authority; the
  Maker configures, prices and publishes the Zcash offer; the Taker lists and takes it; the Nodes
  form the agreement at take time, the Taker's supervisor locks LEZ and the Maker funds Zcash;
  once the funding is in a block the Maker is killed with `kill -9` and systemd restarts it
  (`NRestarts=1`); the Maker reveals, the Taker reaches `claim_available`, claims its Zcash, and
  both report `completed` (the Maker's history: `Completed`). The guest mined three Zebra blocks,
  one each for the Maker's funding, its second confirmation and the Taker's claim, to Zebra's
  configured miner address (lane 1's Maker), as `node-e2e.py`'s mining policy does.
- **Balances**: the Taker pays 50 000 LEZ and gains 99 990 000 zatoshis (1 ZEC less its claim
  fee); the Maker gains 50 000 LEZ, and its Zcash balance moves by the 1 ZEC it funded and the fees
  less the three blocks' coinbase, which Zebra pays to it.
- **Limit (stated, not changed)**: Zcash keys are not credentials. During and after the swap the
  state directories hold, `lez-swap` mode 0600 and not removed at completion: each role's
  long-lived Zcash key copied into the swap's `btc/swaps/<id>/zec/zcash.key` (for the Maker that
  key is also its Delivery key; the transcript compares the copies with the role keys), the
  Taker's also in `zec/zcash.key`, a per-swap `claim-recovery.key` per role, the Maker's per-swap
  `claim-preimage.key`, and the Maker's long-lived `zec-claim-recovery.key`.
- **Not run**: Taker sells ZEC (the Taker would need mature transparent outputs on its own key,
  for example blocks mined to its address or a transfer from the Maker's); an earlier attempt of
  this run, stopped by hand after the funding's first confirmation because the harness never
  mined the second, left that swap's Taker LEZ lock and Maker Zcash funding on lane 1 unrefunded.

## The earlier run without chains

Recorded before the units ran the role launcher; without chains the script now starts each Node
from its node.json through a drop-in, as this run's unit did directly.

| File | What |
|---|---|
| `transcript.log` | the run: every command, its output and exit status |
| `transcript-before-unit-fix.log` | an earlier run with `RestrictSUIDSGID=yes` still in the unit, up to the failed start |

### Inputs

- Source: commit `3395e6a0` of the working branch the run was made on (`agent/m5-systemd`; not in
  this branch's history). Its trees of `packaging` (`4c39cffecaab6332605f01faf5091d17d612d04c`),
  `crates/node-common` (`e4f9501d02b0de8a77fd852d46a1ef0b6ace79aa`) and the blob of
  `scripts/run-maker-node-systemd-vm.sh` (`c908087ce783609d801406ccae5780d5df46ca9f`) are the same
  at this head (`git rev-parse <commit>:<path>`); `crates/maker-node` (`bc9e9bde` then) differs by
  `e1516cf2` (swap history phase), `crates/taker-node` (`9c38fe0c` then) by the CLI's error line
  printing its causes (`cd031757`). Debug binaries built on the host (Ubuntu 24.04, glibc 2.39)
  with `cargo build --locked -p lez-maker-node --features pair-zec --bins`, the same for
  `lez-taker-node`, and `cargo build --locked -p zec-reference-actor --bin lez-zec-maker-actor`:
  - `lez-maker-node` `728c03afa18ae371b54e12a1e470c92cb85fe989261646633be675a2acb98bd7`
  - `lez-maker-cli` `5f9bd0e0d4fb5f184fd9e2d221a1f32010d0e340524c5dec2367ae7b27d41958`
  - `lez-zec-maker-actor` `c0a58260976958115bacf233ce95a170ec9a8a1d9623cf90fe5b002728a9c96d`
  - `lez-taker-node` `3cc49aa9a577aca4a91dcc853c091f01c30f34cbdcfa415369f8ca8b90728540`
  - `lez-taker-cli` `fb08c81e13048808240b57221e95dee3c79893ce5cf47b8343588301f8921fdd`
- VM image: `ubuntu-24.04-server-cloudimg-amd64.img` from
  `https://cloud-images.ubuntu.com/releases/noble/release/`, sha256
  `04e8f2461a16a44e78a370284471f29616cdd9baf1893df25807b71f985b6c94` (checked against that
  directory's `SHA256SUMS`; the GPG signature on it was not checked). Ubuntu 24.04.5 LTS, kernel
  6.8.0-147-generic, systemd 255 (255.4-1ubuntu8.18), no TPM. QEMU 8.2.2.
- Keys and credentials were generated inside the VM from `/dev/urandom` and died with it.

### What ran and what it shows

- **Install**: service user, `install-maker-node-service.sh`, three `systemd-creds` credentials;
  `node.json` names the Delivery key as `/run/credentials/lez-maker-node.service/delivery-signing.key`.
- **Readiness**: `Type=notify`, `ActiveState=active`, `StatusText=maker RPC and durable state are
  ready` (the Node's sd_notify READY) on the first start. No `*.key` file exists under `/etc/lez`.
- **Credential**: systemd exposes it as `root:root` mode 0440 in `/run/credentials/<unit>/`
  (itself `root:root` 0550); the Node accepts it there (rule in `crates/node-common/src/secure_file.rs`).
- **Hardening**: `systemd-analyze security` overall exposure **1.6 OK** for both units; the misses
  it lists are SystemCallFilter `@resources`/`@privileged`, RootDirectory, RestrictSUIDSGID,
  AF_UNIX/AF_INET, PrivateNetwork, PrivateUsers, DeviceAllow, IPAddressDeny.
- **Owner socket**: `/run/lez/maker/node.sock`, `lez-swap:lez-swap` mode 0600 in a 0700
  directory; `lez-maker-cli health` as `lez-swap` answers, as another user it gets `Permission
  denied (os error 13)`.
- **Prices and advertises** (run-local Delivery): configure-pair, set-local-price, quote and
  publish-offer for Zcash/TakerSellsLez; the offer file appears in the Delivery directory.
- **Restart**: `kill -9` of the main pid; systemd restarts it (`NRestarts=1`, new pid, READY
  again); health answers and pairs, prices and the active offer are unchanged.
- **CLI stop/start**: `lez-maker-cli stop` returns `inactive` and the socket is gone;
  `lez-maker-cli start` returns `active`, health answers, the offer is still there.
- **Taker Node**: installed with `install-taker-node-service.sh`; its role config trusts the Maker
  identity that `lez-maker-cli delivery-identity` derived from the same credential (so the CLI
  read the root-owned credential too). READY, socket `lez-swap` 0600 in a 0700 directory, health
  refused to another user, the Maker's offer listed and authenticated by `lez-taker-cli swap
  offers`, `lez-taker-cli stop` returns `inactive`. The Taker unit loads no credentials.
- **Refused key**: a world-readable key file makes the start fail with `Error: Delivery signing key
  must be an owner-owned, single-link mode-0400-or-0600 regular file within its size bound`.

### Findings

1. **`RestrictSUIDSGID=yes` stopped the Node from starting** (`transcript-before-unit-fix.log`):
   under it systemd fails `openat2()` with ENOSYS, and the Node opens its lease, database and keys
   with `openat2(RESOLVE_NO_SYMLINKS)`. The directive was dropped from both Node units; exposure
   1.5 → 1.6.
2. **The Node refused the key systemd hands it as a credential** until `241d7fc5`: it accepted
   only a key file its own user owns with mode 0400/0600.

### Not run

No chain node, LEZ sidecar, Chat socket, ZEC actor deployment or route-health probe: the Maker
was started with run-local Delivery only, so route health reads `disabled`, and the claim-recovery
credential was loaded by the unit but not read. **No swap was executed.**
