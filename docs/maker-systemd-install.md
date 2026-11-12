# Installing the Maker and Taker Nodes as systemd services

Every command below ran as root in an Ubuntu 24.04 VM (systemd 255) under
`scripts/run-maker-node-systemd-vm.sh`, which also executed a Bitcoin and a Zcash swap between the
two units against the devnet's chains; record: `docs/evidence/maker-systemd-2026-11-12/`.

## Prerequisites

Linux with systemd 255 or later, as root; `jq` and `socat` (`apt install jq socat`); Bitcoin
Core, the LEZ sequencer, the LEZ indexer and Zebra answering on `127.0.0.1:18443`, `:3040`, `:8779`
and `:18232`
(local nodes, or a loopback forward such as `socat TCP-LISTEN:18443,bind=127.0.0.1,fork
TCP:<core>:18443`); a funded LEZ account and Core wallet per role. Binaries built from this
checkout (`cargo build --locked -p lez-maker-node -p lez-taker-node --features
lez-maker-node/pair-zec,lez-taker-node/pair-zec --bins`, `-p btc-reference-actor --bins`,
`-p zec-reference-actor --bin lez-zec-maker-actor --bin lez-zec-taker-actor`,
`-p lez-logos-price-c-api --bins`,
`-p lez-runtime-healthcheck`) and the LEZ sidecar of the same tree, built in the builder image
(`deploy/scripts/from-scratch.sh --only build` → `provision/data/sidecar/lez-v02-bridge-poc`).

## Install

```sh
useradd --system --no-create-home --shell /usr/sbin/nologin lez-swap
SOURCE_BIN_DIR=target/debug SIDECAR_PROGRAM=<sidecar> ./scripts/install-maker-node-service.sh
SOURCE_BIN_DIR=target/debug SIDECAR_PROGRAM=<sidecar> ./scripts/install-taker-node-service.sh
```

Binaries, the sidecar and the role launcher (`lez-role-node-launcher`) go to `/usr/bin`, the
units to `/usr/lib/systemd/system/`, `env.example` and an empty mode-0700 `credentials/` to
`/etc/lez/<role>/`. Each unit runs the launcher, which renders the Node's configuration from
`/etc/lez/<role>/env` and execs the Node.

## Keys and inputs

The Delivery and LEZ signer keys reach the Nodes only as encrypted credentials, decrypted at each start
into `/run/credentials/<unit>/` (root:root 0440, readable by `lez-swap` through an ACL) and read
there; only a Zcash swap copies one of them (the Maker's Delivery key, below). The Maker unit
loads four, the Taker unit one:

```sh
head -c 32 /dev/urandom | systemd-creds encrypt --name=delivery-signing.key - /etc/lez/maker/credentials/delivery-signing.key.cred
systemd-creds encrypt --name=lez-signer.key lez-signer.key /etc/lez/maker/credentials/lez-signer.key.cred
```

(the same `head -c 32 …` for `maker-claim-recovery.key` and `maker-claim-preimage.key`, and the
Taker's own `lez-signer.key` into `/etc/lez/taker/credentials/`). `lez-signer.key` is the role's
LEZ account key (64 lowercase hex). Without a TPM, `systemd-creds` binds them to
`/var/lib/systemd/credential.secret` only. Per role, as run:

```sh
install -m 0644 market-bootstrap.env /etc/lez/market-bootstrap.env
install -d -m 0755 /etc/lez/maker/identity
install -o lez-swap -g lez-swap -m 0400 identity.json /etc/lez/maker/identity/
install -o lez-swap -g lez-swap -m 0400 <core datadir>/.cookie /etc/lez/maker/btc-rpc-cookie
cp /etc/lez/maker/env.example /etc/lez/maker/env   # then set the wallet and LEZ_V02_CHANNEL_ID
```

`market-bootstrap.env` holds the LEZ escrow program id and genesis hash; the Core cookie copy
must be renewed when Core restarts. In `env`, `LEZ_ZEC_STATE_DIR` (`/var/lib/lez/<role>/zec`) keeps
the launcher's Zcash state inside the unit's state directory. For Zcash, the Maker's Zcash key
is its Delivery key (the funder needs mature transparent outputs on it); the Taker's is a file,
`install -o lez-swap -g lez-swap -m 0400 zcash.key /etc/lez/taker/identity/taker/`.

**Zcash keys are not credentials.** During and after a Zcash swap the state directory holds,
`lez-swap` mode 0600 and kept after completion: each role's long-lived Zcash key (for the Maker
also its Delivery key) copied into every swap's `btc/swaps/<id>/zec/zcash.key`, the Taker's in
`zec/zcash.key` too, a per-swap `claim-recovery.key` per role, the Maker's `claim-preimage.key`,
and the Maker's long-lived `zec-claim-recovery.key`.

## Start and check

```sh
systemctl daemon-reload
systemctl start lez-maker-node    # before the Taker: it publishes the Maker identity the Taker trusts
systemctl start lez-taker-node
systemctl show -p ActiveState,SubState,MainPID,NRestarts,StatusText lez-maker-node
runuser -u lez-swap -- lez-maker-cli health
runuser -u lez-swap -- lez-taker-cli health
journalctl -u lez-maker-node
```

`StatusText=maker RPC and durable state are ready` is the Node's sd_notify READY. State lives in
`/var/lib/lez/<role>/`; the owner sockets are `/run/lez/<role>/node.sock`, `lez-swap` mode
0600 in a 0700 directory. Route health: `packaging/systemd/README.md`.

## Stop, start, upgrade

`lez-maker-cli stop` / `start` (as root) drive `systemctl` on that unit only. Upgrade: stop,
re-run the install scripts with the new binaries, `systemctl daemon-reload`, start; state is kept.
