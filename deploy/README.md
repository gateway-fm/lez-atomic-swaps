# lez-swap-stack — fully dockerized local LEZ ↔ BTC / ZEC swap environment

One compose project: the LEZ v0.2 devnet (one Basecamp hosting the LEZ node and
the indexer as Logos modules, plus the sequencer) + Bitcoin Core regtest +
Zebra regtest + each party's own Basecamp, holding that party's desk, its LEZ
signer and its canonical Node (drivable over VNC and by automated end-to-end
tests). Block explorers for both chains are an optional profile. All native to
the host, started with one command.

Seven containers run by default: `bitcoin-core`, `btc-miner`, `zebra`,
`network`, `sequencer`, `maker-node` and `taker-node`. No init container runs.
`basecamp-ui` is built but never started; `btc-explorer` and `lez-explorer`
(profile `explorers`), `heartbeat` (profile `heartbeat`) and `tools` (profile
`tools`, one-shot) start only on request. [docs/submission-115.md](../docs/submission-115.md)
says which of these run in Basecamp and why the rest stay in Compose.
The [component map](../docs/architecture/deployment-components-and-rpcs.md#component-map)
draws them: which container is a Basecamp, and which module in it is upstream,
patched or ours.

## Prebuilt images (no build)

Every release attaches `lez-swap-stack-<tag>-arm64.tar.gz`: the compose
file, these scripts and `release.env` naming the linux/arm64 images the
release workflow pushed to `ghcr.io/gateway-fm/lez-atomic-swaps/lez-*`.
On an arm64 host with Docker (Apple silicon, or arm64 Linux), with `jq`,
`openssl`, `xxd`, `python3` and bash 4.4 or newer (5.1 for `regress.sh`). macOS ships bash 3.2 as
`/bin/bash`: `brew install bash coreutils` once (coreutils brings the `timeout`
that `regress.sh` uses); `scripts/host.sh` refuses an older bash at the start
of every script and names that command.

```sh
tar -xzf lez-swap-stack-v0.4.0-arm64.tar.gz && cd lez-swap-stack-v0.4.0-arm64
./scripts/start.sh            # pull → wallet identities → config → stack → market bootstrap → desk suites
./scripts/start.sh --swap     # …and one full BTC → LEZ swap through the two desks
```

`start.sh` mints the four wallet identities into `market/` with the
`lez-wallet-module` image (the `tools` service), brings the stack up through `up.sh` in pull mode
(`LEZ_IMAGES=pull`), runs the idempotent market bootstrap in one throwaway
tools container, restarts both Nodes on the recorded escrow deployment and
runs the desk suites in both parties' Basecamps. Rerunning it resumes; `./scripts/down.sh` stops the
stack and `--wipe` removes chains and Node state. `market/identities` holds the
funded wallets and survives a wipe; `market/bootstrap` and
`market/market-bootstrap.env` belong to one LEZ chain, so `--wipe` retires them
(renamed `*.chain-<time>.bak`) and the next `start.sh` deploys the escrow again.
The bootstrap also refuses a deployment recorded for another genesis, and when it
fails `start.sh` prints its whole output, stops both Nodes and exits 2. The same script works from a checkout of the tagged commit
when `LEZ_IMAGE_PREFIX` and `LEZ_IMAGE_TAG` are exported.

The images are built by `.github/workflows/release-images.yml` when a GitHub
release is published (never on pushes or pull requests), and only when the
tagged commit has finished, green `ci.yml` and `public-readiness.yml` runs
(its first job, `scripts/check-release-ci.sh`, fails otherwise), from the very phases
`from-scratch.sh` runs below, on GitHub's arm64 runners; the bundle is
attached to that release only after a fresh runner has started the stack from
it (ADR 0215). Publish the release with its notes first; the assets follow
about 2.5 hours later. `scripts/package-dist.sh <prefix> <tag>` produces the bundle
locally from a committed tree.

Upgrading from one release to the next is not supported before 1.0.0. Drain
the old release first: withdraw every offer, let each open swap reach
`completed` or `refunded` (a refund waits for its refund time), then install
the new release on fresh state. The work to
support upgrades is issue #133 and starts after 1.0.0.

## From scratch

One command takes an arm64 host (macOS with Docker Desktop, or Linux with
Docker) from nothing to the running stack with both desks verified:

```sh
./scripts/from-scratch.sh          # prerequisites → pinned sources → payloads (ephemeral builders) → stack → desk suites
./scripts/from-scratch.sh --swap   # …and one full BTC → LEZ swap through the two desks
```

It installs the host tools (Homebrew on macOS), clones the pinned
`logos-execution-zone` and `logos-basecamp` next to this repository, builds the
Basecamp bundle, both role packages and the Chat/Delivery modules in the pinned
Nix image, builds the Node binaries and Bitcoin actors in the pinned Rust
image, builds the LEZ services, r0vm, rapidsnark, the escrow artifact, the LEZ
sidecar and the four wallet identities in throwaway containers of the builder
(the sidecar carries its source-tree stamp; staging and the Node image builds
refuse a sidecar built from another `compat/lez-v0_2-sidecar` tree)
image (`builder/`, caches in `lez-build-*` volumes, outputs in
`provision/data`), stages every image payload, then runs `up.sh`, the market
bootstrap (one throwaway container on the stack network), both desk suites and
`verify-all.sh`. No long-lived build container remains, and only the
reproducible Risc0 guest build sees the host Docker socket, for that one run. Every phase is idempotent and resumable (`--only <phase>`).
The cold path builds several Rust toolchains' worth of code and takes a few
hours on Apple silicon; a rerun takes minutes.

Two macOS specifics. If Docker Desktop keeps a Docker Hub login in the
Keychain (`credsStore: "desktop"` in `~/.docker/config.json`), every pull and
every BuildKit `FROM` lookup asks the Keychain, and macOS raises a prompt in
the terminal's name; unattended runs then hang or fail with
`DeadlineExceeded`. Click "Always Allow" once, or `docker logout` so public
pulls stay anonymous. And the Docker VM disk fills up from build caches over
time: `docker builder prune` and `docker image prune` are safe; the
`lez-build-*` volumes are caches you may drop at the cost of a rebuild.

## Record and share evidence

To repeat the concurrent-swap and both refund scenarios with fresh wallets,
see [the evidence guide](EVIDENCE.md). `scripts/from-scratch.sh --evidence`
prepares an API-only stack with fast timing and automatic regtest funding.
The portable recorder produces an offline replay page and public chain checks.

## Quick start

```sh
./scripts/up.sh                    # start or resume everything: chains, market, both Nodes, UI
./scripts/up.sh --fresh-lez        # recreate the LEZ chain only (Bedrock cannot restart after long uptime)
./scripts/up.sh --build            # also rebuild the images from the staged payloads
./scripts/swap-through-ui.sh       # one BTC → LEZ swap through the two desks
./scripts/export-node-evidence.py  # publish a completed swap's public evidence (explorer + proof view)
./scripts/down.sh                  # stop everything (--wipe also removes state and volumes)
```

A stop is safe for the LEZ chain: the LEZ node and the indexer (both in the
`network` Basecamp) and the sequencer shut down cleanly on SIGINT only, so
compose stops them with it; `up.sh` then resumes the same
chain. If one of them was killed anyway (a crash, `docker rm -f`) and does not come
back, `up.sh --fresh-lez` recreates the LEZ chain and keeps Bitcoin and the wallets.
It abandons every open swap (their LEZ legs go with the chain): never run it with
real funds locked. The per-swap keys of an open swap, the Bitcoin refund key among
them, are kept under `.reset-archive/` in the Node's state volume (see
`reset-swaps.sh` below).

The local LEZ devnet runs faster than upstream's: a LEZ block every second
(sequencer `block_create_timeout`, upstream 10 s) and finality depth 2 (Bedrock
`security_param`, upstream 5, so an epoch lasts 40 s instead of 100 s). Bedrock
includes about one inscription every 2.2 s at this pace, and a transaction is
finalized in the indexer ~5 s after it is sent (~9 s on upstream's timing).
gen-config.sh renders both settings from the upstream files in `assets/lez-source`.

The same command is the remedy for a frozen devnet (logos-blockchain#3643). The
mechanism:

- The sequencer pays for each inscription from a Bedrock wallet key: the exact fee at
  the tip it funds against, plus a tip (`priority_fee`, upstream 200).
- Bedrock re-prices storage at every epoch turn, by up to an eighth and by at least
  one unit.
- An inscription in flight across a turn that raised the price no longer pays enough.
  Bedrock drops it (`InsufficientBalance`), and every later inscription with it,
  because each names the dropped one as its parent (`InvalidParent`).
- Bedrock's log shows `0 transactions (N removed)` with N growing. The indexer's
  finalized height stands still and every swap waits.
- The sequencer resubmits the same underpaid transactions forever, so restarting it
  does not help.

On upstream's settings, isolated chains froze 3 to 8 minutes after they started, and
at 1 s blocks within 4 minutes.

gen-config.sh removes the cause with configuration only:

- The sequencer's tip is 10^7 (`LEZ_SEQUENCER_PRIORITY_FEE`), enough to absorb one
  price step for a 9 KB LEZ block until the storage price passes ~8000.
- The price only drifts up (~+45 an hour), so the tip costs ~10^7 per LEZ block.
  Upstream's 48 x 10^6 of genesis funding would run dry within an hour (it did, in
  46 minutes, at 1 s blocks). The devnet's genesis gives those 48 notes 10^12 each.
- The sequencer no longer pays from the block producer's notes. Upstream's sequencer
  funds from the leader's own genesis key, and Bedrock's leader can only win a slot
  with a note that is in its epoch's aged snapshot and still unspent. The sequencer
  spends one note per LEZ block, so at ~1.5 s a block it left the leader no eligible
  note at an epoch turn: Bedrock stopped proposing and the sequencer exited on
  `Wallet does not have enough funds, available=0`. In the devnet's genesis 40 of the
  48 notes belong to the node's SDP key (`ed26..`), which is the sequencer's
  `funding_key`; the leader keeps 8 notes nobody spends (commit `88fcabbf`).
- The notes' values and owners are part of the genesis transaction, so the genesis
  block root and the SDP declaration's locked note id were recomputed for them
  (`builder/regenesis`); `gen-config.sh` checks both against the upstream file.
- Each new chain's genesis time is the moment it is made (`LEZ_GENESIS_TIME` in
  runtime.env, kept for its restarts), computed by `builder/regenesis` in the LEZ
  network's image. With upstream's time (2026-07-31) the indexer's cold start first
  read every slot since then, which delayed a new chain's first finalized block and
  every re-index; an image without the tool still starts chains that way.

