# Installing the Maker Node as a systemd service

Every command below ran as root in an Ubuntu 24.04 VM (systemd 255) under
`scripts/run-maker-node-systemd-vm.sh`; record: `docs/evidence/maker-systemd-2026-11-12/`.

## Prerequisites

Linux with systemd 255 or later, as root, and the binaries built from this checkout:
`cargo build --locked -p lez-maker-node --features pair-zec --bins` and
`cargo build --locked -p zec-reference-actor --bin lez-zec-maker-actor`.

## Install

```sh
useradd --system --no-create-home --shell /usr/sbin/nologin lez-swap
SOURCE_BIN_DIR=target/debug ./scripts/install-maker-node-service.sh
```

Binaries go to `/usr/bin`, the unit to `/usr/lib/systemd/system/`, examples and an empty
mode-0700 `credentials/` to `/etc/lez/maker/`.

## Credentials

The unit loads three encrypted credentials; create all three:

```sh
head -c 32 /dev/urandom | systemd-creds encrypt --name=delivery-signing.key - /etc/lez/maker/credentials/delivery-signing.key.cred
head -c 32 /dev/urandom | systemd-creds encrypt --name=maker-claim-recovery.key - /etc/lez/maker/credentials/maker-claim-recovery.key.cred
head -c 32 /dev/urandom | systemd-creds encrypt --name=maker-claim-preimage.key - /etc/lez/maker/credentials/maker-claim-preimage.key.cred
```

Without a TPM, `systemd-creds` binds them to `/var/lib/systemd/credential.secret` only. No key
file stays on disk in plaintext: at each start systemd decrypts them into
`/run/credentials/lez-maker-node.service/` (root:root, mode 0440, readable by the service user
through an ACL), and the Node reads them there. The two claim keys are read only with the ZEC
pair's authority configured, which the run did not.

## Configure and start

`/etc/lez/maker/node.json` as run (run-local Delivery; no Chat, ZEC actors or route health):

```json
{"schema_version": 1, "arguments": [
  "--socket", "/run/lez/maker/node.sock", "--database", "/var/lib/lez/maker/maker.sqlite3",
  "--ready-file", "/run/lez/maker/ready", "--delivery-directory", "/var/lib/lez/maker/delivery",
  "--delivery-signing-key-file", "/run/credentials/lez-maker-node.service/delivery-signing.key"]}
```
```sh
chown lez-swap:lez-swap /etc/lez/maker/node.json
chmod 0600 /etc/lez/maker/node.json
systemctl daemon-reload
systemctl enable --now lez-maker-node
```

## Check it

```sh
systemctl show -p ActiveState,SubState,MainPID,NRestarts,StatusText lez-maker-node
runuser -u lez-swap -- lez-maker-cli health
systemd-analyze security lez-maker-node
journalctl -u lez-maker-node
```

`StatusText=maker RPC and durable state are ready` is the Node's sd_notify READY. State lives in
`/var/lib/lez/maker/` (SQLite database, its lease, Delivery offers); the owner socket is
`/run/lez/maker/node.sock`, owned by `lez-swap` with mode 0600 in a 0700 directory, so only the
service user and root can drive the Node. Route health: `packaging/systemd/README.md`.

In `journalctl -u lez-maker-node` a healthy start is `Starting …` then `Started …`, nothing from
the Node; a key it refuses (wrong owner or mode, or outside the credential directory) is `Error:
Delivery signing key must be an owner-owned, single-link mode-0400-or-0600 regular file …`.

## Stop, start, upgrade

`lez-maker-cli stop` / `start` (as root) drive `systemctl` on this unit only and wait for
`inactive` / `active`. Upgrade: `lez-maker-cli stop`, re-run the install script with the new
binaries, `systemctl daemon-reload`, `lez-maker-cli start`; state is kept. (The run exercised
each command, not this sequence as an upgrade.) The Taker Node installs the same way with
`scripts/install-taker-node-service.sh` and `/etc/lez/taker/{node,role}.json`.
