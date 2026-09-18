# Running the swap stack on public testnets

This guide runs the Maker and Taker Nodes, and both Basecamp desks, against
the **official LEZ v0.2 testnet** (`https://testnet.lez.logos.co`, channel
`0101…01`, LEZ v0.2.4) and **Bitcoin testnet4**. Nothing here is a local
chain: four local services join the public networks, and the Nodes reach
them over a Docker network.

| Service | What it is | Why it runs locally |
|---|---|---|
| `lez-btc-testnet4` | Bitcoin Core 31.1 on testnet4, unpruned, `txindex` + `txospenderindex` | the Nodes fund their locks from a wallet per role. Observation alone needs only `txindex` on any Core from 24.0: without `txospenderindex` the adapter finds a spender from the mempool, the UTXO set and a scan back from the tip |
| `lez-testnet-node` | Logos Blockchain node on the public testnet | feeds the indexer |
| `lez-testnet-indexer` | LEZ v0.2.4 indexer following the public channel | the public endpoint serves no indexer (`getLastFinalizedBlockId`) |
| `lez-testnet-sequencer` | nginx: plain HTTP on the Docker network to `https://testnet.lez.logos.co` | the Nodes accept only literal-loopback HTTP endpoints |

Host requirements: arm64 with Docker, about 40 GB for testnet4 and room for
the Logos Blockchain node's state.

## Which networks