`regress.sh` treats a chain whose storage price has reached 6000 (about five days of
chain time) as worn and starts the next test on a new one.

If a chain still freezes, the fix is the same: start a new one.
`up.sh --fresh-lez` waits for the new chain's first finalized height, which a healthy
chain's indexer serves ~15 s after the chain is healthy, for up to 300 s
(`LEZ_FRESH_FIRST_FINALITY_SECONDS`). It then waits 60 s for the height to move
(`LEZ_FRESH_FINALITY_SECONDS`). It starts another chain at once when either never
comes, or when the market bootstrap cannot finish; six chains at most
(`LEZ_FRESH_ATTEMPTS`). `regress.sh` calls a chain frozen after 60 s of flat finality
(`LEZ_REGRESS_FLAT_SECONDS`), the heartbeat after 60 s, and `node-e2e.py` checks that
finality advances before each scenario.

The block explorers are off by default (Compose profile `explorers`):
`LEZ_EXPLORERS=1 ./scripts/up.sh` starts them with the stack, `verify-all.sh`
starts them for its explorer checks, and
`docker compose --env-file runtime/runtime.env --profile explorers up -d` at any time.

`up.sh` ends with the repo-style desk verification: each party's desk, in that
party's own Basecamp, driven through its QML inspector against the party's live
Node. Skip with `SKIP_UI_VERIFY=1`.

| What | Where |
|---|---|
| Bitcoin regtest RPC | `http://127.0.0.1:18443` (user `lezrpc`, password in `runtime/runtime.env`), auto-mining every 120 s |
| LEZ indexer JSON-RPC | `http://127.0.0.1:8779` (`getLastFinalizedBlockId` and the rest of `indexer_service`'s methods) |
| BTC explorer (profile `explorers`) | http://127.0.0.1:3002 |
| LEZ explorer + M3 evidence (profile `explorers`) | http://127.0.0.1:3003/#/evidence |
| **Maker's Basecamp (VNC)** | **`vnc://127.0.0.1:5901`**: the Maker desk (password `lezswap`; override with `VNC_PASSWORD`) |
| **Taker's Basecamp (VNC)** | **`vnc://127.0.0.1:5902`**: the Taker desk (same password) |
| Maker Node | `docker exec lez-maker-node lez-maker-cli --socket /run/lez/maker/node.sock health` |
| Desk verify | `docker exec lez-maker-node node /ui-tests/verify.mjs maker`, `docker exec lez-taker-node node /ui-tests/verify.mjs taker` |
| Hold a Node down (its Basecamp and desk stay up) | `scripts/node-hold.sh hold\|release maker\|taker` |
| Logs | `docker compose --env-file runtime/runtime.env logs -f <service>` |

Open the two VNC URLs; each party's desk is the control surface of its own
Basecamp, and the two Nodes settle the swap:

1. In the Maker's Basecamp, **LEZ Maker** (wallet **Munich Vault 01**, the Maker Node's
   identity) and click **Publish offer**. Each click publishes one 0.01 BTC →
   1,000 LEZ offer through the Maker Node; withdrawn and consumed offers leave
   the Taker's order book.
2. In the Taker's Basecamp, **LEZ Taker** (wallet **Zurich Wallet 01**, the Taker Node's
   identity) and click **Take offer** on an order-book row. The Taker Node
   reserves the lot with the Maker, plans its Bitcoin funding, prepares its LEZ
   claim, runs the signing ceremony and activates its actor; the row then
   offers **Lock 0.01000000 BTC**.
3. Advance the swap like two real users:

   | Direction | Role-owned steps |
   |---|---|
   | BTC → LEZ (`TakerSellsForeign`) | Taker: Lock BTC → Maker Node funds LEZ → Taker: Claim LEZ → Maker Node claims Bitcoin |

   The Maker's two steps are its Node's automatic effects; the Maker desk shows
   their progress. The Taker's revealing claim stays a user action.
4. `scripts/export-node-evidence.py` publishes the completed swap's five
   transaction identities (confirmed against both chains) to the explorer's
   evidence view (`http://127.0.0.1:3003/#/evidence`) and hash index;
   `swap-through-ui.sh` does so on completion. Until the first swap, that view
   shows the certified sample `m5arm-08180005`.

## Manual test walkthrough

Everything the automated swap does can be done by hand and watched from the
outside. Two helpers for the owner sockets (run from `deploy/`):

```sh
set -a; source runtime/runtime.env; set +a
maker() { docker exec lez-maker-node curl -sS --unix-socket /run/lez/maker/node.sock \
  -H 'content-type: application/json' --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$1\",\"params\":[${2:-{\}}]}" http://localhost/ | jq; }
taker() { docker exec lez-taker-node curl -sS --unix-socket /run/lez/taker/node.sock \
  -H 'content-type: application/json' --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$1\",\"params\":[${2:-{\"schema_version\":1\}}]}" http://localhost/ | jq; }
btc() { docker exec lez-bitcoin-core bitcoin-cli -conf=/run-config/bitcoin.conf -datadir=/var/lib/bitcoin "$@"; }
```

1. **Stack.** `docker compose --env-file runtime/runtime.env ps` shows every
   service healthy. `maker maker_health` and `taker taker_health` answer.
   `./scripts/reset-swaps.sh` gives you an empty swap history when you want a
   clean run (`--abandon-open` when a swap is still open).
2. **Publish.** Connect VNC to the Maker's Basecamp at `127.0.0.1:5901`
   (password `lezswap`); on **LEZ Maker** click **Publish offer**. Check it from outside:
   `maker maker_offer_list` lists it `active` at revision 1, and
   `docker exec lez-taker-node ls /delivery` shows its signed file.
3. **Discover and take.** Connect VNC to the Taker's Basecamp at
   `127.0.0.1:5902`; on **LEZ Taker** the order book shows the
   offer as **Munich Vault 01 · 1,000 LEZ**. `taker taker_offer_list_v1` shows
   the same. Click **Take offer**. Within about ten seconds the row offers
   **Lock 0.01000000 BTC**. Meanwhile: `taker taker_swap_list_v1` lists the
   swap in `awaiting_first_lock`; `maker maker_offer_list` shows the offer
   `consumed` with its `swap_id`; the Delivery file is gone; both containers
   run one new `lez-v02-bridge-poc`
   (`docker exec lez-taker-node ps -eo pid,args | grep bridge`); each Node has
   a new `swaps/<reservation id>/` directory.
4. **Lock BTC.** Click **Lock 0.01000000 BTC**. `taker taker_swap_list_v1`
   moves to `locking_btc` and then `awaiting_maker_lock`;
   `btc -rpcwallet=lez-taker listtransactions '*' 1` shows the funding
   transaction; it appears on http://127.0.0.1:3002 once the miner includes it
   (a block every two minutes).
