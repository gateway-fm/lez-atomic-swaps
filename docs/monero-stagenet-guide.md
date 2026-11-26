# Monero Stagenet: monerod, wallet-rpc and a Node-driven XMR/LEZ swap

This guide sets up the pinned Monero release on public Monero Stagenet, by a
self-hosted `monerod` or through a public node, gives each role its
`monero-wallet-rpc`, gets Stagenet coins, and runs XMR/LEZ swaps between the
Maker and Taker Nodes with Monero on Stagenet and LEZ on the devnet. It also
runs the Monero leg alone with the project's SDK. It is RFP-003 U9 and the M4
deliverable "`monerod` + `monero-wallet-rpc` stagenet setup documentation"
([rfp#124](https://github.com/logos-co/rfp/issues/124)).

What has run on Stagenet, and what has not:

- **Run on Stagenet, through the self-hosted node.** On an Apple-silicon Mac
  with Docker Desktop, on 2026-11-24 to 26: the release verification, the
  image, the pruned self-hosted `monerod` of route 1 synced to the Stagenet
  tip (about 19 hours, 6.4 GiB), a `monero-wallet-rpc` per role on it, and
  the devnet stack with both Nodes on it, built from `f259e791` without the
  devnet test hooks. Under the windows below (C, R, P = +10 min, +2 h,
  +4 h) the Nodes completed a swap (34 minutes from the take), refunded one
  by the Taker Node's own refund at R (the Maker's Monero swept back) and
  punished one whose Taker Node was stopped through [R, P), every Monero
  read and transaction of theirs going through that node
  ([the run](#the-node-driven-run-of-2026-11-26-on-the-self-hosted-node),
  [its evidence](evidence/stagenet-0.4.0-2026-11-26/)).
- **Run on Stagenet, through public nodes.** On 2026-11-21: route 2, wallets
  funded from a public faucet, and the devnet stack with both Nodes on public
  nodes. On 2026-11-22 Nodes built from `f059d3fa` completed, refunded and
  punished a swap there under the same windows, and refunded one the Maker
  could not fund ([that run](#the-node-driven-run-of-2026-11-22)); a first
  run on 2026-11-21 had done the same under an earlier 30-minute P − R
  ([that run](#the-node-driven-run-of-2026-11-21)). The SDK example ran the
  Monero leg of a completed swap alone, with the LEZ escrow modelled
  ([its run](#the-sdk-examples-run-of-2026-11-21)). CI starts the same
  `monerod` on Stagenet in every run, without syncing it to the tip
  ([below](#in-ci)).
- **Not run.** LEZ was the devnet's, not LEZ testnet, and one operator ran
  both Nodes, and the one `monerod` both read, on one host. No Monero testnet
  or mainnet run exists.

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
grep linux-armv8 "$REPO/tests/e2e/monero/release/hashes-v0.18.5.1.txt"
shasum -a 256 monero-linux-armv8-v0.18.5.1.tar.bz2
```

`gpg` prints `Good signature from "binaryFate <binaryfate@getmonero.org>"`
and the key fingerprint above (GnuPG 2.4 labels it `Primary key
fingerprint:`, 2.5 prints it alone on the next line), with a warning that the
key is not certified, which only says that you have not signed it yourself.
The archive's SHA-256 must equal the line `grep` prints. On x86_64 replace
`linux-armv8` by `linux-x64`.
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
umask 077
openssl rand -hex 24 > daemon.pass
openssl rand -hex 24 > wallet-rpc.pass
printf 'daemon:%s\n' "$(cat daemon.pass)" > daemon.login
printf 'operator:%s\n' "$(cat wallet-rpc.pass)" > wallet.login
printf 'digest\nuser = "daemon:%s"\nheader = "content-type: application/json"\n' "$(cat daemon.pass)" > daemon.curlrc
printf 'digest\nuser = "operator:%s"\nheader = "content-type: application/json"\n' "$(cat wallet-rpc.pass)" > wallet.curlrc
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
  -p 127.0.0.1:38081:38081 --cpus 3 --memory 3g --read-only --tmpfs /tmp \
  --cap-drop ALL --security-opt no-new-privileges:true --restart unless-stopped \
  --entrypoint /usr/local/bin/monerod lez-m4-stagenet-monero:0.18.5.1 \
  --config-file=/run-config/monerod.conf --non-interactive
```

The `add-peer` lines matter: on the first start without them the daemon found
no peer in two minutes from its built-in seeds; with them it had four within
a minute. The peers are untrusted: `monerod` checks every block against the
Stagenet genesis, the consensus rules and its checkpoints. The
`chmod 644` lets the container's user 65532 read the file; keep the directory
holding it private (`umask 077` above makes the login files `0600`).
`--restart unless-stopped` lets the sync go on after a restart of Docker or
of the host; it resumes from the database. On Docker Desktop the daemon logs
`The blockchain is on a rotating drive`: the virtual disk says so of itself,
and the warning changes nothing.

Check it:

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

To keep a record of whom the node served, and which transactions reached it
(for a run's evidence), raise the log of its RPC server and its pool while it
runs; `log-level=0` logs neither:

```sh
curl -s -K daemon.curlrc http://127.0.0.1:38081/set_log_categories \
  -d '{"categories":"*:WARNING,global:INFO,daemon.rpc:INFO,net.http:DEBUG,txpool:DEBUG"}'
```

Each request then logs `HTTP [<client IP>] POST /json_rpc` and `calling RPC
method <method>` (or `calling /<path>`), with the caller's address on the
Docker network, and each transaction the pool takes logs its id; `docker logs
--timestamps lez-m4-stagenet-monerod` keeps them, and so does `bitmonero.log`
in the data directory. `set_log_level` with `{"level":0}` turns it back.

**How long it takes, and how much disk.** A full sync checks every block's
proof of work and transactions, and takes the CPUs it is given: the peers
were never the limit (thousands of downloaded spans waited for
verification). On 2026-11-24 and 25 the node above, on an Apple-silicon Mac
whose Docker also ran other stacks, reached the tip, height 2,258,356, in
about 19 hours of running: 3 CPUs for its first ten hours, at 700 to 4,800
blocks a minute depending on the host's other load, then 6 CPUs. Its pruned
database was then 6.4 GiB on disk; reserve at least 10 GB of Docker disk for
it. (A restricted public node reports its `database_size` rounded up to a
multiple of 5 GiB, so its 16.1e9 bytes say little about an unpruned
database.) Until the node reaches the tip, a wallet on it sees nothing newer
than its height. `get_info` then reads `synchronized: true` and
`target_height: 0`; compare its top block hash with a public node's at the
same height (`get_block_header_by_height`). A Node on this route waits for
the tip: its topology check refuses a daemon still syncing
(`DaemonNotSynchronized`). An earlier attempt, on 2026-11-21, ran six hours
on 2 CPUs, reached height 647,823 of 2,255,424 and was stopped and removed
when the host's Docker disk ran full.

A wallet-rpc on the self-hosted node, behind the wallet-rpc login made
above:

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
the node and the wallet-rpc (`daemon.login` and `wallet.login`, made above;
they use the wallet `self-hosted` as the role wallet), from `$REPO`:

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
The same three tests, with the third that checks the Stagenet topology,
passed at height 647,823 of that sync (the topology check refused the
syncing node) and, with `MONERO_STAGENET_EXPECT_SYNCED=1`, through public
nodes at the tip (it admitted them).

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

## The windows

The windows are LEZ times fixed in Stage A (`XmrWindowsV1`) as offsets from
the Maker's LEZ anchor at take time. In the order of the flow
([ADR 0217](architecture/0217-xmr-taker-sells-lez-deviates-from-the-common-flow.md)):
the Maker funds only after the Taker's LEZ lock is final and while the
finalized LEZ time is before C; the Taker verifies the output at ten
confirmations and authorises the Maker's claim (tag 14) before R; the Maker
claims (tag 15) only on a tag 14 it sees final before its claim deadline
D = R − (R − C)/2, so that tag 15 lands before R; the Taker refunds (tag 16)
in [R, P); from P the Maker punishes (tag 17).

Assumptions, for Stagenet and the devnet's LEZ:

- Stagenet targets a block every 120 s. The funding waits for its block and
  ten confirmations: 11 block intervals, 22 minutes on average. At the target
  rate 11 intervals take more than 40 minutes in 1.1 % of cases, more than 45
  in 0.27 %, more than 50 in 0.06 % and more than 60 in 0.002 % (a sum of 11
  exponential intervals). Stagenet's hashrate is small and moves: on
  2026-11-21 five blocks once took 25 minutes; at 1.5 times the target
  interval 11 blocks take more than 45 minutes in 12 % of cases.
- LEZ finalizes in seconds on the devnet; one Maker drive and one Taker
  supervisor tick are counted at 80 s each, and a LEZ inclusion at a step of
  20 s, as on regtest (`deploy/scripts/gen-config.sh`). A supervisor sweep
  runs every swap's refund before any Monero read, and a Monero step of the
  Taker waits at most 30 s for its wallet-rpc and takes at most 2 minutes:
  a public node that stalls does not hold a refund back.
- A late Monero confirmation costs liveness, not funds: a tag 14 seen after D
  is not claimed, and the swap falls back to the Taker's refund at R, in
  which the Maker sweeps its Monero back. A Taker that cannot land its
  refund in [R, P) loses its LEZ (TM-0031); P − R is the only window whose
  shortness costs funds.

What each window must hold:

| Window | Holds | Sized |
|---|---|---|
| C (from the take) | the take's Chat rounds, the Taker's LEZ lock final, the Maker's drive that funds (one submit-only transfer; nothing to mine) | 10 min: the regtest C (+320 s) covers the same steps with mining included |
| (R − C)/2, from C to D | a funding sent just before C: its block and ten confirmations, the Taker's verify and tag 14 final, the Maker's drive that sees it | 55 min: 45 min of blocks (0.27 % later at the target rate) and 10 min of LEZ steps and drives; the funding of a swap is normally sent a few minutes after the take, so the blocks have C's remainder as well |
| (R − C)/2, from D to R | the Maker's claim tool and tag 15 final | the same 55 min (the rule halves R − C); minutes are used |
| P − R | the Taker Node's refund from R, sent again on every tick until final, through an outage of that Node, its host or its LEZ sidecar | 2 h: the reviewed M1 profile's; minutes of LEZ work, the rest for the outage |

`PublicStagenet` admits R − C ≥ 100 min (2 × (45 + 5)) and P − R ≥ 2 h
(`STAGENET_MINIMUM_FUNDING_TO_REFUND_MS`, `STAGENET_MINIMUM_REFUND_TO_PUNISH_MS`
in `crates/xmr-swap-sdk/src/agreement_v1.rs`); a role config with shorter
windows is refused at load. The devnet's Stagenet profile
(`LEZ_MONERO_NETWORK=stagenet`, `deploy/scripts/gen-config.sh`) is C, R,
P = +10 min, +2 h, +4 h. A completed swap takes the funding's
confirmations, the Maker's claim and the Taker's sweep with its own ten
confirmations: about an hour. A refund cannot start before R, two hours
after the take, and a punishment before P, four hours after it. P − R is the
Taker's safety window: a Taker Node down for less than two hours from R
still refunds, and a longer wait for the punishment costs the Maker only
time. The Node-driven run of 2026-11-22 ran under these windows; the first
run, of 2026-11-21, under an earlier draft of the profile, P = +2.5 h
(P − R 30 minutes).

The Nodes' own timeouts on Stagenet: a Maker Monero tool runs at most
4 minutes (2 on regtest) and a Maker actor attempt 5 (2 on regtest), as a
wallet restored through a public node pulls Stagenet's block hashes to its
restore height and may wait on a node that stops answering; the Maker's
funding reads `maker_funding_in_doubt` only from D (from C on regtest), as
its confirmations come after C. The view-only and reconstructed wallets
restore from a height, never from genesis: the Taker from the Maker's
monerod tip at take time (checked against its own), the Maker from the last
block stamped six hours before C, found by bisection over block headers
(`agreement_restore_height`), so its tools agree without a stored height.

## The Nodes on Stagenet

The Maker and Taker Nodes take a Monero swap under the profile of their
Monero role configuration (`xmr-role.json`): `accelerated-regtest` (the
devnet's offline regtest chain) or `public-stagenet`. Mainnet and testnet
are refused by name there, by the launcher (`LEZ_XMR_NETWORK`), by
`gen-config.sh` (`LEZ_MONERO_NETWORK`) and by `MoneroNetwork::from_str`. The
profile goes into the Stage-A plan and so into Stage A, which both parties
sign: a Taker refuses a Maker's Stage A of another profile before anything
else, and the genesis both Nodes read must be the same. What a Node does on
Stagenet that it does not on regtest:

- **It mines nothing.** Only a `MoneroRegtestMining` capability reaches
  `generateblocks`, and one exists only for a Regtest chain whose daemon
  names the fakechain in its own `get_info`; on Stagenet the Maker's funding
  tool submits one transfer and exits, the funding observer records the
  funding at ten confirmations, and every sweep is submitted once and
  observed.
- **It checks a daemon that syncs.** Each role's attestor checks the
  Stagenet genesis and the daemon's `get_info` network at take time; the
  Maker's funding receipt, and the Taker's record of the funding, its tag 14
  and its claim sweep, also need the daemon synchronized, at its target
  height, and its tip its own and at most an hour old by the host clock
  ([below](#a-daemon-the-node-does-not-run)).
- **It waits for nobody's Monero reads to refund.** A Taker sweep runs every
  swap's refund first, and each Taker Monero step has a time budget.
- **It names the network.** The Taker's swap view carries `monero_network`;
  the Taker CLI and desk name Stagenet and its waits beside the swap, and the
  desks' Monero rows say how long ten confirmations take there.

### Each role's wallet-rpc

Each role needs a `monero-wallet-rpc` of its own: its tools switch the open
wallet between the role wallet and the swap's view-only and reconstructed
wallets, under a lock no other process holds. The role wallets are named
after the roles (`maker`, `taker`) and have no password, as on regtest. The
Maker's wallet must hold unlocked Stagenet coins, one output of at least the
lot and its fee per swap in flight: a funding's change is locked for ten
blocks.

On the self-hosted route both wallet-rpcs read the operator's `monerod`:
`wallet-maker.conf` and `wallet-taker.conf` are `wallet-self.conf`, each with
its role's own login (`maker-wallet-rpc.pass`, `taker-wallet-rpc.pass`), the
same `daemon-address=monerod:38081`, `daemon-login` and `trusted-daemon=1`.
Each mounts its role's wallet volume, and a wallet volume is mounted by one
wallet-rpc at a time:

```sh
for role in maker taker; do
  openssl rand -hex 24 > $role-wallet-rpc.pass
  printf 'operator:%s\n' "$(cat $role-wallet-rpc.pass)" > $role-wallet-rpc.login
  printf 'digest\nuser = "operator:%s"\nheader = "content-type: application/json"\n' "$(cat $role-wallet-rpc.pass)" > $role-wallet.curlrc
  sed "s/^rpc-login=.*/rpc-login=operator:$(cat $role-wallet-rpc.pass)/" wallet-self.conf > wallet-$role.conf
  chmod 644 wallet-$role.conf
done
docker run -d --name lez-m4-stagenet-wallet-maker --network lez-m4-stagenet-net \
  -v lez-m4-stagenet-wallet:/var/lib/monero-wallet -v "$PWD/wallet-maker.conf:/run-config/wallet.conf:ro" \
  -p 127.0.0.1:38083:38083 --cpus 1 --memory 1g --read-only --tmpfs /tmp \
  --cap-drop ALL --security-opt no-new-privileges:true --restart unless-stopped \
  --entrypoint /usr/local/bin/monero-wallet-rpc lez-m4-stagenet-monero:0.18.5.1 \
  --config-file=/run-config/wallet.conf
```

and the same for the Taker, `lez-m4-stagenet-wallet-taker` on the volume
`lez-m4-stagenet-wallet-taker` and `127.0.0.1:38085`.

On route 2, route 2's wallet-rpc serves the Maker; a second one serves the
Taker (`wallet-taker.conf` is `wallet-self.conf` with the Taker's login and a
public node as `daemon-address`, `untrusted-daemon=1` instead of
`trusted-daemon=1`, and no `daemon-login`):

```sh
openssl rand -hex 24 > taker-wallet-rpc.pass
docker volume create lez-m4-stagenet-wallet-taker
docker run -d --name lez-m4-stagenet-wallet-taker --network lez-m4-stagenet-net \
  -v lez-m4-stagenet-wallet-taker:/var/lib/monero-wallet -v "$PWD/wallet-taker.conf:/run-config/wallet.conf:ro" \
  -p 127.0.0.1:38085:38083 --cpus 1 --memory 1g --read-only --tmpfs /tmp \
  --cap-drop ALL --security-opt no-new-privileges:true \
  --entrypoint /usr/local/bin/monero-wallet-rpc lez-m4-stagenet-monero:0.18.5.1 \
  --config-file=/run-config/wallet.conf
curl -s -K wallet.curlrc http://127.0.0.1:38083/json_rpc \
  -d '{"jsonrpc":"2.0","id":0,"method":"create_wallet","params":{"filename":"maker","password":"","language":"English"}}'
curl -s -K taker-wallet.curlrc http://127.0.0.1:38085/json_rpc \
  -d '{"jsonrpc":"2.0","id":0,"method":"create_wallet","params":{"filename":"taker","password":"","language":"English"}}'
```

Fund `maker` from the funding wallet, several outputs at once (here four of
0.015 XMR), and wait the ten blocks of their unlock:

```sh
curl -s -K wallet.curlrc http://127.0.0.1:38083/json_rpc -d "$(jq -cn --arg a "$MAKER_ADDRESS" \
  '{jsonrpc:"2.0",id:0,method:"transfer",params:{destinations:[range(4)|{address:$a,amount:15000000000}],priority:0}}')"
```

### Each role's daemon

Each Node reads its monerod over its loopback 18081, which the launcher
forwards to `host:port` (`LEZ_MONEROD_HOST`). On the self-hosted route that is
the operator's `monerod` on its Docker network (`LEZ_XMR_STAGENET_MONEROD`,
[the stack](#the-stack)), and `logins/monerod.login` holds its Digest login
(`daemon.login` above); nothing stands between the Nodes and it.

On route 2, through public nodes: a public Stagenet node limits
the RPC connections of one address: on 2026-11-21 of six concurrent
requests `stagenet.xmr-tw.org` answered one, `node.monerodevs.org` and
`node2.monerodevs.org` three or four, and the Node's tools open several
clients at once (the attestor, the wallet effects, the verifier, the
topology check), so through a plain forwarder most of the Maker's reads
failed with `error sending request`. A forwarder that holds one kept-alive
connection to the node and queues the Nodes' requests on it serves them; it
is still the remote node's answers (one per role, on two different nodes):

```sh
cat > maker-forward.cfg <<'EOF'
global
  maxconn 200
defaults
  mode http
  timeout connect 10s
  timeout client 180s
  timeout server 120s
  timeout queue 170s
  timeout http-keep-alive 60s
  retries 3
frontend f
  bind :38089
  default_backend b
backend b
  http-reuse always
  server primary node2.monerodevs.org:38089 maxconn 1
EOF
chmod 644 maker-forward.cfg
docker run -d --name lez-m4-stagenet-remote-node --network lez-m4-stagenet-net -p 127.0.0.1:38089:38089 \
  --read-only --tmpfs /tmp --cap-drop ALL --security-opt no-new-privileges:true --memory 64m \
  -v "$PWD/maker-forward.cfg:/usr/local/etc/haproxy/haproxy.cfg:ro" haproxy:3.0-alpine
```

and the same, `taker-forward.cfg` with `node.monerodevs.org:38089`, as
`lez-m4-stagenet-remote-node-taker` on `127.0.0.1:38090`. The forwarder
carries reads only: the transfers go through each role's wallet-rpc, which
reaches its own node (`set_daemon` moves it when that node stops answering).

### The stack

The devnet's stack with its Nodes on Stagenet
([deployment guide](../deploy/README.md#monero-stagenet-lez_monero_networkstagenet)):
`logins/` holds `monerod.login`, `maker-wallet-rpc.login` and
`taker-wallet-rpc.login` (`user:password`; a public node takes any), which
`gen-config.sh` copies beside the roles' identities, so no login is on a
command line.

The images: the stack's own, built from the checkout without the devnet test
hooks (`LEZ_TEST_HOOKS` unset; the regression's images carry them), under an
image prefix of their own so that the devnet's stay as they are. From
`$REPO`, with the devnet's workspace (`$WORKSPACE`, `from-scratch.sh`) and
the stack's variables below exported:

```sh
git submodule update --init --recursive
unset LEZ_TEST_HOOKS; export LEZ_MONERO=1
for phase in nix rust build stage; do bash deploy/scripts/from-scratch.sh --workspace "$WORKSPACE" --only $phase; done
(cd deploy && bash scripts/gen-config.sh runtime && set -a && . runtime/runtime.env && set +a && docker compose build)
for role in maker taker; do docker run --rm --entrypoint cat lez-m4-stagenet-$role-node:local /usr/local/share/lez/build-source.txt; done
```

Both images name the checkout's commit, and their Node binaries and sidecar
hold no `LEZ_TEST_HOOKS` or `test_hooks` string
(`grep -c -a -E 'LEZ_TEST_HOOKS|test_hooks|test-hooks'` over
`/var/lez/basecamp-<role>-user/modules/lez_<role>_node/bin/*` gives 0; it
gives 1 or 2 on the devnet's images built with `LEZ_TEST_HOOKS=1`). The
first `gen-config.sh`, before the network image exists, warns that the new
chain keeps the upstream genesis time; `up.sh --fresh-lez` below runs it again
with the image.

Then, from `$REPO/deploy`, as a second stack beside a devnet that may
already run, on the self-hosted route:

```sh
export LEZ_STACK_PREFIX=lez-m4-stagenet-dev LEZ_STACK_PORT_OFFSET=3000
export LEZ_MARKET_ROOT="$WORK/market" LEZ_WALLET_IDENTITIES="$WORK/market/identities"
export LEZ_PROVISION="$PROVISION" LEZ_WORKSPACE="$WORKSPACE" LEZ_TIMING_PROFILE=fast SKIP_UI_VERIFY=1
export LEZ_MONERO=1 LEZ_MONERO_NETWORK=stagenet LEZ_XMR_STAGENET_LOGINS="$WORK/logins"
export LEZ_XMR_STAGENET_NETWORK=lez-m4-stagenet-net
export LEZ_XMR_STAGENET_MONEROD=lez-m4-stagenet-monerod:38081
export LEZ_XMR_STAGENET_MAKER_WALLET=lez-m4-stagenet-wallet-maker:38083
export LEZ_XMR_STAGENET_TAKER_WALLET=lez-m4-stagenet-wallet-taker:38083
export LEZ_IMAGE_PREFIX=lez-m4-stagenet LEZ_IMAGE_TAG=local
bash scripts/up.sh --fresh-lez
```

On route 2 the two daemons are the forwarders instead
(`LEZ_XMR_STAGENET_MAKER_MONEROD=lez-m4-stagenet-remote-node:38089`,
`LEZ_XMR_STAGENET_TAKER_MONEROD=lez-m4-stagenet-remote-node-taker:38089`) and
the Maker's wallet-rpc is route 2's (`lez-m4-stagenet-wallet-rpc:38083`).
`$WORK/market` is a market root of the stack's own, holding a copy of the
first stack's `identities/` (a market root belongs to one stack, and
`gen-config.sh` refuses another's); `$PROVISION` and `$WORKSPACE` are the
devnet's provision directory and workspace. Both Nodes then read `profile: public-stagenet` and the windows 600, 7200 and 14400 s
in their `xmr-role.json` (600, 7200 and 9000 s in the first run, under the
earlier draft), and `maker_health` shows the Monero route
available once its probe reads the daemon's `get_info`.

### A swap

The Maker's CLI enables the route and publishes an offer, the Taker takes it
over its owner RPC, exactly as on regtest (`deploy/scripts/node-e2e.py`,
`xmr_publish` and `take`): the lot of the run was 0.01 XMR for 50,000 LEZ
units, set with `lez-maker-cli configure-pair --pair monero --direction
taker-sells-lez --minimum-foreign-units 10000000000 --maximum-foreign-units
10000000000` and `set-local-price --lez-units-per-lot 1
--foreign-units-per-lot 200000`. Then, by itself:

1. The Nodes agree Stage A and Stage B over Chat (six rounds) and the Taker
   locks its LEZ (tag 13): about two minutes.
2. The Maker's drive that sees the lock final sends the funding and logs
   `Maker Monero funding: sent … in <txid>; the network's miners confirm it`.
3. Stagenet confirms it; the Taker's view-only wallet finds it and the
   Taker's view reads `claim_available` at ten confirmations; the Maker's
   observer records the funding (`both_legs_locked`).
4. The Taker's owner claims (`taker_swap_claim_v1`): the Node authorises the
   Maker's claim (tag 14); the Maker claims (tag 15); the Taker rebuilds the
   key from the claim signature and sweeps the Monero; the sweep's ten
   confirmations end the swap `completed`.

A swap the Taker does not claim is refunded from R: by the Taker Node
itself, or, with its own refund off (`xmr-automatic-refund`), by the owner's
`taker_swap_refund_v1`; the Maker rebuilds the key from the refund signature
and sweeps its Monero back. A swap the Taker neither claims nor refunds
before P is punished: the Maker takes the LEZ and the Monero stays at the
shared address.

## A daemon the Node does not run

A Node should read a monerod its operator runs, at the tip. A public node,
or anyone between the Node and it, can:

- **Lie about the chain.** It can serve a branch of its own: on Stagenet's
  difficulty (about 2.5 million on 2026-11-21) a few desktops mine eleven
  blocks in hours. The view-only wallet checks that an output pays the
  shared view key; nothing the Node does checks that the blocks are the
  heaviest chain. A Taker that authorises on such an output loses the
  Monero side once the Maker claims (TM-0033).
- **Lag or stall.** A daemon behind the network makes the Maker fund late
  or the Taker authorise late; the swap then falls back to the refund. Each
  Node refuses a daemon that is not synchronized, is below its target
  height, or whose tip is older (or further ahead) than an hour by the
  host's clock: the Maker for its funding receipt and its sweep's
  observation, the Taker before it records the Maker's funding, before tag 14
  and before its claim sweep.
- **Withhold.** It sees the transfers and outputs the wallet asks about and
  the Node's IP address, and can drop them.
- **Answer badly.** `monero-rpc` 0.5.1 has no response-body limit and
  discards the untrusted flag on single-header reads (`TRANSPORT_RESIDUAL`,
  `HEADER_TRUST_RESIDUAL`): the loopback-only rule assumed a process of the
  operator's own behind the endpoint, which a forwarder to a public node is
  not.

What every role verifies itself, whatever the daemon: the Stagenet genesis
and the daemon's own `get_info` network, at take time and again before each
decision; the exact transaction, the shared
address and the amount, by the view key; zero unlock distance (a funding
with an unlock time is refused); ten confirmations and one stable tip across
the daemon's and the wallet's reads; that its own sweep destination is a
Stagenet address. The Taker reads the recorded funding again, at its depth
and unlocked on the current chain, right before tag 14 is authorised and
before each resubmission, so a funding reorganised out or double spent
before the claim stops it. Ten confirmations is an agreement constant, not
sized to any value: a reorganisation deeper than ten blocks after the
Taker's authorisation, or a double spend of the funding on a heavier branch, takes
the Monero side of a completed swap (TM-0034). The Monero transfers pay the
wallet-rpc's default fee, and a new output is locked for ten blocks: the
Maker's change and the Taker's swept Monero are spendable twenty minutes or
so after they confirm (TM-0035).

## The Node-driven run of 2026-11-26, on the self-hosted node

On the same Mac, both Nodes built from `f259e791` without the devnet test
hooks (`build-source.txt` in both images; no `LEZ_TEST_HOOKS` or
`test_hooks` string in their binaries), on a fresh LEZ chain, under the
windows C, R, P = +10 min, +2 h, +4 h, with the Taker Node's own refund on.
Both Nodes and both wallet-rpcs read the operator's own `monerod`, the one of
[route 1](#route-1-a-self-hosted-monerod), synchronized at the Stagenet tip
(its top block hash the same as three public nodes' at the start and at the
end of the run); no public node, and no forwarder to one, was configured.
Each swap moved 0.003 XMR for 50,000 LEZ units. Times are UTC; Monero times
are block timestamps. The
[evidence set](evidence/stagenet-0.4.0-2026-11-26/) holds the per-swap table,
the node's own log of the swaps' transactions and of the callers it served,
the Nodes' daemon addresses as Docker ran them, and the Maker Node's log.

The names were `lez-stagenet040-*` rather than the guide's
`lez-m4-stagenet-*`, which an earlier run's volumes on the host still held.
The Maker's role wallet was topped up with 0.1 XMR from the XMR-TW faucet
before the run (the funding wallet of 2026-11-21 could not be opened: its
password file was lost), and the swaps were taken one after another, each
once the Maker had an unlocked output for it.

- **A, completed** (`b5fc18b7…`, taken 01:55:27): the Taker's LEZ lock final
  at 01:55:33; the Maker's funding
  `3ff40f7986894e068304c38eed5447ec984afc1599b7fcbfb1a91743d918ab2d`
  (block 2,258,583, 01:58:36), which the node logged entering its pool at
  01:56:31; `claim_available` at 02:08:59, the owner's claim at 02:09:02;
  tag 14 final at 02:09:43, the Maker's tag 15 at 02:10:42; the Taker's
  sweep `5b182e8c75c68fc937db3f99677c795a6c7f6bc9ea16a8a2dc3d8a88874306bf`
  (block 2,258,594, 02:13:19, 0.00296962 XMR); both Nodes `completed` at
  02:29:39, 34 minutes after the take.
- **B, refunded by the Taker Node itself** (`3d4c2915…`, taken 02:00:36, R
  04:00:26): funded in
  `a722e1e43be96cd83888fbbbe4d2c3107d4349ca9e3b1d2ed401688bc215fe13`
  (block 2,258,588, 02:03:51), not claimed. With no owner's command the
  Taker Node's tag 16 was final at 04:00:32, 6 seconds after R; the Maker
  swept its Monero back in
  `7dcb913175f2e3f6c3c99446f11581f2dba0ac831a5dbbde4b772057de2f0f0a`
  (block 2,258,649, 04:09:30, 0.00296956 XMR) and read `refunded` at
  04:21:23, the sweep at ten confirmations.
- **C, punished** (`9d17b588…`, taken 02:20:26, R 04:20:17, P 06:20:17):
  funded in `2bc4938409ff547c3d210465a63ab3cd03c81e10b1462f2a56e45f1b217991b7`
  (block 2,258,599, 02:22:11). The Taker Node was stopped from 04:10:10 to
  06:26:16; the Maker's tag 17 was final at 06:20:32, 15 seconds after P,
  and the Taker Node read `punished` after its restart. The 0.003 XMR stays at
  the shared address, which neither role can spend.

What the node shows of them: each funding and sweep reached it from the
sending role's wallet-rpc (`POST /sendrawtransaction` from that wallet-rpc's
address on the Docker network, then `Transaction added to pool: txid <…>` in
its log), each was mined in a block it had verified, at the height it gives
for it, and each Node's launcher forwarded the Node's loopback Monero ports to
it and to the role's wallet-rpc; no other daemon was configured anywhere in
the stack. Its top block hash matched three public nodes' at the start
(height 2,258,580) and at the end of the run (height 2,258,725).

## The Node-driven run of 2026-11-22

On the same Mac and stack as [the first run](#the-node-driven-run-of-2026-11-21),
on a fresh LEZ chain, both Nodes built from `f059d3fa` (whose shipped code
is that of `7b4e46da`, the commit of the
[full regression](evidence/regression-0.4.0-2026-11-22-macos-arm64/))
without the devnet test hooks, under the windows C, R, P = +10 min, +2 h,
+4 h (`xmr-role.json`: 600, 7200 and 14400 s), with the Taker Node's own
refund on. The Maker Node read `node2.monerodevs.org`, the Taker Node
`node.monerodevs.org`, each behind its forwarder; both wallet-rpcs read
`stagenet.xmr-tw.org`. Each swap moved 0.003 XMR for 50,000 LEZ units. The
[evidence set](evidence/stagenet-0.4.0-2026-11-22/) holds the per-swap table
with every transaction, the Nodes' log lines and the Taker's records.

- **A, completed** (`13711985…`, taken 10:14:07): the Maker's funding
  `007ccb716a347a830f0ae6d59225aa3d1ea97efc06fb83462115d80d92082f74`
  (block 2,255,819, 10:20:09); `claim_available` at 10:43:03, the owner's
  claim at 10:43:12; tag 14 final at 10:44:11, the Maker's tag 15 at
  10:44:28; the Taker's sweep
  `f2e6b550fb950c3e05885c94040b1d8ece00602997d2052ac1169c30fcfca87f`
  (block 2,255,838, 10:55:00, 0.0029695 XMR); both Nodes `completed` at
  11:14:22, 60 minutes after the take.
- **B, refunded by the Taker Node itself** (`fc596d6b…`, taken 10:13:44, R
  12:13:44): funded in
  `99acf8fd385da05230851ea41b4d7e489454ad2e0a7b3d5e9f216093713e1ff6`
  (block 2,255,818), not claimed. With no owner's command the Taker Node's
  tag 16 was final at 12:15:25, 101 seconds after R; the Maker swept its
  Monero back in
  `5b1ea9674331c03e22a9a3478de2ab632eab53c5b9b389aa992fa06cbb9980a3`
  (block 2,255,869, 12:22:09, 0.00296954 XMR) and read `refunded` at
  12:50:47.
- **C, never funded, refunded** (`5a696c9f…`, taken 10:28:50, R 12:28:50):
  the Maker's three funding attempts were answered `not enough unlocked
  money` (A's and B's fundings had taken its outputs, and their change was
  still locked), so it ended `expired`. The Taker Node refunded its LEZ by
  itself: tag 16 final at 12:28:57. This is the unfunded case of TM-0031,
  with the Taker Node running.
- **C2, punished** (`353a4758…`, taken 10:44:34, R 12:44:34, P 14:44:34):
  funded in `3779bd5b2e9b06ba4569adea461678d2fae95cee1b06afb1e3f5ade3d2fe87d5`
  (block 2,255,837). The Taker Node was stopped from 12:33:01 to 14:50:08;
  the Maker's tag 17 was final at 14:44:57, 23 seconds after P, and the
  Taker Node read `punished` after its restart. The 0.003 XMR stays at the
  shared address, which neither role can spend.

What the fixes made after the first run did here: every refund went out in
the Taker Node's first sweep past R; eighteen of its Monero steps (C's
observation of a funding that never came, C2's and A's) ended at their
two-minute budget, as view-only wallets restored through public nodes
refresh for longer, and the next step read what the wallet-rpc had
refreshed meanwhile; A's tag 14 followed a fresh read of its funding; no
step failed the daemon's sync or tip-age check; C2's funding observer timed
out once on the Maker and the drive went on. An authorised claim past P did
not occur.

## The Node-driven run of 2026-11-21

The first run, under an earlier draft of the windows. On an Apple-silicon Mac with Docker Desktop: a second devnet stack
(`lez-m4-stagenet-dev`, [the stack](#the-stack)) with LEZ on its own fresh
devnet chain, both Nodes built from `8f66d11b`
(`/usr/local/share/lez/build-source.txt` in both images), Monero on public
Stagenet: the Maker's daemon `node2.monerodevs.org`, the Taker's
`node.monerodevs.org`, each behind its forwarder, the Maker's wallet-rpc on
`stagenet.xmr-tw.org`, the Taker's on `node.sethforprivacy.com`. Times UTC;
Monero times are block timestamps. The windows were the earlier draft's, C,
R, P = +10 min, +2 h, +2.5 h: the run shows the flow of each ending, and the
punishment came 30 minutes after R where the profile now waits two hours.
The fixes made after this run (the refund first in every sweep, the
funding read again before tag 14, the daemon checked before each Taker
decision, the Maker's watch for a refund while its funding is unrecorded, an
authorised claim ending `punished` only on tag 17) ran on Stagenet in
[the run of 2026-11-22](#the-node-driven-run-of-2026-11-22).

**Swap D, completed** (`d9353027…`, 0.01 XMR for 50,000 LEZ units):

| | When | What |
|---|---|---|
| take | 22:50:31 | Stage A and Stage B over Chat; LEZ anchor 22:50:22, so C, R, P = 23:00:22, 00:50:22, 01:20:22 |
| Taker's LEZ lock | 22:50:46 final | tag 13 `24708a40…` and its funding `d75a6222…` |
| Maker's funding | 22:53:10, block 2,255,487 | `807a7d6c551c04c64508572976013900629ccda9ad2e562a6b2c32a2d8bfb74c` |
| ten confirmations | 23:11 | the Taker's view reads `claim_available` |
| Taker's claim | 23:11:21 | the owner's `taker_swap_claim_v1`; tag 14 `651c196d…` final 23:12:48 |
| Maker's claim | 23:23:27 final | tag 15 `ec19a8cc…`; the Maker's drives had recorded the funding at ten confirmations about 23:12 |
| Taker's sweep | 23:28:57, block 2,255,511 | `e27ca30796352973c9ab84a2b7400e198d1bf2151b5a044fe8ffa3a85201a68d`, 0.00996954 XMR to the Taker's wallet (fee 0.00003046) |
| completed | 23:45 | both Nodes `completed`, the sweep at ten confirmations |

From the take to the Monero in the Taker's wallet at ten confirmations: 55
minutes, of which 20 minutes were the funding's confirmations and 17 the
sweep's. The Maker claimed 11 minutes after the Taker's authorisation: each
of its drives scanned 128 LEZ blocks, and the devnet makes one every two
seconds or so; since `5e9c07e9` a Maker on Stagenet scans 1,024 a drive.

**Swap E2, refunded** (`8eb3c3f6…`, 0.004 XMR for 50,000 LEZ units, taken
23:04:25, R 01:04:15, P 01:34:15): the Maker funded in
`a0240b756fb0a3354ba019a53e3abb0bae1e83f2826192e3ad532bd6e265eaef` (block
2,255,496, 23:09:26); the Taker did not claim. With the Taker Node's own
refund switched off (`xmr-automatic-refund` false), its owner refunded at
01:05:55 (`taker_swap_refund_v1`); tag 16 `38e6a2dc…` was final at 01:05:57.
The Maker found the refund at 01:11, rebuilt the key from the refund
signature and swept its Monero back in
`4eb1244da6e50d3ecacc0171de98f1fc65d09341464bfec4d1bf6f020dc3da4e` (block
2,255,564, 01:18:18), 0.00396964 XMR to its wallet (fee 0.00003036);
the Maker Node read `refunded` at 01:49, the sweep at ten confirmations,
and the Taker Node `refunded`.

**Swap F, punished** (`682c0484…`, 0.004 XMR, taken 23:04:38, R 01:04:27,
P 01:34:27): the Maker funded in
`d3a757175f7216bc4913292d8ee75cae1dea41cbdd99c610b2d682e734ff0e88` (block
2,255,500, 23:14:40); the Taker neither claimed nor refunded. From P the
Maker found no refund to the finalized tip, sent tag 17 `a3651d69…` (final
01:35:02, 35 seconds after P) and took the LEZ; both Nodes read `punished`.
Its 0.004 XMR stays at the swap's shared address, which neither role can
spend.

**What did not work, and what was changed for it.** The first three swaps
(`6554b020…`, `31ced2ec…`, `2d520725…`, from 21:39) ran on images of
`45bb0e7d`: they agreed, locked and funded on Stagenet, and the Taker saw
their outputs at ten confirmations, but the Maker's observer failed for half
an hour on reads through a plain forwarder (the public nodes' connection
limit, [above](#each-roles-daemon)) and, once it read, its branch
activation still required Monero Regtest; `8f66d11b` removed that
requirement. A Maker Node pins each swap's actor program, so those three
could not move to the new build: their Taker refunded its LEZ by the
owner's refund from R, and their Maker's 0.03 XMR stays at their shared
addresses (the Taker's share is revealed in each tag 16, so it can be swept
by hand; that was not done). A fourth swap (`f365832e…`) found the Maker's
wallet without an unlocked output (the change of the funding before it was
still locked) and ended `expired`, nothing funded. During the run the shared
Docker disk of the host ran full once, and Bitcoin Core and the Maker Node
of the stack restarted; the swaps resumed.

## The Monero leg alone: the SDK example

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
LEZ_XMR_EXAMPLE_SEED="$(openssl rand -hex 32)" \
  cargo run --locked -p xmr-reference-actor --example full-lifecycle
```

On the self-hosted route, once it is at the tip, point `LEZ_XMR_MONEROD_URL`
at `http://127.0.0.1:38081/` with its login and `LEZ_XMR_FUNDING_WALLET_URL`
at the wallet-rpc of route 1. The keys come from the seeded stream named by
`LEZ_XMR_EXAMPLE_SEED`: anyone who knows the seed can rebuild the shared
spend key and sweep the output first, so on Stagenet the example refuses to
run without a seed of your own of at least 32 characters (regtest keeps the
default `full-lifecycle`). The Stagenet run moves 0.02 and 0.01 XMR,
a tenth of the regtest amounts, and needs a little more than 0.03 XMR
unlocked.

What to expect, in order (the run's own lines are in
[its run](#the-sdk-examples-run-of-2026-11-21)):

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
  RPC, and reads the real wallet and daemon for a Stagenet one;
- the Stagenet topology check, which a Node's funding receipt needs, refuses
  the daemon while it syncs (`DaemonNotSynchronized`); a daemon at the tip
  passes it (`MONERO_STAGENET_EXPECT_SYNCED=1`).

Then the daemon must reach 1,000 blocks of the public Stagenet chain within
15 minutes (`MONERO_STAGENET_SYNC_HEIGHT`, `MONERO_STAGENET_SYNC_SECONDS`).
The job does not sync to the tip, holds no coins and runs no swap; it
depends on public Stagenet peers answering. Run locally in an arm64 Linux
container on a Mac (the arm64 release) on 2026-11-21, it passed: both tests,
then 3,101 blocks from three peers in 47 s (before the third test was added).

## The SDK example's run of 2026-11-21

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
   reason given), and the example stopped. In the example the refund path,
   the Maker rebuilding the key from the Taker's refund signature and
   sweeping its Monero back, has run on regtest only (D-2 of the
   [M4 checklist](m4-acceptance-checklist.md)); through the Nodes it ran on
   Stagenet in swap E2 of [the first run](#the-node-driven-run-of-2026-11-21) and swap B
   of [the run of 2026-11-22](#the-node-driven-run-of-2026-11-22).

The self-hosted node had synced 137,850 of 2,255,265 blocks when the run
ended.

## Stopping and removing

```sh
(cd "$REPO/deploy" && bash scripts/down.sh --wipe)    # the second stack, from its own deploy directory
docker rm -f lez-m4-stagenet-monerod lez-m4-stagenet-wallet-rpc lez-m4-stagenet-wallet-self \
  lez-m4-stagenet-wallet-taker lez-m4-stagenet-remote-node lez-m4-stagenet-remote-node-taker
docker network rm lez-m4-stagenet-net
docker volume rm lez-m4-stagenet lez-m4-stagenet-wallet lez-m4-stagenet-wallet-self lez-m4-stagenet-wallet-taker
docker image rm lez-m4-stagenet-monero:0.18.5.1
```

These commands were not run in the session, apart from removing the
self-hosted node's container and chain volume. The wallet volumes hold
Stagenet keys; delete them with the volumes.

## Known limits

- A Node checks its daemon's network, sync and tip age and each output by
  its view key, not that the daemon's chain is the heaviest (TM-0033). The
  run of 2026-11-26 read the operator's own synchronized `monerod`, whose
  peers were public Stagenet nodes; the runs of 2026-11-21 and 22 read public
  nodes as their daemons, through forwarders. One `monerod` served both
  roles in the run of 2026-11-26; two parties run two.
- Ten confirmations is a constant of the agreement: a reorganisation deeper
  than that after the Taker's authorisation takes the Monero side (TM-0034).
- P − R is two hours on Stagenet: a Taker Node down for longer from R loses
  its LEZ (TM-0031), as swap C2 of the run of 2026-11-22 and swap C of the run
  of 2026-11-26 show.
- The role wallets have no password; the Maker's needs one unlocked output
  per swap in flight; fees are the wallet-rpc's default (TM-0035).
- A Maker Node pins each swap's actor program: a swap taken under one build
  does not finish under another.
- The LEZ side of the Node-driven run is the devnet's escrow, not LEZ
  testnet; the SDK example's LEZ escrow is modelled in memory.
- `VIEW_ONLY_SPEND_RESIDUAL`: a view-only wallet cannot prove that an output
  is still unspent; the swap relies on a fresh shared address per swap.
- CI's Stagenet job depends on public peers and syncs a bounded prefix of the
  chain, not the tip; the node that reached the tip ran on an operator's
  host, not in CI.
- 0.18.5.1 is the pinned release; Stagenet's next hard fork would need the
  release that follows it.
