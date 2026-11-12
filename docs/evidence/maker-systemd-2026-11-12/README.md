# Maker and Taker Nodes under systemd in an isolated VM, 2026-11-12

`scripts/run-maker-node-systemd-vm.sh` booted a stock Ubuntu cloud image under QEMU/KVM inside an
`ubuntu:24.04` container given only `--device /dev/kvm` (no `--privileged`, no unit on the host;
the VM's NIC is QEMU user networking with `restrict=on`, so it has no outside network). In the VM,
as root, it followed [docs/maker-systemd-install.md](../../maker-systemd-install.md) and recorded
the result. The keys reached the Node only as encrypted systemd credentials.

| File | What |
|---|---|
| `transcript.log` | the run: every command, its output and exit status |
| `transcript-before-unit-fix.log` | an earlier run with `RestrictSUIDSGID=yes` still in the unit, up to the failed start |

## Inputs

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

## What ran and what it shows

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

## Findings

1. **`RestrictSUIDSGID=yes` stopped the Node from starting** (`transcript-before-unit-fix.log`):
   under it systemd fails `openat2()` with ENOSYS, and the Node opens its lease, database and keys
   with `openat2(RESOLVE_NO_SYMLINKS)`. The directive was dropped from both Node units; exposure
   1.5 → 1.6.
2. **The Node refused the key systemd hands it as a credential** until `241d7fc5`: it accepted
   only a key file its own user owns with mode 0400/0600.

## Not run

No chain node, LEZ sidecar, Chat socket, ZEC actor deployment or route-health probe: the Maker
was started with run-local Delivery only, so route health reads `disabled`, and the claim-recovery
credential was loaded by the unit but not read. **No swap was executed.**