5. **Maker funds LEZ (automatic).** The Maker desk moves from **Waiting for the
   Taker's lock** to **Funding the LEZ escrow** to **Waiting for the Taker's
   LEZ claim**; `maker maker_actor_monitor_v1 '{"id":"<swap id>"}'` reports the
   phase. The escrow initialization and funding appear in the LEZ explorer
   (http://127.0.0.1:3003, latest blocks).
6. **Claim LEZ.** The Taker row offers **Claim 1,000 LEZ**; click it. The
   revealing claim lands on LEZ, then the Maker Node claims the Bitcoin
   (**Claiming Bitcoin** on the Maker desk; `btc -rpcwallet=lez-maker
   listtransactions '*' 1`). Both `taker taker_swap_list_v1` and the Maker
   monitor end at `completed`, revision 4.
7. **Proof.** `./scripts/export-node-evidence.py` writes the swap's five
   transactions, confirmed against both chains, to `runtime/evidence/` and the
   explorer's evidence view. Open http://127.0.0.1:3003/#/evidence and paste
   any of the five hashes into the explorer's search.
8. **Verify without another swap.** `./scripts/verify-all.sh`: containers,
   both chains, explorer display of every exported swap, the Node market
   (15 checks) and both desk suites.

Things worth trying deliberately: withdraw an offer on the Maker desk and
watch it leave the Taker order book; `docker restart lez-taker-node` between
lock and claim and watch the observer resume; `docker restart
lez-bitcoin-core` and watch the next action still reach Core through the
Node's own loopback forwarder. Where to look when something stalls:
`docker compose logs maker-node taker-node` (both Nodes' actor supervisors
print one secret-free line per distinct supervised effect — `maker actor <swap>:
drive → …` / `taker actor <swap>: drive → …` with command, outcome, operation,
phase, revision, next action — a line per backoff class when it first appears
or changes, and the Taker Node one line when a swap's bundle fails its checks;
the Taker Node drives its Zcash actor itself, from the take's activation
through the LEZ lock and the observation of the Maker's legs to
claim_available, every 5 s while something moves and every 30 s after a
failure, with the class on the row's `last_failure_class`), the
swap directory's `sidecar.log`, and
`docker exec lez-taker-node lez-btc-taker-actor --config /var/lib/lez/taker/btc/swaps/<reservation id>/actor/actor-config.json status`.

## Swap scenarios through the Node APIs

`scripts/node-e2e.py <scenario>` drives both Nodes over their owner sockets
without the desks: it publishes, takes, locks, claims, mines regtest blocks on
demand, and stops or restarts a Node where the case calls for it. Each
scenario first runs `repair-indexer.sh` (the standing indexer must serve
historical reads or the Maker's sidecar cannot classify its escrow), then is
one subcommand with its own exit code and a summary in
`runtime/e2e/<scenario>.json`, so they run as independent CI jobs
(`.github/workflows/node-e2e.yml`, one matrix job per scenario on a
self-hosted `lez-stack` runner).

| Scenario | What it proves |
|---|---|
| `happy` | BTC→LEZ: take, lock, Maker funds, claim, Maker claims; both Nodes `completed` |
| `replay` | publish, take, lock and claim repeated with their request ids: same result, no second effect |
| `wrong-inputs` | stale revisions, mismatched envelope hash, off-preset amount, wrong quote, unknown swap, early claim, taking a consumed lot: all rejected fail-closed |
| `restart-taker`, `restart-maker` | a Node restarted between lock and claim resumes and completes |
| `survivor` | the Taker Node is stopped right after its revealing claim; the Maker completes alone; the Taker catches up on restart |
| `late-claim-confirms` | the revealing claim is sent and the Taker Node stopped before it has seen it confirmed; the claim confirms while the Taker is down (selling LEZ: a Bitcoin claim held in the mempool, the miner stopped, then mined; selling Bitcoin: the Taker's indexer forwarder cut as its LEZ claim goes out, then the Node stopped); the Maker completes alone, the Taker on restart, and both chains show the lots traded |
| `late-claim-unmined` | the revealing claim is sent but never mined, the Taker Node stopped, until the refund path is open (selling LEZ: empty blocks at the profile's pace mature the Maker's Bitcoin refund with the claim still in the mempool; selling Bitcoin: the Taker's sequencer forwarder cut so its LEZ claim is never included, through the claim window); then the Maker asks for its refund, the Taker comes back and both Nodes settle on their own: either outcome passes, but both chains' balances must fit it and agree with both Nodes (no party with both lots or neither) |
| `concurrent` | two swaps interleaved end to end (one sidecar per swap) |
| `early-lock` | the Taker's lock is in front of the Maker unconfirmed (selling Bitcoin: in the mempool; selling LEZ: included at the sequencer, its finality source, the indexer, cut); the Maker must not lock: its phase, its own record and the chain (its LEZ balance, or its Core wallet's sends) stay clean |
| `offline-after-lock` | after the first lock Chat and Delivery are denied to both Nodes' user (socket and directory to mode 0000, refused with EACCES from the Taker's container); the Maker holds no Chat connection until the swap completes; both Nodes report both dependencies unavailable; the restore is checked from both sides |
| `taker-refund` | the Maker never funds; after the cutoff and CSV maturity the Taker refunds its Bitcoin; the Maker reconciles |
| `maker-refund` | the Taker never claims; the Maker refunds LEZ after `refund_at`, then the Taker refunds Bitcoin |
| `refund-then-maker-lock` | the Taker's refund is admitted with the Maker's lock on chain but unobserved (asserted on the Taker's view), then the lock appears; the Taker's observer refunds unaided, the Maker refunds too, and both parties' balances on both chains are back less the locker's Bitcoin fees |

Each long wait logs its state, LEZ's finalized height and the sequencer's tip when
the state changes and at least once a minute, and a timeout names what it waited
for and what it saw last; `regress.sh` writes both heights on each try's start and
verdict line and every 30 s into `chain.tsv`.

The refund scenarios need the `refund` timing profile (they also run on `fast`,
only slower); `regress.sh` switches to it by itself and back to `fast` for the
other Bitcoin tests:

```sh
LEZ_TIMING_PROFILE=refund ./scripts/up.sh     # a block every 2 s; 123 and 65 CSV blocks; 100/115/245 s deadlines, 15 s margin; 130-block LEZ window
./scripts/node-e2e.py taker-refund
```

`refund` follows from the LEZ chain's step latency instead of fixed numbers
(`gen-config.sh`): the cutoff is two LEZ steps (`LEZ_LEZ_STEP_SECONDS`, 20 s
on the 1 s devnet: ~5 s to finality plus the Nodes' polling) plus 20 s of desk
and a finality stall (`LEZ_LEZ_STALL_SECONDS`, 40 s); earlier = cutoff +
margin; the later refund is the least the refund order below allows; the LEZ
window is the cutoff plus 30 s in blocks of the fastest LEZ pace
(`LEZ_LEZ_BLOCK_SECONDS`, 1). On the old 10 s chain the same rule with
`LEZ_LEZ_BLOCK_SECONDS=10 LEZ_LEZ_STEP_SECONDS=45 LEZ_LEZ_STALL_SECONDS=70`
gives a 180 s cutoff, refunds at 195 and 405 s and a 21-block window. The values are kept across reruns like the profile.

`local` (the default) keeps network-like deadlines: a 30-minute Maker cutoff,
60- and 120-minute refund bounds, a 3600-block LEZ discovery window. The profile
is configuration only (`LEZ_BTC_*` and `LEZ_LEZ_DISCOVERY_MAX_BLOCKS` in
`runtime.env`, rendered into each Node's `btc-role.json`).

A Bitcoin refund delay is a block count and the rest of the schedule is seconds,
and only the order of the two refunds protects either side: a Bitcoin claim has
no deadline on chain. So the delay differs by direction (`refund_csv_blocks`
when Bitcoin is the first lock, `second_lock_refund_csv_blocks` when it is the
second), the profile states the pace the chain may keep (`block_seconds`), and a
Node **refuses to start** on a profile that breaks any of three rules:

- first lock: `blocks x fastest >= later` — the Taker's Bitcoin refund does not
  mature before the later refund time;
- second lock: `blocks x fastest >= earlier + margin` — the Maker's Bitcoin
  refund does not mature inside the claim window, where it could race a
  revealing claim;
- second lock: `cutoff + blocks x slowest + margin <= later` — it matures before
  the Taker's LEZ refund opens, after which the Taker could refund its LEZ and
  still claim the Bitcoin.

`local` uses 60 and 33 blocks at the regtest miner's 120 s; `fast` and `refund`
use the fewest blocks the rules allow at `LEZ_BTC_BLOCK_SECONDS` (2 s: 420 and
240 on `fast`, 123 and 65 on `refund`; at 20 s `fast` is the old 42 and 24) (the profile sets the miner's pace,
`MINE_INTERVAL`); regtest still mines past a delay on demand.

The LEZ discovery window sets how soon a refund can start when the Maker never
locks: the Taker asserts the Maker lock's absence only once every block of the
window is finalized. The window must cover the Maker cutoff at the fastest pace the
devnet can make LEZ blocks (one a second: 240 blocks on the fast profile, 300 kept).
At the measured ~2.2 s per block, the fast profile's 300 blocks mean the refund becomes
eligible about 11 minutes after the take.

## Settlement chains

Swaps settle on one permanent pair of chains, the way they must against real
Bitcoin and LEZ networks — chain lifecycle is not the swap's job:

* **Bitcoin**: the standing `bitcoin-core` regtest chain (`txindex`,
  `txospenderindex`), continuously mined by `btc-miner`. Swap transactions are
  native chain transactions, visible in the Bitcoin explorer.
* **LEZ**: the standing chain of the `network` Basecamp (LEZ node and indexer) and the
  `sequencer` (genesis allocations
  of 2 000 000 LEZ to each role's signer and settlement wallet, `gen-config.sh`, which
  writes them to `runtime/runtime.env` as `LEZ_WALLET_{MAKER,TAKER}_ALLOCATION` for the
  market bootstrap's vault claims, the one place the number lives: a
  ZEC corridor's 50 000-LEZ lot leaves the depositor for good on a completed swap,
  so a whole matrix fits on one chain; `LEZ_BOOTSTRAP_DEFER_CLAIM=<wallet>` on
  `up.sh --fresh-lez` leaves one wallet's allocation in its vault, which is how
  the depositor-balance refusal is shown, and `scripts/bootstrap-market.sh`
  run again claims it), whose genesis
  funds four persistent wallets (Munich, Basel, Zurich, Limmat). Balances
  accumulate across swaps; the UI's wallet ledger shows true opening → closing
  values, not per-run genesis allocations.
* **Finality heartbeat** (`heartbeat` service, `lez-wallet-heartbeat` from the
  lez-wallet-module image: its transfers go through the LEZ wallet as the Logos
  lez_core module): the pinned Bedrock's finalized height freezes while the
  sequencer keeps publishing (logos-blockchain#3643). The service is the
  **liveness detector**, not a keep-alive: measured on 2026-10-29, chains with
  the heartbeat from genesis froze after 36–56 minutes (controls without it
  after 17–30) while every heartbeat transfer kept being included in about
  two seconds, so a steady trickle does not prevent the freeze; it names it
  within two minutes. It sends one authenticated transfer of one LEZ unit from
  `heartbeat-pulse-01` to `heartbeat-echo-02` (two identities minted beside the
  wallets, funded 100 000 each in the genesis, claimed by the market bootstrap;
  never a role's or a wallet's account) every `LEZ_HEARTBEAT_SECONDS` (default
  20; `0` in `runtime/runtime.env` disables it, `gen-config.sh` writes it), with
  one log line per tick (`heartbeat: tick n tx … tip T finalized F (flat s)`).
  When the finalized height is flat for `LEZ_HEARTBEAT_STALL_SECONDS` (default
  60) while the tip advances it prints `heartbeat: STALL …` (again every five
  minutes while it lasts, once when it clears) and writes
  `runtime/heartbeat/status.json` with `stalled: true`, which the runners read
  instead of waiting out a vault sweep on a dead chain.
  Both Nodes apply the same rule to their own reads of the indexer and the
  sequencer with the same threshold (`LEZ_NODE_FINALITY_STALL_SECONDS`, which
  `gen-config.sh` sets to `LEZ_HEARTBEAT_STALL_SECONDS`): `maker_health` and
  `taker_health` turn `degraded` with `lez_chain.reason: lez_finality_stalled`
  and both desks show "LEZ finality has not advanced for N min — swaps wait
  until it does" (docs/api/README.md; `scripts/ui-e2e.sh lez-finality-stall`
  checks it by stopping the sequencer).

Both carry `org.logos-co.atomic-swaps.{run,scope,component}` labels
(`market-btc-0001` / `market-lez-0001`). The one-time bootstrap — escrow
program deployment and the four wallet vault claims — is idempotent:

```sh
./scripts/from-scratch.sh --only stack    # runs it inside one throwaway builder container
```

The bootstrap runs `scripts/market-bootstrap.sh` in one throwaway container on
the stack's network with the escrow deployer and the vault-claim tool: the
`lez-builder:local` image (pulled by the digest in `builder/image.lock`, or
built locally; see `builder/README.md`) with the tools from `provision/data`
on a developer host (`from-scratch.sh`), or the published `lez-wallet-module` image
(compose profile `tools`, `start.sh`); the vault claims go through the LEZ wallet
module (`lez-wallet-vault-claim`) either way. Both tools accept only literal-loopback URLs, so the
container forwards `127.0.0.1:3040/8779` to `sequencer`/`indexer` for the run.

Chains are never torn down between swaps. Wallet identities and the bootstrap
manifest live in the market root (`LEZ_MARKET_ROOT`: `market/` next to this
repository, or `runner-work/market/` on hosts provisioned earlier); deleting
them would strand the funded accounts.

## Verification

```sh
./scripts/verify-all.sh          # everything below, in one run
```

| Stage | What it proves |
|---|---|
| containers | every service is up |
| settlement chains | both chains are advancing and the Bitcoin spender index is present |
| `verify-explorers.py` | each exported swap's transactions are *displayed* — Bitcoin ones in a real block on the Bitcoin explorer (rendered content, hidden markup excluded), LEZ ones as live transactions with program and accounts. The certified sample predates the settlement chains and is checked against the proof endpoint, since its chains no longer exist |
| `verify-market.py` | the Node market: route terms, publication, idempotent replay, request-identity reuse, discovery through Delivery, stale-revision rejection, withdrawal |
| desk regressions | each party's desk suite, in that party's Basecamp, against its live Node |

The BTC view can be driven automatically, on the desk the Taker's Basecamp has
open:

```sh
docker exec lez-taker-node node /ui-tests/verify.mjs taker
```

For a completed reverse-flow evidence file, select its exact UI contract while
running either role suite:

```sh
docker exec -e M3_UI_DIRECTION=TakerSellsLez lez-taker-node node /ui-tests/verify.mjs taker
```

### Regression lanes

`scripts/regress.sh` runs the devnet regression one test at a time by default.
`LEZ_LANES=N scripts/regress.sh <tier>` runs N at a time, one per lane: lane 1
is this stack, and lanes 2..N (`scripts/lanes.sh up N`, Compose projects
`lez-lane-<k>` with `compose.lane.yaml`) are each one more Maker/Taker pair,
each party's own Basecamp, with its own Bitcoin Core, miner and Zebra, on the
same LEZ network (`scripts/lane.sh` names a lane's containers `lez-l<k>-*`, its
files `runtime/lanes/<k>/`, its Zebra `127.0.0.1:18232+10(k-1)` and its VNC
`5901+10(k-1)`/`5902+10(k-1)`). Lane 2 settles as `maker-basel-02` /
`taker-limmat-02`, which every genesis funds. Lanes 3 and up settle as market
wallets of their own: `scripts/lanes.sh mint N` mints them into the market
root, and the next LEZ chain (`up.sh --fresh-lez`) funds them. One watcher follows LEZ finality
for all lanes: a frozen chain voids every lane's test, and one new chain
(`up.sh --fresh-lez` with `LEZ_LANES`, which stops and restarts the lanes) serves
them all. The scenarios, `reset-swaps.sh` and `tests/fixtures/zec-catalog-fixture.sh` act on one lane
when `LEZ_LANE=<k>` is set: `LEZ_LANE=2 bash scripts/ui-e2e.sh happy`.

### Monero (LEZ_MONERO=1)

Monero swaps run on this devnet only: an offline Monero **regtest** chain on
each lane, on **x86_64 hosts only** (the pinned release is `linux-x64`;
`stage-assets.sh` refuses another architecture). No Monero stagenet, testnet or
mainnet route exists: the Nodes' Monero role accepts only the
`accelerated-regtest` profile. The protocol, its windows and the punish branch
are in [protocol-design.md](../docs/milestone-1/protocol-design.md#xmrlez).

Monero is off by default: a Bitcoin/Zcash run starts and waits for nothing of
it, and a release bundle, which publishes no Monero image, never pulls one.
`LEZ_MONERO=1` turns it on for the stack and every lane (Compose profile
`monero`; `gen-config.sh` keeps the choice in `runtime.env`, so a later
`up.sh --fresh-lez` keeps it, and `LEZ_MONERO=0` turns it off again):

```sh
LEZ_MONERO=1 bash scripts/stage-assets.sh    # once: fetch and verify the pinned release
LEZ_MONERO=1 bash scripts/up.sh              # builds lez-monerod:local, starts it on every lane
python3 scripts/monero-lane.py fund --role maker   # mine, pay the Maker 10 XMR, read its balance
python3 scripts/monero-lane.py balance --role taker
```

**The pinned release.** Monero 0.18.5.1 (`tests/e2e/monero/provenance.env`:
tag `v0.18.5.1`, source commit `4f92268d…`, archive
`monero-linux-x64-v0.18.5.1.tar.bz2` with its sha256 `22a7dda7…`, the signed
`hashes.txt` checked against the pinned signer key, fingerprint
`81AC591F…2A0BDF92`). `stage-assets.sh` runs `scripts/verify-monero-release.sh`
in a throwaway container, keeps only `monerod` and `monero-wallet-rpc` and runs
their `--version` there; the downloaded binaries never run on the host. The
image `lez-monerod:local` (115 MB) runs them on a distroless base.

**What runs per lane.** `monerod` (`--regtest --offline`, fixed difficulty: no
peers, no miner; blocks are generated on demand) and three `monero-wallet-rpc`:
`xmr-funding-wallet`, which the harness mines to and funds the roles from, and
one per role, `maker-xmr-wallet` and `taker-xmr-wallet`. Each role's wallet is
named after the role (`maker`, `taker`), has no password, and is created by
`monero-lane.py` when its wallet-rpc has none open. Every RPC takes a Digest
login that `gen-config.sh` mints once under `runtime[/lanes/<k>]/monero`; each
role's Node gets monerod's and its own wallet-rpc's login and forwards
`127.0.0.1:18081` and `:18083` to them (the Monero adapter accepts only
literal-loopback endpoints). monerod and the funding wallet-rpc publish
`127.0.0.1:18081+10(k-1)` and `18083+10(k-1)`. Like Zebra, Monero outlives
`up.sh --fresh-lez` and `reset-swaps.sh`; `down.sh --wipe` (and
`lanes.sh down N --wipe`) remove its volumes.

**The Nodes.** Where a role has those logins, `node-entrypoint.sh` writes its
Monero role configuration `xmr-role.json` (owner-private: the endpoints, the
logins, the wallet name, the three windows, the Stage-A planner and, on the
Maker, the six effect tools) and passes it to the Node: the Maker gets
`--xmr-role-config`, `--xmr-maker-actor-root` and its pinned
`lez-xmr-maker-actor`, the Taker `initiation.xmr_role_config`. The Node
binaries are built with `pair-xmr` (`from-scratch.sh --only rust`). The route
health carries a Monero probe for the one route, Monero `TakerSellsLez`
(`probe-monero`: monerod's `get_info`; not the role's wallet-rpc, which the
Node's own tools keep busy for seconds at a time), so `maker_health` /
`taker_health` read the Monero route unavailable, and the Node degraded, while
monerod does not answer. Every Monero transfer the Nodes
send (the Maker's funding, either role's sweep) is followed by the sender
mining its own 10 confirmations; a Maker whose funding or refund-sweep tool
stopped in between (killed, or its mining call failed) mines them when it
next observes that transfer in the pool or short of its depth (a regtest
duty only: on a real network miners confirm). The harness mines only to
fund the Maker and, in `xmr-kill-maker`, for a funding whose tool it killed
before it mined.

**The windows.** `runtime.env` carries the agreement's three offsets from the
take, which both Nodes read and must have alike:
`LEZ_XMR_MAKER_FUNDING_CUTOFF_SECONDS` (C), `LEZ_XMR_REFUND_SECONDS` (R) and
`LEZ_XMR_PUNISH_SECONDS` (P). C is the Bitcoin Maker cutoff plus 20 s for the
ten confirmations, and at least 320 s: the windows start with the take, and
the second of two swaps taken one after the other has its funding drive start
only after the take (at most 44 s with three lanes busy on 2026-11-17, counted
at 60 s) and three of its drives' spacing (counted at 80 s each), plus the
20 s depth. The Maker claims only on a claim authorization it sees final
before R − (R − C)/2, and a Maker that does not act in that window forfeits
the claim to the Taker's refund, so each half of R − C holds the 20 s depth, a
LEZ step and two of the Maker's actor drives counted at 80 s each
(`xmr_drive`: the Maker Node's one actor worker drives all its swaps in turn,
and one Monero swap's drives started at most 50 s apart with the three
heaviest Monero tests on three lanes, five times on 2026-11-17; a swap's
funding stood at depth at most 143 s after its take started, and C is 2.2
times that). P is R plus a step, a stall, the margin and two such drives.
In seconds, with the LEZ step S, the stall T and the profile's Bitcoin Maker
cutoff B and margin M: C = max(B + 20, 60 + 3 × 80 + 20 = 320),
R = C + 2 × (20 + S + 2 × 80), P = R + S + T + M + 2 × 80. With the default
S = 20 and T = 40, R − C is 400 s on every profile. On `fast` (B = 240,
M = 60), the profile every Monero scenario runs on, that is +320, +720 and
+1000 s: the Taker may refund only in the 280 s between R and P, and from P a
Maker can take the LEZ while the Monero stays locked. `refund` (B = 2S + 20 + T
= 100, M = 15) gives +320, +720, +955 s (235 s to refund) and `local`
(B = 1800, M = 300) +1820, +2220, +2740 s (520 s). The drive count holds only
while the Maker Node is no busier than three devnet lanes: a Maker driving more
swaps at once spaces each swap's drives further apart and needs wider windows.

**Scenarios.** `node-e2e.py --pair Monero <scenario>` drives both Nodes over
their owner sockets (the Maker's offer through `lez-maker-cli`, the Taker over
its owner RPC); `regress.sh` runs them as `node:xmr:<scenario>:TakerSellsLez`,
on `fast`, and leaves them out with a line on a stack without `LEZ_MONERO=1`.
All nineteen are in the `full` tier, none in `smoke`. The six added with the
Node's own refund (`xmr-taker-clock-ahead` to `xmr-taker-cheats` below) have
not run yet: they were written while the devnet was being rebuilt for arm64. A lot is 1 XMR for 50 000
LEZ units.

```sh
bash scripts/regress.sh run node:xmr:xmr-happy:TakerSellsLez
bash scripts/regress.sh run node:xmr:xmr-punish:TakerSellsLez node:xmr:xmr-kill-maker:TakerSellsLez
```

| Scenario | What it asserts |
|---|---|
| `xmr-happy` | take over Chat, the Taker's LEZ lock, the Maker's Monero funding at depth, the Taker's claim, the Maker's LEZ claim, the Taker's sweep; LEZ to the unit, the Taker's one received transfer the lot less at most 0.01 XMR of fee, both Nodes completed |
| `xmr-taker-refund` | the Maker funds, the Taker does not claim and its owner refunds from R (the Node's own refund switched off for the scenario: `xmr-automatic-refund`, below); the Maker sweeps its Monero back; the Taker's owner signs nothing for the refund |
| `xmr-taker-refund-kill` | as `xmr-taker-refund`, the Taker's container killed with SIGKILL right after the refund's admission; started again it submits the refund |
| `xmr-maker-never-funds` | the Maker Node is stopped right after the take and kept down until the Taker Node refunded from R by itself, with no refund commanded; back up, the Maker ends `expired`; no Monero moved |
| `xmr-punish` | the Maker funds, the Taker Node is stopped once it has verified the Monero; from P the Maker sends tag 17 at a live owner nonce other than 0 and ends `punished`; the LEZ moves to the Maker, the Monero stays at the shared address, the Taker back up reads `attention_required` (`xmr_refund_window_closed`) |
| `xmr-concurrent` | two swaps of the same Nodes at once, one offer each; both complete; two fundings, two sweeps |
| `xmr-kill-taker` | the Taker killed with SIGKILL at each of 7 steps (after the take, lock in flight, locked, Monero verified, claim authorised, waiting for the Maker's claim, sweeping), one fresh swap each; each completes with the owner accounts' LEZ transaction counts and the Monero moved once |
| `xmr-kill-maker` | the Maker killed at each of 6 steps (after the take, the Taker's lock observed, its Monero funding in flight, funded, its LEZ claim tool running, its LEZ claim in flight); each completes or, killed with the Taker's lock observed or its funding in flight, may end refunded with nothing lost |
| `xmr-monerod-down` | the lane's monerod stopped before a take: both Nodes read the Monero route unavailable (the Maker withdraws and refuses Monero offers, the Taker refuses a take), Bitcoin's and Zcash's available; started again, a swap completes; then stopped right after a take until the Maker has seen the Taker's lock final and 60 s more, within the funding cutoff: the swap completes, or ends refunded with nothing lost. The Maker's drive asks monerod before it spends the swap's one funding and keeps it for a later drive while monerod does not answer, so an outage that ends before the cutoff leaves the funding to go out after it; one that begins while the funding tool runs (its three attempts five seconds apart) still spends it, and the swap is refunded |
| `xmr-wallet-restart` | a role's `monero-wallet-rpc` restarted mid-swap: the Maker's after the take, the Taker's while it watches for the Monero and before its sweep; both swaps complete |
| `xmr-outage-before-take` | Logos Chat and Delivery cut before a take: both Nodes degraded, the take refused, no swap on either Node; restored, the same request takes the offer and the swap completes |
| `xmr-cut-after-lock` | Chat and Delivery cut once the Taker's LEZ lock is out, until the swap completes under the cut |
| `mixed-three-pair` | a Monero, a Bitcoin and a Zcash swap of the same Nodes at once, all TakerSellsLez; all three complete |
| `xmr-taker-clock-ahead` | the Taker Node's clock 60 s ahead of LEZ time (test hook `clock-offset-ms`), its own refund off: the owner refunds as soon as the swap offers it, 60 s before LEZ time reaches R; no refund is recorded as sent before the finalized LEZ time reaches R, then it lands and the Maker sweeps back |
| `xmr-punish-replans` | as `xmr-punish`, with the punishment's nonce taken six times (the Maker sidecar's test hook `punish-nonce-thefts` sends one LEZ unit from the Maker's owner to the Taker at that nonce instead of the punishment); the Maker re-plans past five and ends `punished`; the lot moves to the Maker less the six units |
| `xmr-maker-claim-without-authorization` | a cheating Maker (the test-only probe, `--effect xmr-claim`) sends tag 15 with no tag 14 and no Monero (its wallet-rpc stopped): refused, the escrow stays `Funded`; the Taker Node refunds by itself; the Maker ends `expired`. The Maker can witness tag 15 only with its own key, since the claim authority is the two parties' MuSig2 key |
| `xmr-maker-early-punish` | a cheating Maker sends tag 17 before P: refused, the escrow stays `Funded`; the Taker Node refunds from R; past P tag 17 again: refused, the escrow is `Refunded`; the Maker sweeps back |
| `xmr-unfunded-punish` | TM-0031: an unfunded swap (the Maker's wallet-rpc stopped), the Taker Node stopped through [R, P): tag 17 from P pays the Maker the LEZ; then the same with the Taker Node running: it refunds by itself from R and tag 17 from P is refused |
| `xmr-taker-cheats` | a cheating Taker (test hooks `cheat-refund-before-r`, `cheat-refund-after-claim`): tag 16 before R is dropped and the escrow keeps the LEZ, then the same refund lands from R; on a completed swap tag 16 from R is refused and no LEZ moves |

**The Taker's own refund and the test hooks.** The Taker Node refunds a
Monero swap's LEZ by itself from R (finalized LEZ time). Its entrypoint turns
that off with `LEZ_XMR_AUTOMATIC_REFUND=false` or the word `false` in
`<state>/xmr-automatic-refund` (read at each start); `node-e2e.py` writes the
file and restarts the Node. The devnet's build (`from-scratch.sh`) compiles
the Taker Node with the feature `test-hooks` and the LEZ sidecar with its own
`test-hooks`; both read files in `<state>/test-hooks` (`LEZ_TEST_HOOKS_DIR`,
set by the entrypoint): `clock-offset-ms` (the Taker supervisor's clock
offset), `cheat-refund-before-r` and `cheat-refund-after-claim` (a cheating
Taker), `punish-nonce-thefts` (a count, in the Maker's sidecar). No release
build has either feature. A stack staged by another script must build the
Nodes and the sidecar with the same features for these scenarios.

In the full regression of 2026-11-15 they ran across lanes 1 to 3
(`docs/evidence/regression-m4-2026-11-15/`). Footprint, measured on 2026-11-13: after 190 blocks a lane's
monerod volume holds 0.5 MB and each wallet's 0.4 MB; monerod uses about
270 MB of memory, each wallet-rpc about 13 MB.

**Owner nonces.** Each role signs its LEZ transactions of every pair with one
owner account, at the account's live nonce, so two transactions prepared
before the first is in a block carry the same nonce and the sequencer keeps
one. What the Monero side does about it, and what is left:
- The Taker's Monero LEZ lock (tag 13, a pair: initialisation and funding) is
  planned under the Taker Node's LEZ lock gate, as a Bitcoin lock is, and only
  at a nonce the gate saw stand still since the last look (a Zcash lock, which
  is planned at the live nonce without the gate, moves it). Left open: a Zcash
  lock sent within the same LEZ block as the Monero lock is planned. If the
  Monero pair loses, nothing is locked and the swap ends
  `xmr_lez_lock_window_closed` at C (it is not planned again); if the Zcash
  lock loses, the Zcash actor plans it again. A Monero pair that dies gives the
  gate back at C, so later locks of either pair do not wait for it.
- The Taker's tag 14 (the claim authorization) is signed by its owner account
  without the gate. If it loses its nonce the claim never starts: the Maker
  does not claim, and from R the swap reads `xmr_maker_did_not_claim` and the
  Taker refunds; nothing is lost. If the other transaction loses: a Bitcoin LEZ
  lock is not planned again and that swap expires with nothing locked; a Zcash
  lock or revealing LEZ claim is planned again; whether a Bitcoin swap's LEZ
  claim is planned again was not checked (a claim that is not included reveals
  nothing on chain, and the swap can still refund).
- The Taker's tag 16 (the refund) is signed by the swap's own refund authority
  account, not the owner: it cannot collide with another swap's transaction.
- The Maker's tag 17 (the punishment) is signed by the Maker's owner account
  without coordination. The Maker Node's actor attempts run one at a time, so
  two owner transactions collide only if the second is prepared before the
  first is in a block. Until it is final the Node drives tag 17 again on every
  attempt, and the sidecar sends the current bytes again unless the node
  already includes them; it answers with the same bytes unless the node has
  not included them while the owner's nonce is above theirs (they can then
  never land), and then plans the same punishment at that nonce, kept beside
  the first in a re-plan slot of its own. Tag 17 has no upper window and
  nothing else moves the escrow after P, so it is re-planned for as long as
  its nonce is taken (the bound, 16,384, only keeps the lineage finite;
  `MAX_XMR_PUNISH_REPLANS`), never given up after a count. The escrow is
  punished once: a second tag 17 cannot execute, and the Maker finds the one
  that landed by its terms. Not run on the devnet: no scenario makes another
  owner transaction take tag 17's nonce.

**Scan distances.** Every finalized-LEZ scan on the Monero paths keeps a
durable cursor, so a scan of any distance ends however short its attempts
are (a LEZ block a second: on `local` up to about 2700 blocks from the
Taker's lock to P, more on a real network's longer windows):
- Maker actor (tags 13, 14, 16, in process): 8 pages of 16 blocks per drive
  (a page takes the swap's sidecar 0.4 to 5 s on the devnet, and the Node's
  one actor worker drives every swap in turn), cursor `maker-lez-scan.json`
  written at the end of each drive; a drive that stops short of the
  finalized tip concludes nothing absent. Tag 13
  starts at the finalized height the Maker Node read when it activated the
  swap (the Taker locks only once activated); without that seed (the sidecar
  did not answer then) 256 blocks before the first drive's finalized tip (a
  Maker Node that first drives such a swap more than 256 blocks after the
  Taker's lock misses it, never funds, and the swap ends refunded with
  nothing lost). Tags 14 and 16 start after the Taker's lock.
- Maker observer (tags 15, 17, `xmr-reference-tag15-finalized`): 16-block
  pages within the actor's 30 s observer timeout (about 800 blocks on the
  devnet), from the block after tag 14 (tag 15) or the refund scan's cursor
  (tag 17), resuming at `tag15-scan-cursor.json` / `tag17-scan-cursor.json`.
- Taker supervisor (tags 13, 14, 15, 16, in process): 8 windows of
  `discovery_max_blocks` (2048) per step, cursors in `xmr-taker-progress.json`.
  Tag 16 starts after the funding, about R − lock blocks before the refund can
  land (two windows on `local`).

Both desks publish, take and settle Monero swaps (`ui:xmr:xmr-happy`), refund
one the Maker funded and the Taker never claimed (`ui:xmr:xmr-taker-refund`: the
refund window open, then closing, the Refund button, the Maker's refund sweep) and
run two at once (`ui:xmr:xmr-concurrent`), recorded in
`docs/evidence/regression-m4-desks-2026-11-20/videos/`; the punish rows have not
been seen on a desk. Not on the devnet yet: there is no macOS
package for it. The Maker's systemd unit takes a Monero role from an optional
drop-in (`docs/maker-systemd-install.md`, "Monero"), checked statically and with
stub programs, not run in the VM; the Taker unit has none.

### A second stack on the same host

Lanes share one LEZ network. A second, independent stack (its own LEZ chain,
Bitcoin Core, Zebra, Nodes, heartbeat and lanes) runs beside the first under
another stack prefix, `LEZ_STACK_PREFIX`, from which `scripts/lane.sh` derives
every name, and a port offset, `LEZ_STACK_PORT_OFFSET`, added to every port the
stack and its lanes publish on 127.0.0.1:

| | default stack | `LEZ_STACK_PREFIX=lezb LEZ_STACK_PORT_OFFSET=1000` |
|---|---|---|
| Compose project, Docker network | `lez-swap-stack`, `lez-swap-chains` | `lezb-swap-stack`, `lezb-swap-chains` |
| LEZ network, sequencer, heartbeat | `lez-network`, `lez-sequencer`, `lez-heartbeat` | `lezb-network`, `lezb-sequencer`, `lezb-heartbeat` |
| explorers | `lez-btc-explorer` :3002, `lez-lez-explorer` :3003 | `lezb-btc-explorer` :4002, `lezb-lez-explorer` :4003 |
| lane 1 (the stack's pair and chains) | `lez-maker-node`, `lez-taker-node`, `lez-bitcoin-core`, `lez-btc-miner`, `lez-zebra` | `lezb-maker-node`, … `lezb-zebra` |
| lane k ≥ 2 | project `lez-lane-<k>`, containers `lez-l<k>-*` | project `lezb-lane-<k>`, containers `lezb-l<k>-*` |
| Bitcoin RPC, sequencer, indexer, Zebra | :18443, :3040, :8779, :18232 (+10 per lane) | :19443, :4040, :9779, :19232 (+10 per lane) |
| VNC (Maker, Taker) | :5901, :5902 (+10 per lane) | :6901, :6902 (+10 per lane) |
| named volumes | `$LEZ_VOLUME_PREFIX-*` (`lez-<stamp>` after `up.sh --fresh`) | `lezb-*` |

The second stack runs from a deploy directory of its own, so it has its own
`runtime/` (chain state, keys, Node files, regression results), and from a
market root of its own: a root's escrow deployment belongs to one stack's
chain. Its identities can be copies of the first root's.

```sh
git worktree add --detach ../lez-stack-b HEAD
mkdir -m 0700 ~/market-b && cp -a ~/market/identities ~/market-b/
cd ../lez-stack-b/deploy
LEZ_STACK_PREFIX=lezb LEZ_STACK_PORT_OFFSET=1000 LEZ_MARKET_ROOT=~/market-b \
  LEZ_PROVISION=~/provision/data LEZ_TIMING_PROFILE=fast \
  SKIP_UI_VERIFY=1 bash scripts/up.sh --fresh-lez
bash scripts/regress.sh run node:btc:happy:TakerSellsLez
bash scripts/down.sh --wipe       # its containers, volumes and runtime/ only
```

`gen-config.sh` records the prefix and the offset in that directory's
`runtime.env` (only when they are not the default, so the default stack's
`runtime.env` is unchanged) and marks the market root (`<root>/stack`); every
script run from that directory then acts on that stack without the variables,
a prefix the directory's `runtime.env` does not name is refused, and a stack
other than the default takes no market root that holds another chain's
deployment or another stack's mark. Both stacks use the same `lez-*:local`
images. Use an offset of 100 or more (lanes take +10 each) and check that the
shifted ports are free. `compose.testnet.yaml` keeps its own project
(`lez-testnet`) and container names.

### Two transports

The role launcher's `LEZ_TRANSPORT` (`modules/lez-role-node/node-entrypoint.sh`)
says how the Taker reaches the Maker. `local`, the default: offers and the
Maker's Delivery identity from the shared `delivery_dir` volume, Chat rounds
straight to the Maker's socket in the shared `maker_socket` volume, so both
Basecamps must be on one host. `logos` (`compose.logos.yaml`, test
`ui:btc:logos-happy:TakerSellsForeign`): nothing is shared but the chains. Each
role starts its Chat gateway (`lez-<role>-chat-gateway`, in its package) on
`LEZ_LOGOS_CHAT_GATEWAY_SOCKET` (under `/run/lez/<role>`), the socket its desk's
Logos Chat bridge uses; offers travel over Logos Delivery into the Taker desk's
order book and the take's Chat rounds over Logos Chat. The Taker is given the
Maker's public Delivery identity in `LEZ_MAKER_DELIVERY_IDENTITY`. The two
Delivery nodes form a network of their own, `LEZ_LOGOS_DELIVERY_CONFIG`
(delivery_module's createNode JSON, through the patched chat_module,
`builder/patches/logos-chat-module`): a private cluster id, the Maker's node
listening on TCP 60000 on every interface, the Taker's dialling
`LEZ_MAKER_DELIVERY_PEER` (`/ip4/<address>/tcp/60000/p2p/<peer id>`; the Delivery
node's resolver does not use Docker's DNS, so an address, not a name). Two
hosts set the same variables with the Maker's reachable address; NAT traversal
is not covered. On the devnet (2026-11-12) offers reached the Taker in about
3 s, every Chat round took 0.3–0.9 s and the six rounds of a take 3–4 s, in
six passing runs. A Zcash offer published from the Maker desk's form takes the
same path (`ui:zec:logos-happy:TakerSellsLez`): its take-time agreement is three
Chat rounds (`zec_reserve_v1`, propose, complete; 7.7, 10.5 and 4.0 KB to the
Maker, 4.0, 4.2 and 0.4 KB back), 0.3–0.8 s each, three of three runs. On the public `logos.test` fleet instead, of 13 Chat
messages 5 arrived in 1–4 s, 2 after 109 s and 258 s and 6 not within the 20 s
to 6 min watched, against the gateways' 25 s per round: no take completed.

### A Logos price module

A Maker route can take its price from a Logos price module, a shared library with the C API of
`crates/logos-price-c-api/include/lez_logos_price_api.h`, instead of the price saved with it. Set
`LEZ_LOGOS_PRICE_MODULE` to the module file and `LEZ_LOGOS_PRICE_MODULE_SHA256` to its sha256 (both
or neither; `compose.yaml` hands them to the Maker's Basecamp): the launcher starts the Node with the
package's `lez-logos-price-worker`, which loads the module in a child process on every quote and
publish. `lez-maker-cli configure-pair … --price-source logos-c-api` then selects it for a route.
Before every call the Node checks the module: an absolute path to a regular file (no symlink, one
hard link) owned by root or the Node's user, not group- or other-writable, at most 64 MiB, with the
pinned sha256, in a directory owned by root or the Node's user that is not group- or other-writable.
A module that fails a check, answers unavailable or reports a price older than 30 s fails the quote
or publish closed; offers already out keep their price. The systemd package installs the worker as
`/usr/bin/lez-logos-price-worker`; there the unit runs the same launcher, so the two
`LEZ_LOGOS_PRICE_MODULE*` values go in `/etc/lez/maker/env` and the launcher passes the flags. No Logos price module exists yet: `node:btc:maker-price-module:TakerSellsForeign` runs
the repository's TEST fixture (`tests/fixtures/logos_price_fixture.c`: 5 LEZ units per 2 sat). How to write and run a module: [docs/price-module-guide.md](../docs/price-module-guide.md).

## Services

| Service | Image | Notes |
|---|---|---|
| `bitcoin-core` | `images/bitcoin-core` | official Core 31.1 binaries (from the checksum/Guix-verified archive used by the repo's e2e flow), digest-pinned Chainguard `glibc-dynamic`, `txindex=1` + `txospenderindex=1` (the actors' lock observation needs the spender index), healthchecked, settlement-chain labels |
| `btc-miner` | `images/btc-miner` | regtest coinbase miner over JSON-RPC, `MINE_INTERVAL` from the timing profile (120 s on `local`, `LEZ_BTC_BLOCK_SECONDS` (2 s) on `fast` and `refund`); a scenario that needs a transaction unconfirmed stops it |
| `btc-explorer` (profile `explorers`) | `images/btc-explorer` | btc-rpc-explorer 3.5.1 from its GitHub tag commit (npm stops at 3.4.0) on Node 24, patched for an exhausted regtest halving schedule (post-era subsidy is 0 and the homepage halving card is skipped; upstream 500s the homepage) |
| `network` | `images/lez-network-module` | the LEZ v0.2 network in one headless Basecamp (the `basecamp-ui` image plus the indexer's r0vm): its LEZ Network desk starts `blockchain_module` 0.2.4 (the devnet's block producer; HTTP API on `:18080`, alias `bedrock`, state in `runtime/bedrock`) and `lez_indexer_module` (`indexer_service`'s JSON-RPC on `:8779`, alias `indexer`, published on loopback; the Basecamp user directory with its database in `runtime/indexer`). The desk's Re-index rebuilds the indexer's database while the node keeps running (`repair-indexer.sh`). Runs as the host user |
| `sequencer` | `images/lez-services` | `sequencer_service` from the pinned `logos-execution-zone` v0.2.4, native rebuild + arm64 r0vm; a plain service (there is no sequencer module); runs as the host user, state in `runtime/sequencer` |
| `zebra` | `images/zebra` | Zebra 7.0.0 on Regtest, NU7 from height 1 (digest-pinned upstream build in a distroless runtime), the ZEC pair's chain |
| `lez-explorer` (profile `explorers`) | `images/lez-explorer` | zero-dependency Node proxy + UI over the indexer RPC (`getBlocks/…/getAccount`); search resolves every exported swap's transaction hashes (`runtime/evidence/`) |
| `maker-node` | `images/maker-node` | the Maker's own Basecamp (the `basecamp-ui` image) on VNC `127.0.0.1:5901` with the Maker's Node package `lez_maker_node` installed in its user modules directory (`/var/lez/basecamp-maker-user/modules/lez_maker_node`; built by `scripts/package-role-node.sh`): the canonical Node, CLI, `lez-btc-maker-actor`, `lez-zec-maker-actor`, the LEZ v0.2 role sidecar program (spawned per swap), the health probe and the launcher `node-entrypoint.sh`, stripped; `/usr/local/bin/<tool>` link into the package. Nothing of the Node is copied into the image on its own. Its LEZ Maker desk opens at start, hosts the Maker's LEZ signer (`lez_core`), asks the package for its launcher (`LEZ_MAKER_NODE_LAUNCHER=package`) and has `lez_swap_node` launch and supervise the Node from it; the UI tests drive the same desk (`ui-tests/`, mounted). The launcher forwards loopback 18443/3040/8779 to Core, sequencer and indexer because the actor, wallet client and sidecars accept only literal-loopback endpoints. The image owns its state directories (a new named volume takes the image directory's owner and mode), so no init container runs |
| `taker-node` | `images/taker-node` | the Taker's own Basecamp on VNC `127.0.0.1:5902`, as `maker-node`, with the Taker's package `lez_taker_node`: canonical Node, CLI, registry initializer, `lez-btc-taker-actor`, `lez-zec-taker-actor` (supervised by the Node itself: activation, the LEZ lock, the observation of the Maker's legs; its schedule lives in `taker-actor-schedule.sqlite3` beside the registry), the role sidecar, the health probe and the launcher; same loopback forwarders as the Maker. Beside the package: `zec-local-poc-identity` (gen-config.sh derives zebra's miner address with it) and, in the devnet's image only, the dealer test fixture's tools (`zec-local-poc-provision`, `-chat-draft`, `-chat-finalize`, `-transfer`: `executables.json` class `fixture`, cargo feature `dealer-fixture`), run by the catalog tests' `tests/fixtures/zec-catalog-fixture.sh` and to fund the devnet Taker's Zcash key; the published image does not carry them |
| `heartbeat` (profile `heartbeat`) | `images/lez-wallet-module` | the LEZ finality heartbeat (above); `up.sh` starts it when `LEZ_HEARTBEAT_SECONDS` is not `0` |
| `tools` (profile `tools`) | `images/lez-wallet-module` | one-shot only (`docker compose --profile tools run --rm tools '…'`): the escrow deployer, the vault claims through the LEZ wallet module (`lez-wallet-vault-claim`) and the identity tool for `start.sh`'s bootstrap; runs as the host user with the market root mounted, never as a service |
| `basecamp-ui` (build only, `scale: 0`) | `images/basecamp-ui` | the base image of every Basecamp in the stack (`maker-node`, `taker-node`, `network`, through their `basecamp` build context); runs no container. Portable Basecamp 0.2.0-RC3 **inspector twin** + the release packages installed (desks, runtime modules) + qt-mcp; Xvfb/fluxbox/x11vnc |

Each desk reaches its own Node over the Node's owner socket inside the same
container; the desks read their market from the Nodes
(`apps/basecamp/common/node_market.cpp`): offers, takes, the Taker's lock and
claim, and the Maker's automatic progress. The C++ backends enforce
`uid == socket owner && mode 0600`; Basecamp and the Node share the uid 4713.
The only socket two containers share is the Maker's Chat socket
(`maker_socket`, mounted in the Taker's Basecamp for its Node).

The Bitcoin Core and Zebra data volumes take their owner (the images' nonroot
uid) from the images' own data directories on their first mount, and the LEZ
services' bind-mounted state belongs to the host user they run as, so no init
container chowns either.

All services: `restart: unless-stopped`, log rotation, most are `read_only` +
`cap_drop: ALL` + `no-new-privileges`; ports published on loopback only. No
service holds the host Docker socket.

That log rotation covers each container's own output (three files of 200 MB). It does
not cover the log files Basecamp writes inside a Node container, under
`/var/lez/basecamp-<role>-user/logs/`: Basecamp starts a new file every 10,000 lines and
never removes one, and has no setting for it. The Delivery module prints every message it
receives there (one offer announcement is one line of about 23 KB, and every Maker
announces each offer every 10 seconds), so a Node's log directory grew by about 11 MB an
hour idle and 70 to 96 MB an hour during tests. The files are in the container's writable
layer and go when the container is recreated (`up.sh`, a profile switch). A Node kept
running for weeks needs its old `basecamp_*.log` files removed from outside.

## Native-arm64 notes

- LEZ services, r0vm (risc0 v3.0.5 tag), maker-node binaries, and bitcoind are
  aarch64 — rebuilt from the exact upstream pins (the repo's evidence pins are
  x86_64; the Logos rapidsnark fork's `rapidsnark-linux-aarch64-pic-v0.0.8`
  release provides the arm64 prover libs).
- `blockchain_module` is the released multi-arch lgx (sha256-pinned in
  `from-scratch.sh`); its `linux-arm64` variant runs here.
- Basecamp + role packages were built with the module-builder flake for
  aarch64-linux; the `bin-bundle-dir-inspector` output is self-contained.
- The desks, Chat and Delivery are built as release packages (`lgx-portable`,
  variant `linux-arm64`/`linux-amd64`), the same files `logosctl` installs; the
  stage phase pins our packages' dependencies
  (`scripts/pin-package-dependencies.sh`, the set in
  `images/basecamp-ui/assets/packages/`) and installs the images from them.
  Nothing is renamed.

## Layout

```
compose.yaml               the stack (image names: ${LEZ_IMAGE_PREFIX}-<service>:${LEZ_IMAGE_TAG})
scripts/start.sh           prebuilt path: pull the release images, identities, stack, bootstrap, desk suites
scripts/package-dist.sh    assemble the release bundle (deploy/ from git archive + release.env)
scripts/from-scratch.sh    everything from an empty host (prerequisites, sources, payloads, runner, stack)
scripts/up.sh              one-command bring-up (+ desk verification); LEZ_IMAGES=pull skips the build
scripts/swap-through-ui.sh one complete swap driven through the two desks
scripts/node-hold.sh       hold a Node down inside its running Basecamp, or release it
scripts/market-bootstrap.sh one-time settlement-chain bootstrap (one throwaway builder container)
scripts/pin-package-dependencies.sh write the release set: our packages' dependencies pinned, each once
scripts/package-chat-module.sh the catalog's chat_module: Logos's 0.2.2 release with Delivery pinned to 0.2.x
scripts/stage-basecamp-package.sh install one release package (desk or module) into the Basecamp image
scripts/export-node-evidence.py publish a completed swap's public evidence from the Nodes
scripts/reset-swaps.sh     forget every persisted swap on both Nodes (after an actor rebuild)
scripts/verify-all.sh      containers, chains, explorers, Node market, desk suites
scripts/down.sh            stop / --wipe
scripts/gen-config.sh      renders runtime/ (LEZ configs, bitcoin.conf, secrets) — idempotent
scripts/btc-miner.sh       regtest mining loop
scripts/regress.sh         the devnet regression (smoke, refunds, full) that survives finality freezes
scripts/lane.sh, lanes.sh  regression lanes 2..N beside the stack (compose.lane.yaml)
scripts/repair-indexer.sh  rebuild the indexer's database while the LEZ node keeps running
scripts/logos-native-acceptance.sh  the packages through logosctl, outside Compose (docs/evidence/logos-native-2026-11-10)
compose.lane.yaml          overlay for one regression lane
compose.testnet.yaml       overlay for the public testnets (docs/testnet-run.md)
images/                    one dir per image (Dockerfile + payloads)
  basecamp-ui/             the Basecamp every role and the network run in (build only)
  lez-network-module/      the network Basecamp: blockchain_module + lez_indexer_module
  basecamp-ui/assets/      portable Basecamp bundle, role trees, qt-mcp framework
  lez-wallet-module/       the LEZ wallet as the Logos lez_core module: vault claims, heartbeat,
                           the roles' swap signers; also the escrow deployer and identity tool (profile "tools")
assets/lez-source/         pinned v0.2.0 config templates (bedrock, sequencer, indexer)
assets/certified-evidence-m5arm-08180005-ui.json  explorer evidence seed until the first Node swap
builder/Dockerfile         the ephemeral builder image (docker run --rm only; not on the stack)
ui-tests/verify.mjs        end-to-end UI test (maker + taker) via the QML inspector
  runtime/                   generated state (gitignored; wiped by --wipe)
    evidence/                one exported document per completed swap
```

The workspace next to this repository holds the pinned `lez-source` checkout,
`provision/data` (built artifacts) and the market root; `runtime/runtime.env`
records `LEZ_MARKET_ROOT` for provisioning. The Nodes mount the public
`runtime/market-bootstrap.env` copy and their individual staged identities;
they do not mount the private market root.

## Node-owned Bitcoin swaps (ADR 0213)

Both Nodes settle a BTC↔LEZ swap themselves: no runner, no demo controller
on the stack.
The role package's launcher (`modules/lez-role-node/node-entrypoint.sh`,
installed as `lez_<role>_node/bin/node-entrypoint.sh`) renders each role's
inputs into its state volume — the
LEZ identity (`runtime/lez/<role>`, the Maker is wallet `maker-munich-01`, the
Taker `taker-zurich-01`; beside it `zcash.key`, the role's Zcash key that
`gen-config.sh` mints once — the Maker's is also its Delivery signing key and
the address zebra mines to, rendered into `runtime/zebra/zebrad.toml`; the
Taker's funds Zcash when it sells it), the Bitcoin RPC cookie and, once
`market-bootstrap.sh` has recorded the escrow deployment in
`runner-work/market/market-bootstrap.env`, the `btc-role.json` that names the
Bitcoin network, endpoints, wallet, policy, LEZ chain identity and the sidecar
program. Chains are chosen only by those inputs.

By default a role's Bitcoin claims and refunds pay a key minted for that swap
alone, so Core cannot see or spend the proceeds and each payout is unlinkable
from the last. Set `LEZ_BTC_CLAIM_DESTINATION` to an address of the role's own
wallet to pay there instead, which is how the LEZ leg has always behaved, or to
`wallet` to take that address from `LEZ_BTC_WALLET` on first start and keep it.
The desk stack does this, so a completed swap tops up the balance the desk
shows. The two roles must not share an address: the agreement rejects a pair
whose participants name the same claim destination. Each Node spawns one LEZ role
sidecar per swap (own loopback port, capability, state directory and log under
the swap directory; run id derived from the reservation id) because a sidecar
holds one durable escrow and one claim reservation per state directory. The
Maker runs with `--chat-socket`, `--btc-role-config` and `--actor-supervisor`;
the Taker's `role.json` points at the Maker's Chat socket and the same role
config.

```
scripts/node-swap.sh            # publish an offer, take it, lock BTC, wait for both actors
scripts/node-swap.sh --no-wait  # stop after the Taker's lock
```

The Taker's `taker_swap_initiate_v1` performs the reservation
(`btc_reserve_v1`), plans its Bitcoin funding from Core wallet `lez-taker`,
prepares its LEZ claim through its sidecar, composes the draft, negotiates
(`btc_chat_propose_v2` / `btc_chat_complete_v2`), runs the three ceremony
rounds and activates its actor; `taker_swap_lock_v1` broadcasts the exact
funding transaction. The Maker's supervisor drives its actor (LEZ funding,
Bitcoin claim) as the actor's status calls for it; the Taker Node's observer
drives its actor's chain observations and, after `taker_swap_claim_v1`, the
claim's follow-up, so `taker_swap_monitor_v1` reaches `completed` without
the runner. `maker_actor_monitor_v1` and `taker_swap_monitor_v1` show
progress.

Bitcoin Core wallets: `lez-taker`, `lez-maker` (descriptor wallets) and
`lez-miner`, which imports the well-known regtest mining key (the scalar 1,
rendered by `gen-config.sh` into `runtime/secrets/mining.key`) so the mined
coins fund the Taker; regtest's subsidy is exhausted past height 11k, so
mining to a fresh address yields nothing. An aborted take leaves its swap
directory and sidecar behind; the next take is a new reservation with its own
sidecar. Persisted swaps pin the actor program's hash, so after the actor
binaries are rebuilt they list as `attention_required`; `scripts/reset-swaps.sh`
forgets every persisted swap on both Nodes (directories, registry and store
rows, reserved and consumed offers, exported evidence) and leaves chains,
wallets, identities and the stored route terms alone. It refuses while a swap
may still have funds locked (a Taker swap not `completed` or `refunded`, a Maker
actor not `terminal`, or a Taker it cannot ask) unless `--abandon-open` is given;
then the Nodes no longer drive those swaps, and their per-swap directories (the
only copy of a Bitcoin refund key) and store databases are moved to
`/var/lib/lez/<role>/.reset-archive/<time>/` in the role's state volume instead
of being deleted. The test runners (`regress.sh`, `ui-e2e.sh`, `up.sh
--fresh-lez`) pass the flag.

## Demo boundary

The checked component inventory and exact support claims live in
`profiles/local-btc-demo-v1.json`; every Cargo executable is classified in
`executables.json`. Validate both with `../scripts/check-runtime-profiles.py`.

| State | Owner | Backup class |
|---|---|---|
| `lez-maker-state` | Maker Node | Critical: DB and Delivery signing identity |
| `lez-taker-state` | Taker Node | Critical after admission |
| `lez-bitcoin-core-data` | Bitcoin Core | Critical for retained local chain evidence |
| `runtime/{bedrock,indexer,sequencer}` | LEZ devnet (`network`: node and Basecamp user directory with the indexer's database; `sequencer`) | Recreatable before funding; preserve with retained chain evidence |
| market root (`LEZ_MARKET_ROOT`) | Market bootstrap | Critical: wallet identities and the escrow deployment manifest |
| `runtime/evidence` | `export-node-evidence.py` | Recreatable from the Nodes' swap directories |

The two Basecamp desks drive the Nodes' fixed, genuine LEZ/BTC lifecycle and
present its completed evidence. They do not yet expose arbitrary amounts,
external wallets, public networks, or production signing; each Node settles as
one configured identity. Both Nodes listen only on owner-only Unix sockets; no
component on the stack holds the host Docker socket. Devnet-only caveat: the
role keys under `runtime/lez/<role>/` (`lez-signer.key`, `zcash.key`) are
host copies the Node reads through the bind mount: on Linux at mode 0640 in a
0750 directory, with the role container carrying the host user's group
(`group_add`, `LEZ_HOST_GID`); on macOS, where Docker Desktop shares files under
its own rules, at 0644. Each entrypoint re-installs them owner-private in its
state volume.
