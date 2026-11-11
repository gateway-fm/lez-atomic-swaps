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
tar -xzf lez-swap-stack-v0.2.5-arm64.tar.gz && cd lez-swap-stack-v0.2.5-arm64
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
release is published (never on pushes or pull requests), from the very phases
`from-scratch.sh` runs below, on GitHub's arm64 runners; the bundle is
attached to that release only after a fresh runner has started the stack from
it (ADR 0215). Publish the release with its notes first; the assets follow
about 2.5 hours later. `scripts/package-dist.sh <prefix> <tag>` produces the bundle
locally from a committed tree.

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
| `concurrent` | two swaps interleaved end to end (one sidecar per swap) |
| `taker-refund` | the Maker never funds; after the cutoff and CSV maturity the Taker refunds its Bitcoin; the Maker reconciles |
| `maker-refund` | the Taker never claims; the Maker refunds LEZ after `refund_at`, then the Taker refunds Bitcoin |

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
them all. The scenarios, `reset-swaps.sh` and `zec-provision.sh` act on one lane
when `LEZ_LANE=<k>` is set: `LEZ_LANE=2 bash scripts/ui-e2e.sh happy`.

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

## Services

| Service | Image | Notes |
|---|---|---|
| `bitcoin-core` | `images/bitcoin-core` | official Core 31.1 binaries (from the checksum/Guix-verified archive used by the repo's e2e flow), digest-pinned Chainguard `glibc-dynamic`, `txindex=1` + `txospenderindex=1` (the actors' lock observation needs the spender index), healthchecked, settlement-chain labels |
| `btc-miner` | `images/btc-miner` | regtest coinbase miner over JSON-RPC, `MINE_INTERVAL` from the timing profile (120 s on `local`, `LEZ_BTC_BLOCK_SECONDS` (2 s) on `fast` and `refund`); a scenario that needs a transaction unconfirmed stops it |
| `btc-explorer` (profile `explorers`) | `images/btc-explorer` | btc-rpc-explorer 3.4.0, patched for an exhausted regtest halving schedule (post-era subsidy is 0; upstream returns `undefined` and 500s the homepage) |
| `network` | `images/lez-network-module` | the LEZ v0.2 network in one headless Basecamp (the `basecamp-ui` image plus the indexer's r0vm): its LEZ Network desk starts `blockchain_module` 0.2.4 (the devnet's block producer; HTTP API on `:18080`, alias `bedrock`, state in `runtime/bedrock`) and `lez_indexer_module` (`indexer_service`'s JSON-RPC on `:8779`, alias `indexer`, published on loopback; the Basecamp user directory with its database in `runtime/indexer`). The desk's Re-index rebuilds the indexer's database while the node keeps running (`repair-indexer.sh`). Runs as the host user |
| `sequencer` | `images/lez-services` | `sequencer_service` from the pinned `logos-execution-zone` v0.2.4, native rebuild + arm64 r0vm; a plain service (there is no sequencer module); runs as the host user, state in `runtime/sequencer` |
| `zebra` | `images/zebra` | Zebra 7.0.0 on Regtest, NU7 from height 1 (digest-pinned upstream build in a distroless runtime), the ZEC pair's chain |
| `lez-explorer` (profile `explorers`) | `images/lez-explorer` | zero-dependency Node proxy + UI over the indexer RPC (`getBlocks/…/getAccount`); search resolves every exported swap's transaction hashes (`runtime/evidence/`) |
| `maker-node` | `images/maker-node` | the Maker's own Basecamp (the `basecamp-ui` image) on VNC `127.0.0.1:5901` with the Maker's Node package `lez_maker_node` installed in its user modules directory (`/var/lez/basecamp-maker-user/modules/lez_maker_node`; built by `scripts/package-role-node.sh`): the canonical Node, CLI, `lez-btc-maker-actor`, `lez-zec-maker-actor`, the LEZ v0.2 role sidecar program (spawned per swap), the health probe and the launcher `node-entrypoint.sh`, stripped; `/usr/local/bin/<tool>` link into the package. Nothing of the Node is copied into the image on its own. Its LEZ Maker desk opens at start, hosts the Maker's LEZ signer (`lez_core`), asks the package for its launcher (`LEZ_MAKER_NODE_LAUNCHER=package`) and has `lez_swap_node` launch and supervise the Node from it; the UI tests drive the same desk (`ui-tests/`, mounted). The launcher forwards loopback 18443/3040/8779 to Core, sequencer and indexer because the actor, wallet client and sidecars accept only literal-loopback endpoints. The image owns its state directories (a new named volume takes the image directory's owner and mode), so no init container runs |
| `taker-node` | `images/taker-node` | the Taker's own Basecamp on VNC `127.0.0.1:5902`, as `maker-node`, with the Taker's package `lez_taker_node`: canonical Node, CLI, registry initializer, `lez-btc-taker-actor`, `lez-zec-taker-actor` (supervised by the Node itself: activation, the LEZ lock, the observation of the Maker's legs; its schedule lives in `taker-actor-schedule.sqlite3` beside the registry), the role sidecar, the health probe and the launcher; same loopback forwarders as the Maker. The ZEC corridor provisioners (`zec-local-poc-*`, run by `scripts/zec-provision.sh`) are image tools, not part of the package |
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
scripts/pin-package-dependencies.sh write the release set: our packages' dependencies with version ranges
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
