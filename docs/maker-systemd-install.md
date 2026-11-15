# Installing the Maker and Taker Nodes as systemd services

Every command below except the Monero section ran as root in an Ubuntu 24.04 VM (systemd 255) under
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
For the Maker's optional Monero role (below), also `lez-maker-node/pair-xmr` and
`-p xmr-reference-actor --bins`, and the Stage-A planner `lez-v02-xmr-stage-a-plan` from the same
builder directory as the sidecar.

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

Every role key reaches the Nodes as an encrypted credential, decrypted at each start into
`/run/credentials/<unit>/` (root:root 0440, readable by `lez-swap` through an ACL) and read
there by the Node, its sidecars and its Zcash actors; the one exception is the Taker's Zcash key
(below). The Maker unit loads four, the Taker unit two:

```sh
head -c 32 /dev/urandom | systemd-creds encrypt --name=delivery-signing.key - /etc/lez/maker/credentials/delivery-signing.key.cred
systemd-creds encrypt --name=lez-signer.key lez-signer.key /etc/lez/maker/credentials/lez-signer.key.cred
```

(the same `head -c 32 …` for `maker-claim-recovery.key` and `maker-claim-preimage.key`, and the
Taker's own `lez-signer.key` and `zcash.key` into `/etc/lez/taker/credentials/`). `lez-signer.key` is the role's
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
is its Delivery key (the funder needs mature transparent outputs on it) and its long-lived
claim-recovery key is the `maker-claim-recovery.key` credential.

**What stays in plaintext for Zcash** (`lez-swap` mode 0600 under `/var/lib/lez/<role>/`): the
Taker's Zcash key as one role-level copy, `zec/zcash.key` (32 raw bytes, written from its
credential only when the key changes: every swap binds that file); and in each swap's
`btc/swaps/<id>/zec/` the swap's own `claim-recovery.key` and the funder's `claim-preimage.key`,
kept after completion (the Maker reads its own again for a replayed completion). No swap
directory holds a role's long-lived key.

## Monero (Maker only, optional)

A packaging proof, not a production recipe: the Monero lane is regtest-only today (the role
configuration accepts only the `accelerated-regtest` profile), and these steps were checked
statically and with stub programs only, not in the VM. The Maker unit as installed has no
Monero role. To give it one, with `monerod` on `127.0.0.1:18081` and the Maker's own
`monero-wallet-rpc` on `127.0.0.1:18083` (loopback only: the Monero adapter refuses any other
endpoint; a `socat` forward as above for a daemon elsewhere), its wallet named `maker` and
opened without a password. The Maker's install script installs the Monero role (the actor, its six
tools, the planner and the drop-in example `/etc/lez/maker/monero.conf.example`) only when the
build has all of them, and refuses a partial set; a Maker installed without them runs Bitcoin and
Zcash as before, and its launcher refuses Monero logins, naming the missing programs.

```sh
systemd-creds encrypt --name=monerod.login monerod.login /etc/lez/maker/credentials/monerod.login.cred
systemd-creds encrypt --name=monero-wallet-rpc.login monero-wallet-rpc.login /etc/lez/maker/credentials/monero-wallet-rpc.login.cred
install -D -m 0644 /etc/lez/maker/monero.conf.example /etc/systemd/system/lez-maker-node.service.d/monero.conf
```

Each login is the RPC's `user:password` (its `--rpc-login`). The drop-in loads both credentials
and names their directory to the launcher (`LEZ_MONERO_LOGIN_DIR`), and sets the three windows
`LEZ_XMR_MAKER_FUNDING_CUTOFF_SECONDS`, `LEZ_XMR_REFUND_SECONDS` and `LEZ_XMR_PUNISH_SECONDS`
to the devnet's fast profile (edit them there or set them in `env`; the Taker must use the
same). The launcher then writes
`xmr-role.json`, the actor root `xmr-actors/` (0700) and owner-private copies of both logins in
`monero-rpc/` under `/var/lib/lez/maker/`, and the Node keeps its wallet lock in
`btc/monero-rpc/`; every swap's effect authority holds the logins again, split into user and
password files. The Node reports the Monero route (Taker sells LEZ only) by asking `monerod`.
The Taker unit has no Monero packaging.

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
