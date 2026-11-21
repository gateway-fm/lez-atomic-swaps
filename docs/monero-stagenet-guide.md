# Monero Stagenet: monerod, wallet-rpc and an XMR/LEZ swap's Monero leg

This guide sets up the pinned Monero release on public Monero Stagenet, by a
self-hosted `monerod` or through a public node, gets Stagenet coins, and runs
the Monero leg of an XMR/LEZ swap on Stagenet with the project's SDK and
adapter. It is RFP-003 U9 and the M4 deliverable "`monerod` +
`monero-wallet-rpc` stagenet setup documentation"
([rfp#124](https://github.com/logos-co/rfp/issues/124)).

What has run on Stagenet, and what has not:

- **Run on Stagenet.** Every command below but the removal at the end, on an Apple-silicon Mac with
  Docker Desktop, on 2026-11-21: the release verification, the image, a
  pruned self-hosted `monerod` syncing from public peers, `monero-wallet-rpc`
  on both routes, a wallet funded from a public faucet, and
  [`examples/full-lifecycle.rs`](../crates/xmr-reference-actor/examples/full-lifecycle.rs)
  with `LEZ_XMR_EXAMPLE_NETWORK=stagenet` through public nodes, whose
  completed swap funded the 2-of-2 shared address on Stagenet, verified the
  output at ten Stagenet confirmations through a view-only wallet, and swept
  it with the spend key the Taker rebuilt from the Maker's LEZ claim
  signature after losing its session state. Its refunded swap stopped at
  the funding, refused by a public node. [The run](#the-run-of-2026-11-21)
  lists the transactions. CI starts the same `monerod` on Stagenet in every
  run ([below](#in-ci)).
- **Not run on Stagenet.** The refund path (run on regtest only), and a swap
  between two Nodes. The Maker and Taker Nodes
  take XMR swaps only under the `accelerated-regtest` profile and refuse a
  Stagenet daemon ([why](#the-maker-and-taker-nodes)); in the example the LEZ
  escrow is modelled in memory, not the devnet's. Node-driven XMR swaps have
  run on Monero regtest only
  ([M4 checklist](m4-acceptance-checklist.md)). The self-hosted `monerod` did
  not reach the Stagenet tip during the session, so the Stagenet run used the
  public-node route; on the self-hosted route the adapter's and SDK's
  Stagenet checks ran against the syncing node and its wallet-rpc (at height
  132,209), and no wallet there was funded.

## The pinned release

The project pins Monero 0.18.5.1 (`tests/e2e/monero/provenance.env`): tag
`v0.18.5.1`, source commit `4f92268d7c16741cfb41e5bbe2aa46cc260a9ea5`, signer
binaryFate, key fingerprint `81AC591FE9C4B65C5806AFC3F0AF4D462A0BDF92`. The
repository keeps the signed `hashes.txt` of that release and the signing key
under `tests/e2e/monero/release/`; the live `hashes.txt` on getmonero.org
names later releases and no longer lists 0.18.5.1. CI and x86_64 hosts take
`monero-linux-x64-v0.18.5.1.tar.bz2`
(`22a7dda7b0cb699fdd6b7674c3b4a4465b337cc98a54983523b759e1e7cc9958`); arm64
hosts take `monero-linux-armv8-v0.18.5.1.tar.bz2`
(`c0caf042cb7c7b760f5ad6be188084b59352440b32990a78b8051497b9398dbc`), which
the same signed file lists. Both run on Stagenet; 0.18.5.1 follows Stagenet's
current hard fork, version 16.

`$REPO` is the repository's checkout and `$WORK` an empty, private
directory for the downloads, configuration and logins:

```sh
cd "$WORK"
curl -fsSLO https://downloads.getmonero.org/cli/monero-linux-armv8-v0.18.5.1.tar.bz2
mkdir -m 700 gnupg
LC_ALL=C gpg --homedir gnupg --import "$REPO/tests/e2e/monero/release/binaryfate-v0.18.5.1.asc"
LC_ALL=C gpg --homedir gnupg --verify "$REPO/tests/e2e/monero/release/hashes-v0.18.5.1.txt"
grep armv8 "$REPO/tests/e2e/monero/release/hashes-v0.18.5.1.txt"
shasum -a 256 monero-linux-armv8-v0.18.5.1.tar.bz2
```

`gpg` prints `Good signature from "binaryFate <binaryfate@getmonero.org>"`
with the primary key fingerprint above (and a warning that the key is not
certified, which only says that you have not signed it yourself). The archive's
SHA-256 must equal the line `grep` prints. On x86_64 replace `armv8` by `x64`.
`scripts/verify-monero-release.sh` does all of this for the archive of the
host's architecture (`MONERO_ARCH`, by default `uname -m`), and also checks
the source tag on GitHub, the archive's members and the binaries'
`--version`.

## The image

The repository's Dockerfile runs the two binaries as a non-root user on a
distroless base, and refuses binaries of another architecture than the
image's:

```sh
mkdir -p ctx/bin
tar -xjf monero-linux-armv8-v0.18.5.1.tar.bz2
cp monero-aarch64-linux-gnu-v0.18.5.1/monerod monero-aarch64-linux-gnu-v0.18.5.1/monero-wallet-rpc ctx/bin/
docker build -q -f "$REPO/tests/e2e/monero/Dockerfile" -t lez-m4-stagenet-monero:0.18.5.1 ctx
docker run --rm lez-m4-stagenet-monero:0.18.5.1
```

The last command prints `Monero 'Fluorine Fermi' (v0.18.5.1-release)`.

## Route 1: a self-hosted monerod

A pruned Stagenet node, its RPC on the host's loopback only, behind a Digest
login. The login sits in a config file, not on the command line, so that
`docker inspect` does not show it:

```sh
openssl rand -hex 24 > daemon.pass
openssl rand -hex 24 > wallet-rpc.pass
cat > monerod.conf <<EOF
stagenet=1
data-dir=/var/lib/monero
prune-blockchain=1
rpc-bind-ip=0.0.0.0
rpc-bind-port=38081
confirm-external-bind=1
rpc-login=daemon:$(cat daemon.pass)
add-peer=node.monerodevs.org:38080
add-peer=node2.monerodevs.org:38080
add-peer=node.sethforprivacy.com:38080
add-peer=stagenet.xmr-tw.org:38080
out-peers=16
no-igd=1
check-updates=disabled
log-level=0
EOF
chmod 644 monerod.conf
docker network create lez-m4-stagenet-net
docker run -d --name lez-m4-stagenet-monerod --network lez-m4-stagenet-net --network-alias monerod \
  -v lez-m4-stagenet:/var/lib/monero -v "$PWD/monerod.conf:/run-config/monerod.conf:ro" \
  -p 127.0.0.1:38081:38081 --cpus 2 --memory 3g --read-only --tmpfs /tmp \
  --cap-drop ALL --security-opt no-new-privileges:true \
  --entrypoint /usr/local/bin/monerod lez-m4-stagenet-monero:0.18.5.1 \
  --config-file=/run-config/monerod.conf --non-interactive
```

The `add-peer` lines matter: on the first start without them the daemon found
no peer in two minutes from its built-in seeds; with them it had four within
a minute. The peers are untrusted: `monerod` checks every block against the
Stagenet genesis, the consensus rules and its checkpoints. The
`chmod 644` lets the container's user 65532 read the file; keep the directory
holding it private.

Check it (`daemon.curlrc` holds `digest`, `user = "daemon:<password>"` and
`header = "content-type: application/json"`):

```sh
curl -s -K daemon.curlrc http://127.0.0.1:38081/json_rpc \
  -d '{"jsonrpc":"2.0","id":0,"method":"get_info"}' | jq -c '.result|{height,target_height,nettype,outgoing_connections_count}'
curl -s -K daemon.curlrc http://127.0.0.1:38081/json_rpc \
  -d '{"jsonrpc":"2.0","id":0,"method":"on_get_block_hash","params":[0]}' | jq -r .result
```

`nettype` is `stagenet` and the height-zero hash is
`76ee3cc98646292206cd3e86f74d88b4dcc1d937088645e9b0cbca84b7ce74eb`, the
Stagenet genesis that `lez_xmr_monero_adapter::MONERO_STAGENET_GENESIS_HASH`
pins. Without the login the daemon answers HTTP 401.

**How long it takes.** Stagenet's tip was at height 2,255,230 on 2026-11-21.
With two CPUs the node synced about 125,000 blocks in its first 29 minutes
(252 MB of database); the measured rate fell from about 6,000 blocks a minute
at the start to about 1,300 as the blocks grew, so a full sync takes many
hours to more than a day on such a host. The public Stagenet nodes reported a
database of 16 GB unpruned; a pruned one keeps roughly a third. Until the node
reaches the tip, a wallet on it sees nothing newer than its height.

A wallet-rpc on the self-hosted node (`wallet-rpc.pass` holds the wallet-rpc
login's password; `wallet.curlrc` is `daemon.curlrc` with
`user = "operator:<that password>"`):

```sh
cat > wallet-self.conf <<EOF
stagenet=1
wallet-dir=/var/lib/monero-wallet
rpc-bind-ip=0.0.0.0
rpc-bind-port=38083
confirm-external-bind=1
rpc-login=operator:$(cat wallet-rpc.pass)
daemon-address=monerod:38081
daemon-login=daemon:$(cat daemon.pass)
trusted-daemon=1
non-interactive=1
log-level=0
EOF
chmod 644 wallet-self.conf
docker run -d --name lez-m4-stagenet-wallet-self --network lez-m4-stagenet-net \
  -v lez-m4-stagenet-wallet-self:/var/lib/monero-wallet -v "$PWD/wallet-self.conf:/run-config/wallet.conf:ro" \
  -p 127.0.0.1:38084:38083 --cpus 1 --memory 1g --read-only --tmpfs /tmp \
  --cap-drop ALL --security-opt no-new-privileges:true \
  --entrypoint /usr/local/bin/monero-wallet-rpc lez-m4-stagenet-monero:0.18.5.1 \
  --config-file=/run-config/wallet.conf
curl -s -K wallet.curlrc http://127.0.0.1:38084/json_rpc -d '{"jsonrpc":"2.0","id":0,"method":"get_version"}'
curl -s -K wallet.curlrc http://127.0.0.1:38084/json_rpc \
  -d '{"jsonrpc":"2.0","id":0,"method":"create_wallet","params":{"filename":"self-hosted","password":"","language":"English"}}'
```

`get_version` answers `{"release":true,"version":65567}`.
`trusted-daemon=1` is right only for a daemon you run yourself.

The SDK and the adapter on this route: the Stagenet tests CI runs, pointed at
the node and the wallet-rpc (`daemon.login` and `wallet.login` hold
`user:password`; they use the wallet `self-hosted` as the role wallet), from
`$REPO`:

```sh
MONERO_STAGENET_DAEMON_URL=http://127.0.0.1:38081/ MONERO_STAGENET_DAEMON_LOGIN_FILE="$WORK/daemon.login" \
MONERO_STAGENET_WALLET_URL=http://127.0.0.1:38084/ MONERO_STAGENET_WALLET_LOGIN_FILE="$WORK/wallet.login" \
MONERO_STAGENET_ROLE_WALLET=self-hosted \
  cargo test --locked -p lez-xmr-monero-adapter --test stagenet_daemon -- --ignored --nocapture --test-threads=1
```

On 2026-11-21, with the node at height 132,209 of its sync, both passed:
the attestor bound the Stagenet genesis, the Regtest checks refused the node,
and the wallet-rpc restored the SDK's 2-of-2 Stagenet address as a view-only
wallet and refused its mainnet-byte form ([In CI](#in-ci) lists the checks).

## Route 2: a public node

A public Stagenet node serves a wallet at once, without a sync. It is
untrusted: it learns which outputs and transactions the wallet asks about and
your IP address, can withhold or delay transactions and blocks, and can lie
about anything the wallet does not check itself (the wallet checks the
outputs it receives against its keys, not that a block is on the heaviest
chain). Use it for Stagenet coins only, never with keys that hold value
elsewhere, and treat a confirmation count it reports as its word. These
answered on 2026-11-21, all at the same Stagenet height:
`node.monerodevs.org:38089`, `node.sethforprivacy.com:38089`,
`stagenet.xmr-tw.org:38081`. Each is a restricted RPC
(`"restricted":true`), which serves what a wallet and the project's verifier
read and refuses mining and administration.

```sh
openssl rand -hex 24 > wallet.pass
docker run -d --name lez-m4-stagenet-wallet-rpc --network lez-m4-stagenet-net \
  -v lez-m4-stagenet-wallet:/var/lib/monero-wallet -p 127.0.0.1:38083:38083 --cpus 1 --memory 1g \
  --read-only --tmpfs /tmp --cap-drop ALL --security-opt no-new-privileges:true \
  --entrypoint /usr/local/bin/monero-wallet-rpc lez-m4-stagenet-monero:0.18.5.1 \
  --stagenet --wallet-dir=/var/lib/monero-wallet --rpc-bind-ip=0.0.0.0 --rpc-bind-port=38083 \
  --confirm-external-bind --rpc-login=operator:$(cat wallet-rpc.pass) \
  --daemon-address=stagenet.xmr-tw.org:38081 --untrusted-daemon --non-interactive --log-level=0
```

(Here the login is on the command line; use a config file as on route 1 where
other users can read `docker inspect`.) The project's adapter talks only to
literal-loopback RPC endpoints, so the verifier reaches the same public node
through a local forwarder:

```sh
docker run -d --name lez-m4-stagenet-remote-node --network lez-m4-stagenet-net \
  -p 127.0.0.1:38089:38089 --read-only --cap-drop ALL --security-opt no-new-privileges:true --memory 64m \
  alpine/socat@sha256:beb4a68d9e4fe6b0f21ea774a0fde6c31f580dde6368939ed70100c5385b015e \
  TCP-LISTEN:38089,fork,reuseaddr TCP:node.monerodevs.org:38089
curl -s http://127.0.0.1:38089/json_rpc -d '{"jsonrpc":"2.0","id":0,"method":"get_info"}' \
  | jq -c '.result|{height,nettype,untrusted,restricted}'
```

The forwarder keeps the route untrusted; it only puts it where the adapter
accepts it. A public node limits the RPC connections of one address (three
by default), and the example's clients and the wallet-rpc together hold more:
with both on `node.monerodevs.org` the wallet's refreshes timed out and a
read was dropped, which ended the second attempt of the run. Public nodes
also stop answering one address for a while (`monerod` bans a host after RPC
errors unless it runs with `--disable-rpc-ban`, and an operator may limit the
rate in front of it; which one applied was not determined): during the third
attempt each of the three nodes above and `node2.monerodevs.org:38089`
refused this host for some minutes at some point, and the forwarder was restarted towards
the next one that answered (`docker rm -f lez-m4-stagenet-remote-node`, then
the `docker run` above with another target), at the end by a loop that
checked it every 30 s. The example rereads through such gaps. The third
attempt had the wallet-rpc on `stagenet.xmr-tw.org:38081` throughout and the
verifier on whichever node the forwarder pointed at; two nodes can disagree
on the tip for a moment, which the verifier reads as "not yet" and rereads.

## Stagenet coins

Create a wallet and read its address (a Stagenet address starts with `5`):

```sh
req=$(jq -cn --arg p "$(cat wallet.pass)" \
  '{jsonrpc:"2.0",id:0,method:"create_wallet",params:{filename:"funding",password:$p,language:"English"}}')
curl -s -K wallet.curlrc http://127.0.0.1:38083/json_rpc -d "$req"
curl -s -K wallet.curlrc http://127.0.0.1:38083/json_rpc \
  -d '{"jsonrpc":"2.0","id":0,"method":"get_address"}' | jq -r .result.address | tee funding.address
```

The first `get_address` on a new wallet waited for the wallet's first refresh
through the public node, a few minutes. The XMR-TW faucet,
<https://stagenet-faucet.xmr-tw.org/>, paid 0.1 XMR within a minute on
2026-11-21: paste the address into its form, or send the request its page
sends:

```sh
curl -s -X POST -H 'content-type: application/json' https://stagenet-faucet.xmr-tw.org/send_tx/ \
  -d "{\"addr\":\"$(cat funding.address)\"}"
```

It answers with the transaction id. The faucet sees the address and your IP
address. Watch the coins arrive and unlock (ten blocks after the block that
holds them):

```sh
curl -s -K wallet.curlrc http://127.0.0.1:38083/json_rpc \
  -d '{"jsonrpc":"2.0","id":0,"method":"get_transfers","params":{"in":true,"pool":true}}'
curl -s -K wallet.curlrc http://127.0.0.1:38083/json_rpc \
  -d '{"jsonrpc":"2.0","id":0,"method":"get_balance"}' | jq -c '.result|{balance,unlocked_balance,blocks_to_unlock}'
```

The Rino community faucet that older Monero documentation names did not
resolve on 2026-11-21. Mining is the other source: Stagenet's difficulty was
about 2.4 million on 2026-11-21, so one desktop at a few thousand RandomX
hashes a second finds a block (0.6 XMR) in the order of a quarter to half an
hour on average, and only on a synced node (`start_mining` on the daemon's
RPC). Mining was not tried here.

## The Monero leg on Stagenet

[`examples/full-lifecycle.rs`](../crates/xmr-reference-actor/examples/full-lifecycle.rs)
runs two swaps on the types the Nodes run (`xmr-reference-actor`,
`lez-adaptor-role-runner`, `lez-xmr-swap-sdk`, `lez-xmr-monero-adapter`): a
completed one and a refunded one, each settled by a role that has dropped its
session state and rebuilds the Monero spend key from what it persisted and
the public LEZ signature. With `LEZ_XMR_EXAMPLE_NETWORK=stagenet` it agrees
under the `PublicStagenet` profile, derives the shared address in Stagenet
encoding, checks the daemon's genesis and `get_info` network, funds the
shared address with one transfer that Stagenet's miners confirm, waits for
ten confirmations, and sweeps the same way. The LEZ escrow is modelled in
memory with its rules and a model LEZ clock (the devnet's escrow runs inside
the Node containers and is not reachable from the host).

`logins/` holds `daemon.login` (`user:password`; a public node has no login,
so any value) and `funding.login` (`operator:<wallet-rpc password>`); from
`$REPO`:

```sh
LEZ_XMR_EXAMPLE_NETWORK=stagenet \
LEZ_XMR_MONEROD_URL=http://127.0.0.1:38089/ \
LEZ_XMR_FUNDING_WALLET_URL=http://127.0.0.1:38083/ \
LEZ_XMR_MONERO_LOGINS="$WORK/logins" \
LEZ_XMR_FUNDING_WALLET=funding \
LEZ_XMR_FUNDING_WALLET_PASSWORD_FILE="$WORK/wallet.pass" \
  cargo run --locked -p xmr-reference-actor --example full-lifecycle
```

On the self-hosted route, once it is at the tip, point `LEZ_XMR_MONEROD_URL`
at `http://127.0.0.1:38081/` with its login and `LEZ_XMR_FUNDING_WALLET_URL`
at the wallet-rpc of route 1. The keys come from the seeded stream named by
`LEZ_XMR_EXAMPLE_SEED` (default `full-lifecycle`): anyone who knows the seed
can rebuild the shared spend key and sweep the output first, so set your own
seed when the coins matter to you. The Stagenet run moves 0.02 and 0.01 XMR,
a tenth of the regtest amounts, and needs a little more than 0.03 XMR
unlocked.

What to expect, in order (the run's own lines are in
[the run](#the-run-of-2026-11-21)):

1. `Monero leg: LIVE Stagenet` with the genesis `76ee3cc9…` and the tip. A
   daemon of another network stops it here: the genesis check refuses
   anything but Stagenet's, and `get_info` must say `stagenet`.
2. The six Chat rounds, R1 to R6, in a second; the shared 2-of-2 address
   starts with `5`.
3. `tag 13 + fund`, then the Maker's check that the LEZ lock is final before
   C. Times are on the model LEZ clock: the `fast` windows times 108, C, R,
   P = +34,560, +77,760 and +108,000 s.
4. `Maker: sent … to the shared address … in <txid> at Stagenet height …`.
5. The Taker's view-only wallet waits, printing what it lacks (not in the
   wallet yet, in the pool, fewer than ten confirmations) with the tip, until
   `Taker: view-only wallet sees the exact output: block … at height …, 10
   confirmations`. Expect 20 minutes and more; see the timings.
6. Swap 1: tags 14 and 15, the Maker's claim, the Taker drops its state,
   rebuilds the key from the claim signature and sweeps the shared output to
   the funding wallet; the sweep waits for its own ten confirmations. Swap 2
   (it has not completed on Stagenet): no claim, the Taker's refund in
   [R, P), the Maker rebuilds the key from the refund signature and sweeps
   its Monero back.
7. The two outcome lines and `private_key_bytes_printed=false`.

## The Maker and Taker Nodes

The Nodes cannot be pointed at Stagenet in this release. Their Monero role
takes swaps only under the `accelerated-regtest` profile: the Maker's and the
Taker's chain-identity attestors are built for `MoneroNetwork::Regtest`
(`crates/maker-node/src/xmr_reserve.rs`,
`crates/taker-node/src/taker_service/xmr_dynamic.rs`), and before the Maker
funds, `MoneroTopologyVerifier` requires a peerless, offline `fakechain`
daemon. A Node given a Stagenet daemon refuses it at both: the Regtest
attestor refuses the Stagenet genesis (`GenesisOfAnotherNetwork`) and its
`get_info` network check fails (`DaemonNetworkMismatch`), and the topology
check fails with `DaemonProfileMismatch`. CI proves both refusals against a
real Stagenet daemon (`tests/stagenet_daemon.rs`). The Nodes' wallet effects
also mine their own confirmations (`generateblocks`), which only regtest
allows. What a Stagenet Node needs beyond that: a topology check for a
daemon that has peers (synced, not `busy_syncing`, the header trust flag of
`HEADER_TRUST_RESIDUAL` preserved), waiting for confirmations instead of
mining them in the Maker and Taker actors, windows sized as below in the
Node configuration, and the Taker and Maker desks and CLIs offering the
profile. The Nodes' regtest configuration is in the
[deployment guide](../deploy/README.md#monero-lez_monero1).

## Timings

Stagenet targets a block every 120 s. The swap waits for the funding
transaction to enter a block and then for ten confirmations
(`REQUIRED_MONERO_CONFIRMATIONS`), 11 blocks: 22 minutes on average. Block
intervals are random: at the target rate, 11 blocks take more than 30 minutes
in 12 % of cases, more than 40 minutes in 1.1 % and more than an hour in
0.002 %. Stagenet's hashrate is small and moves: on 2026-11-21 five blocks
took 25 minutes, five times the target, and a slow stretch stretches every
wait by the same factor.

The windows are LEZ times fixed in Stage A (`XmrWindowsV1`). In the order of
the flow ([ADR 0217](architecture/0217-xmr-taker-sells-lez-deviates-from-the-common-flow.md)):

- **C, the Maker's funding cutoff.** The Maker funds only after the Taker's
  LEZ lock is final and only before C. C leaves time for the LEZ lock's
  finality and the Maker's reaction.
- **R, the refund time.** Between C and R the Monero output must confirm ten
  times, the Taker must verify it and authorise the claim (tag 14), and the
  Maker must claim (tag 15) before its own claim deadline R − (R − C)/2. So
  (R − C)/2 must cover the funding's inclusion and ten confirmations at a
  slow rate, plus both roles' reactions and LEZ inclusion. The
  `PublicStagenet` profile requires R − C of at least 12 hours
  (`STAGENET_MINIMUM_FUNDING_TO_REFUND_MS`), which leaves 6 hours for that
  first half: about 16 times the 22-minute average.
- **P, the punish time.** The Taker's refund (tag 16) is open only in
  [R, P); from P the Maker can punish (tag 17). P − R must cover the Taker's
  reaction and LEZ inclusion, and nothing on Monero; the profile requires at
  least 2 hours (`STAGENET_MINIMUM_REFUND_TO_PUNISH_MS`).

The devnet's regtest windows (+320, +720, +1,000 s on `fast`) are far below
these and are refused under `PublicStagenet` (`InvalidWindows`). The example
scales them by 108 to C, R, P = +9.6 h, +21.6 h, +30 h. A Stagenet swap that
completes takes about 22 minutes of Monero waiting plus the sweep's own ten
confirmations; a refund cannot start before R, 12 hours or more after C.

## Refund and punish

A Monero output has no time lock: the only way back for the Maker's Monero is
the Taker's share, which the Taker's LEZ refund signature reveals. On
Stagenet as on regtest:

- **Refund.** No claim by R: the Taker refunds its LEZ in [R, P) and its
  signature gives the Maker the Taker's share; the Maker rebuilds the spend
  key and sweeps the shared output back (swap 2 of the example, run on
  regtest; on Stagenet it stopped at the funding).
- **Punish.** No refund by P: the Maker takes the LEZ (tag 17) and the
  Monero stays at the shared address, which neither role can spend alone
  (TM-0031). On Stagenet this is a real loss of the funded coins.
- **A lost state.** A role that lost its journal rebuilds the key from its
  own share, the shared view key, Stage A and Stage B and the public LEZ
  signature, as both swaps of the example do. A lost share is not
  recoverable.

## In CI

The `monero-stagenet` job of `.github/workflows/ci.yml` runs
[`scripts/run-monero-stagenet-ci.sh`](../scripts/run-monero-stagenet-ci.sh)
on every pull request: it verifies the release of the runner's
architecture with `scripts/verify-monero-release.sh`, builds the image, starts `monerod
--stagenet` (syncing from public peers, the data on a tmpfs) and
`monero-wallet-rpc --stagenet` with Digest logins, checks the daemon's
network, version and genesis and the wallet-rpc's refusal of a wrong login,
and runs `crates/xmr-monero-adapter/tests/stagenet_daemon.rs`:

- the Stagenet attestor binds the Stagenet genesis and its `get_info` check
  passes; the Regtest attestor, its network check and the Regtest topology
  check, which a Node runs, refuse the same daemon;
- the SDK's 2-of-2 shared address in Stagenet encoding (prefix `5`, network
  byte Stagenet) restores in the Stagenet wallet-rpc as a view-only wallet
  that returns the same address; the same keys in the Regtest encoding
  (mainnet bytes) are refused by it;
- the Stagenet output verifier refuses a mainnet-byte destination before any
  RPC, and reads the real wallet and daemon for a Stagenet one.

Then the daemon must reach 1,000 blocks of the public Stagenet chain within
15 minutes (`MONERO_STAGENET_SYNC_HEIGHT`, `MONERO_STAGENET_SYNC_SECONDS`).
The job does not sync to the tip, holds no coins and runs no swap; it
depends on public Stagenet peers answering. Run locally in an arm64 Linux
container on a Mac (the arm64 release) on 2026-11-21, it passed: both tests,
then 3,101 blocks from three peers in 47 s.

## The run of 2026-11-21

Three attempts, 2026-11-21, all with the default seed, against public nodes
(times UTC). The third attempt ran the example as built from commit
`08cea128`.

1. 14:42. Funded the shared address
   `57s8dfKQoWLJK7Jn…` with 0.02 XMR in
   `349bc8c3c3cdf8da124af1cadb9c1d82552e702c16a1e0bcd1846d1c643c9f83`
   (height 2,255,230); then the view-only wallet's first refresh through the
   public node outlasted the 30 s RPC deadline and the example stopped. It
   now treats a slow refresh as one more wait.
2. 14:53. Funded the same address again with 0.02 XMR in
   `fe39de2e0e1c3d6f5b67fc4d6c3ef6925bbcfb9bd3b2efbe82b4d295a03c2250`
   (height 2,255,241); with the wallet-rpc and the verifier on one public
   node a read was dropped and the example stopped. It now rereads after a
   failed read.
3. 15:21. **Swap 1, the claim, completed on Stagenet:**

   ```text
   [swap 1 claim | LEZ +  6480 s] Maker: sent 20000000000 piconero to the shared address 57s8dfKQoWLJK7Jn… in 0e81a26340d24dd89d386b393d8db1f0d83e309999cd76b07d9b2b9bcfd1a852 at Stagenet height 2255252
   [swap 1 claim | LEZ +  9720 s] Taker: view-only wallet sees the exact output: block 4328c528618bc9d6… at height 2255253, 10 confirmations
   [swap 1 claim | LEZ + 43200 s] tag 14: Taker published its claim partial, escrow XmrClaimAuthorized
   [swap 1 claim | LEZ + 48600 s] tag 15: Maker completed the claim signature with its share; LEZ 500 to the Maker, escrow Claimed
   [swap 1 claim |             ] Taker (recovered): claim presignature re-aggregated from Stage B and the tag-14 partial; Maker's share extracted from tag 15, checked against its DLEQ proof; spend key opens 9f3c088d2fe61567…
   [swap 1 claim |             ] swept the shared output with the rebuilt key: tx 59fdd682ef0b6fe83ece91b273b333e59611c58bc8c76dbff53b0dfb9aed0e07 received 19969560000 piconero (fee 30440000), in block fb587e4dfdc1fb32… at Stagenet height 2255265, 10 confirmations
   ```

   By the block timestamps, the funding entered block 2,255,253 at 15:25:50,
   four minutes after it was sent, and had its tenth confirmation at
   15:48:48, 27 minutes after it was sent; the sweep entered block 2,255,265
   at 15:51:21 and had its tenth at 16:04:18. The three attempts had funded the same
   address three times, as the seed repeats the keys; the sweep spends one
   output of the amount, by the adapter's rule the earliest confirmed one,
   the first attempt's. The other two stay at the address.

   **Swap 2, the refund, did not complete:** after its Stage A and Stage B
   and the modelled LEZ lock, the Maker's funding transfer was refused by
   the wallet-rpc's public node (`transaction was rejected by daemon`, no
   reason given), and the example stopped. The refund path, the Maker
   rebuilding the key from the Taker's refund signature and sweeping its
   Monero back, has run on regtest only (D-2 of the
   [M4 checklist](m4-acceptance-checklist.md)).

The self-hosted node had synced 137,850 of 2,255,265 blocks when the run
ended.

## Stopping and removing

```sh
docker rm -f lez-m4-stagenet-monerod lez-m4-stagenet-wallet-rpc lez-m4-stagenet-wallet-self lez-m4-stagenet-remote-node
docker network rm lez-m4-stagenet-net
docker volume rm lez-m4-stagenet lez-m4-stagenet-wallet lez-m4-stagenet-wallet-self
docker image rm lez-m4-stagenet-monero:0.18.5.1
```

These commands were not run in the session: the node was left syncing. The
volumes hold the synced chain and the wallets; keep them to resume. The
wallet files hold Stagenet keys; delete them with the volume.

## Known limits

- No Node-driven swap on Stagenet; the Nodes refuse it
  ([above](#the-maker-and-taker-nodes)).
- The LEZ side of the Stagenet run is the example's in-memory escrow model,
  not the devnet's escrow and not LEZ testnet.
- On Stagenet the claim path ran end to end; the refund path did not.
- The Stagenet run used public nodes, which are trusted for what they
  report; the self-hosted node was not at the tip during the session, so no
  funds moved on that route.
- `VIEW_ONLY_SPEND_RESIDUAL`: a view-only wallet cannot prove that an output
  is still unspent; the swap relies on a fresh shared address per swap.
- CI's Stagenet job depends on public peers and syncs a bounded prefix of the
  chain, not the tip.
- 0.18.5.1 is the pinned release; Stagenet's next hard fork would need the
  release that follows it.