The networks are configuration, nothing else: `LEZ_BTC_NETWORK` (`bitcoin.network` in
the role's `btc-role.json`) is `regtest`, `testnet4`, `testnet3`, `signet` or `mainnet`,
and `LEZ_LEZ_NETWORK` (`lez.network`) is `devnet`, `testnet` or `mainnet`. This guide
sets `testnet4` and `testnet`; `testnet3` exists because it is what keyless public RPC
providers serve.

A Node refuses a configuration that could pair real money with a test chain:

- `mainnet` settles only against `mainnet` — either side being mainnet while the other is
  not fails at load, in both directions. A configuration written before `lez.network`
  existed means `devnet`, so it can never be read as mainnet.
- `bitcoin.genesis_block_hash` must be the genesis of the network that is named, so a
  name cannot be a label over another network's chain; the observation layer checks the
  node's own `chain` and genesis again at run time, and keeps `testnet3` and `testnet4`
  apart the same way.

## 1. Shared network

```sh
docker network create lez-testnet
mkdir -p ~/lez-testnet
```

## 2. Bitcoin testnet4

```sh
D=~/lez-testnet; umask 077
printf 'BTC_TESTNET4_RPC_USER=lezrpc\nBTC_TESTNET4_RPC_PASSWORD=%s\n' "$(openssl rand -hex 24)" > $D/btc-testnet4-rpc.env
. $D/btc-testnet4-rpc.env
cat > $D/bitcoin-testnet4.conf <<CONF
chain=testnet4
server=1
txindex=1
txospenderindex=1
prune=0
dbcache=2048
[testnet4]
rpcbind=0.0.0.0
rpcport=18443
rpcallowip=192.168.16.0/20
rpcuser=$BTC_TESTNET4_RPC_USER
rpcpassword=$BTC_TESTNET4_RPC_PASSWORD
CONF
chmod 644 $D/bitcoin-testnet4.conf; mkdir -p $D/bitcoin-testnet4; chmod 777 $D/bitcoin-testnet4
docker run -d --name lez-btc-testnet4 --restart unless-stopped --network lez-testnet \
  -p 127.0.0.1:48332:18443 -v $D/bitcoin-testnet4:/var/lib/bitcoin -v $D:/run-config-dir:ro \
  lez-bitcoin-core:local -conf=/run-config-dir/bitcoin-testnet4.conf -datadir=/var/lib/bitcoin -printtoconsole
```

The RPC is reachable only from the `lez-testnet` Docker network and from this
host: `rpcallowip` admits that network alone (use the subnet
`docker network inspect lez-testnet` reports), and the container publishes its
port to `127.0.0.1` only. `rpcbind` is broad inside the container's own network
namespace, which has no other route in.

The initial sync took under an hour on Apple silicon. Stop the node with
`docker stop -t 300 lez-btc-testnet4`; a forced removal loses the unflushed
cache and the node re-syncs tens of thousands of blocks.

Create one wallet per role (`lez-maker`, `lez-taker`) with `createwallet` and
fund them from a testnet4 faucet. CypherFaucet publishes a keyless, captcha-free
API intended for tooling, which pays 0.01 tBTC (1,000,000 sats) per claim and
accepts taproot addresses:

```sh
curl -X POST https://cypherfaucet.com/api/v1/claim \
  -H 'content-type: application/json' \
  -d '{"network":"btc-testnet","address":"<tb1… from getnewaddress>"}'
```

Claims are limited to one per address and one per source IP per hour, so honour
`Retry-After` on 429 instead of retrying in a loop; `GET /api/v1/info` reports the
faucet's balance. One claim funds both directions: the two wallets share a node, so
`sendtoaddress` moves a share to the other role once the claim confirms. About
30,000 sats per wallet covers a swap in each direction, and the Nodes require one
confirmation before they spend an output, so a claim is not usable while it sits in
the mempool. Every other public testnet4 faucet found is gated by a captcha or a
login, and signet's faucets are gated more heavily still. Mining is not a route to
spendable coins: a coinbase output needs 100 confirmations, about 33 hours at
testnet4's block rate, and minimum-difficulty blocks are in any case taken the
moment they become valid.

## 3. The public Logos Blockchain node

Use the `testnet` image; the `0.2.4` release image speaks a different chain-sync
protocol (`dst-0.2.4`) and the bootstrap peers refuse it.

```sh
N=~/lez-testnet/logos-node; I=ghcr.io/logos-blockchain/logos-blockchain:testnet
mkdir -p $N/state; chmod 777 $N $N/state
docker run --rm --user 65532:65532 -e HOME=/tmp -v $N:/cfg --entrypoint /usr/bin/logos-blockchain-node $I \
  init-config -o /cfg/user_config.yaml -p \
  /ip4/65.109.51.37/udp/3000/quic-v1/p2p/12D3KooWFrouXfmrR4nsLMtE7wu15DoMJ6VtoUtHinREZCvbWHar \
  /ip4/65.109.51.37/udp/3001/quic-v1/p2p/12D3KooWJRGau8M1rjT7R5e4YYsgdFhsMX35nRDtMwCDjxQkXAHz \
  /ip4/65.109.51.37/udp/3002/quic-v1/p2p/12D3KooWQXJavMDTRscjauFSgVAB1VLB6Rzpy2uY5SU9Tk7927tb \
  /ip4/65.109.51.37/udp/50001/quic-v1/p2p/12D3KooWSQc7CcGtvWDPF1yCbBthFnQjprfCVHmfmNDUrSmqQsU1
sed -i.bak 's/listen_address: 127.0.0.1:8080/listen_address: 0.0.0.0:8080/' $N/user_config.yaml
docker run -d --name lez-testnet-node --restart unless-stopped --network lez-testnet --user 65532:65532 \
  -e HOME=/tmp -w /cfg -v $N:/cfg -p 127.0.0.1:28080:8080 -p 3000:3000/udp \
  --entrypoint /usr/bin/logos-blockchain-node $I /cfg/user_config.yaml
curl -s http://127.0.0.1:28080/cryptarchia/info
```

The bootstrap peers are those in the Logos Blockchain Node 0.2.4 release notes.

## 4. The sequencer proxy

```sh
mkdir -p ~/lez-testnet/proxy
# nginx.conf: listen 3040; proxy_pass https://testnet.lez.logos.co with
# proxy_set_header Host testnet.lez.logos.co, proxy_ssl_server_name on,
# proxy_ssl_verify on, proxy_ssl_verify_depth 4 (the chain has four certificates),
# proxy_ssl_trusted_certificate /etc/ssl/cert.pem.
docker run -d --name lez-testnet-sequencer --restart unless-stopped --network lez-testnet \
  -p 127.0.0.1:23040:3040 -v ~/lez-testnet/proxy:/etc/nginx/lez:ro \
  nginx:1.29.1-alpine nginx -c /etc/nginx/lez/nginx.conf -g 'daemon off;'
```

Mount the configuration directory, not the file, so an edit survives a restart.

## 5. The indexer

Build `indexer_service` from LEZ v0.2.4 (`47eba25`; its testnet genesis is
enabled by default) and run it against the local node:

```sh
cat > ~/lez-testnet/indexer/indexer_config.json <<JSON
{
  "consensus_info_polling_interval": "1s",
  "bedrock_config": { "addr": "http://lez-testnet-node:8080" },
  "channel_id": "0101010101010101010101010101010101010101010101010101010101010101",
  "allow_chain_reset": true
}
JSON
docker run -d --name lez-testnet-indexer --restart unless-stopped --network lez-testnet \
  -p 127.0.0.1:28779:8779 --user 65532:65532 -e HOME=/tmp -v <dir with indexer_service>:/opt/lez:ro \
  -v ~/lez-testnet/indexer:/cfg --entrypoint /opt/lez/indexer_service lez-services:local \
  /cfg/indexer_config.json --port 8779 --data-dir /cfg/state
```

The indexer serves no finalized block until the node leaves its prolonged
bootstrap period.

## 6. LEZ accounts and funds

The Nodes' identity keys (`lez-v02-local-actor-identity`) work unchanged on
the public network. Fund each role's owner account with the LEZ v0.2.4 wallet
CLI (it links `libpcsclite`):

```sh
export LEE_WALLET_HOME_DIR=~/lez-testnet/wallet-home
wallet change-network testnet
wallet account import public --private-key "$(cat <identity>/lez-signer.key)"
wallet auth-transfer init --account-id Public/<account id>
wallet pinata claim --to Public/<account id>      # 150 LEZ per claim, no captcha
```

`All pollers failed` from the wallet is a poll timeout; confirm with the
sequencer's `getAccount` or the explorer
(`https://explorer.testnet.lez.logos.co/account/<id>`).

## 7. The escrow program

The official sequencer admits at most **614,200 bytes** per transaction, and a
program deployment carries the whole risc0 program binary. The escrow guest
built with default settings is 685,524 bytes: 198 KB of it are symbol and
string tables the zkVM never loads. The guest crate therefore strips symbols
in its release profile (`escrow/methods/guest/Cargo.toml`), which brings the
binary to 487,244 bytes. Stripping changes the ImageID, so the escrow program id
is the stripped guest's (`c22d61fc…`).

Deploy through the sequencer proxy's network namespace, where
`http://127.0.0.1:3040/` is the official sequencer, which is the only kind of
endpoint the deployer accepts:

```sh
docker run --rm --network container:lez-testnet-sequencer \
  -v <dir with lez-zec-escrow-v02-deployer>:/deployer:ro -v ~/lez-testnet/market/bootstrap:/out \
  lez-builder:local bash -c '/deployer/lez-zec-escrow-v02-deployer deploy-m4-local \
    --rpc-url http://127.0.0.1:3040/ \
    --channel-id 0101010101010101010101010101010101010101010101010101010101010101 \
    --timeout-seconds 900 > /out/deployment.json'
```

Its preflight checks the channel and the live builtin program ids
(`authenticated_transfer` `fe96c422…`, `token` `ccc4713e…`, and the
associated-token-account ImageID `9df1315d…`, which `getProgramIds` omits)
before it submits anything.

## 8. Running the swaps on the desks

Both directions run from `deploy/`, against the public stack rather than the local
defaults:

```sh
export LEZ_STACK_COMPOSE_ARGS="-p lez-testnet --env-file testnet.env -f compose.yaml -f compose.testnet.yaml"
export LEZ_CONTAINER_PREFIX=lez-testnet LEZ_REPAIR_INDEXER=0 LEZ_EXPORT_EVIDENCE=0
export LEZ_UI_LEZ_AMOUNT=100 LEZ_UI_BTC_AMOUNT=0.0001
export INTERACTIVE_TIMEOUT_MS=5400000
scripts/ui-e2e.sh happy --direction TakerSellsForeign --record   # the Taker pays BTC
scripts/ui-e2e.sh happy --direction TakerSellsLez --record       # the Maker pays BTC
```

Four things differ from a local run, and each of them will fail a run if missed.

**Trade size.** Keep it small enough for the paying wallet: the local defaults are
1,000 LEZ for 0.01 BTC, and reserving 1,000,000 sat needs a wallet that holds it.
`LEZ_UI_BTC_AMOUNT=0.0001` is 10,000 sat. The Bitcoin payer differs per direction, so
fund both roles: `TakerSellsForeign` spends `lez-taker`, `TakerSellsLez` spends
`lez-maker`.

**The lock fee.** A Bitcoin lock can never be fee-bumped (see below), so the Node
decides its fee once, when the swap is taken: `estimatesmartfee` for a 6-block target,
20 sat/vB when the node has no estimate (runs of empty blocks leave testnet4's estimator
without one), never above 25 sat/vB. It then refuses the take outright if the fee would
exceed 5% of the amount locked, rather than overpay -- left to Core's wallet alone, two
10,000 sat locks in the first run paid 56,064 sat each. A 10,000 sat trade cannot meet
5% at any usable rate, so `testnet.env` raises that one bound to 40%; size real trades so
the default holds. The knobs are `LEZ_BTC_LOCK_FEE_CONFIRMATION_TARGET`,
`LEZ_BTC_LOCK_FEE_FALLBACK_SAT_PER_VB`, `LEZ_BTC_LOCK_FEE_MAX_SAT_PER_VB` and
`LEZ_BTC_LOCK_FEE_MAX_PERCENT` (`bitcoin.lock_fee` in the role's `btc-role.json`), and
each plan records what it paid as `fee_sat` in `bitcoin/funding-plan.json`. Claims and
refunds pay the fixed `claim_fee_sat` (1,000 sat) the agreement signs.

**The wait timeout.** The desks wait `INTERACTIVE_TIMEOUT_MS` (default 1,800,000, so
30 minutes) for the Node to reach the next state, and the swap cannot advance until the
Taker's Bitcoin lock has a confirmation. testnet4 blocks average 20 minutes, so the
default is barely 1.5 block intervals and it does time out. 90 minutes survives a run of
empty blocks.

**Stale offers.** The publish step publishes "until two are pending" and offers live for
`offer_ttl_seconds` (3600). So after changing the trade size, two old offers suppress the
corrected ones and the run takes a stale one instead — the tell is that the "New offer:"
narration lines are missing. Either wait for them to expire or run the other direction
first: `offer-sell-btc-*` (TakerSellsLez) and `offer-sell-lez-*` (TakerSellsForeign) do
not block each other.

**Empty blocks, and why waiting is the only remedy.** Runs of testnet4 blocks carry
nothing but their coinbase, so a lock can sit unconfirmed for tens of minutes at any fee
rate. Do not try to mine your way out: the minimum-difficulty exception applies only to a
block whose timestamp is more than 20 minutes after its parent's, and parent timestamps
here run ~2 h ahead of wall clock while consensus caps a timestamp at `now + 7200`, so a
candidate block gets the real retarget difficulty instead. `getblocktemplate` reports that
honestly (1.3e9 × difficulty 1 when measured); `getmininginfo`'s `difficulty 1` describes
the tip, not your candidate. Never fee-bump either: the Bitcoin lock is a protocol
transaction the Node signed at take time, and replacing it invalidates the signed path.

**Resuming after a desk timeout.** A timeout fails the recording, not the swap: the Maker
Node funds the LEZ escrow on its own once the Taker's lock confirms. Re-running the
scenario would create a *new* swap and strand the funded lock, so drive the remaining
steps against the existing one instead, passing `INTERACTIVE_ACTION`,
`INTERACTIVE_STATE`, `INTERACTIVE_SWAP_ID` (and `INTERACTIVE_EXPECT_LABEL` for a Maker
wait) into the same `/ui-tests/record-step.sh` the runner uses.
