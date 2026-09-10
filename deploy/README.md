# lez-swap-stack — fully dockerized local LEZ ↔ BTC swap environment

One compose project: LEZ v0.2 devnet + Bitcoin Core regtest + explorers for
both chains + the canonical Maker/Taker Nodes + the real Basecamp UI
(drivable over VNC and by automated end-to-end tests). All native arm64,
started with one command.

## Prebuilt images (no build)

Every release attaches `lez-swap-stack-<tag>-arm64.tar.gz`: the compose
file, these scripts and `release.env` naming the linux/arm64 images the
release workflow pushed to `ghcr.io/gateway-fm/lez-atomic-swaps/lez-*`.
On an arm64 host with Docker (Apple silicon, or arm64 Linux):

```sh
tar -xzf lez-swap-stack-v0.2.1-arm64.tar.gz && cd lez-swap-stack-v0.2.1-arm64
./scripts/start.sh            # pull → wallet identities → config → stack → market bootstrap → UI suites
./scripts/start.sh --swap     # …and one full BTC → LEZ swap through the two Basecamp apps
```

`start.sh` mints the four wallet identities into `market/` with the
`lez-tools` image, brings the stack up through `up.sh` in pull mode
(`LEZ_IMAGES=pull`), runs the idempotent market bootstrap in one throwaway
tools container, restarts both Nodes on the recorded escrow deployment and
runs the Basecamp suites. Rerunning it resumes; `./scripts/down.sh` stops the
stack and `--wipe` removes chains and Node state (keep `market/`, it holds the
funded wallets). The same script works from a checkout of the tagged commit
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
Docker) from nothing to the running stack with the Basecamp UI verified:

```sh
./scripts/from-scratch.sh          # prerequisites → pinned sources → payloads (ephemeral builders) → stack → UI suites
./scripts/from-scratch.sh --swap   # …and one full BTC → LEZ swap through the two Basecamp apps
```

It installs the host tools (Homebrew on macOS), clones the pinned
`logos-execution-zone` and `logos-basecamp` next to this repository, builds the
Basecamp bundle, both role packages and the Chat/Delivery modules in the pinned
Nix image, builds the Node binaries and Bitcoin actors in the pinned Rust
image, builds the LEZ services, r0vm, rapidsnark, the escrow artifact, the LEZ
sidecar and the four wallet identities in throwaway containers of the builder
image (`builder/`, caches in `lez-build-*` volumes, outputs in
`provision/data`), stages every image payload, then runs `up.sh`, the market
bootstrap (one throwaway container on the stack network), both UI suites and
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
./scripts/swap-through-ui.sh       # one BTC → LEZ swap through the two Basecamp apps
./scripts/export-node-evidence.py  # publish a completed swap's public evidence (explorer + proof view)
./scripts/down.sh                  # stop everything (--wipe also removes state and volumes)
```

`up.sh` ends with the repo-style UI verification (real Basecamp driven through
its QML inspector against the live Maker and Taker Nodes). Skip with `SKIP_UI_VERIFY=1`.

| What | Where |
|---|---|
| Bitcoin regtest RPC | `http://127.0.0.1:18443` (user `lezrpc`, password in `runtime/runtime.env`), auto-mining every 120 s |
| BTC explorer | http://127.0.0.1:3002 |
| LEZ explorer + M3 evidence | http://127.0.0.1:3003/#/evidence |
| **Basecamp UI (VNC)** | **`vnc://127.0.0.1:5901`** (password `lezswap`; override with `VNC_PASSWORD`) |
| Maker Node | `docker exec lez-maker-node lez-maker-cli --socket /run/lez/maker/node.sock health` |
| UI verify | `docker compose --env-file runtime/runtime.env run --rm --no-deps --entrypoint node basecamp-ui /ui-tests/verify.mjs [maker\|taker]` |
| Switch UI role | `BASECAMP_ROLE=taker docker compose --env-file runtime/runtime.env up -d basecamp-ui` |
| Logs | `docker compose --env-file runtime/runtime.env logs -f <service>` |

Open the VNC URL; the two Basecamp microapps are the control surface and the
two Nodes settle the swap:

1. Open **LEZ / BTC Maker** (wallet **Munich Vault 01**, the Maker Node's
   identity) and click **Publish offer**. Each click publishes one 0.01 BTC →
   1,000 LEZ offer through the Maker Node; withdrawn and consumed offers leave
   the Taker's order book.
2. Open **LEZ / BTC Taker** (wallet **Zurich Wallet 01**, the Taker Node's
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
   clean run.
2. **Publish.** Connect VNC to `127.0.0.1:5901` (password `lezswap`), open
   **LEZ / BTC Maker** and click **Publish offer**. Check it from outside:
   `maker maker_offer_list` lists it `active` at revision 1, and
   `docker exec lez-taker-node ls /delivery` shows its signed file.
3. **Discover and take.** Open **LEZ / BTC Taker**; the order book shows the
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
   (15 checks) and both UI suites.

Things worth trying deliberately: withdraw an offer on the Maker desk and
watch it leave the Taker order book; `docker restart lez-taker-node` between
lock and claim and watch the observer resume; `docker restart
lez-bitcoin-core` and watch the next action still reach Core through the
Node's own loopback forwarder. Where to look when something stalls:
`docker compose logs maker-node taker-node`, the swap directory's
`sidecar.log`, and
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

The refund scenarios need the `fast` timing profile:

```sh
LEZ_TIMING_PROFILE=fast ./scripts/gen-config.sh runtime     # 6 CSV blocks; 600/900/1200 s deadlines, 60 s margin; 120-block LEZ window
docker compose --env-file runtime/runtime.env up -d --no-deps --force-recreate maker-node taker-node
./scripts/node-e2e.py taker-refund
```

`local` (the default) keeps network-like deadlines: 144 CSV blocks, a 30-minute
Maker cutoff, 60- and 120-minute refund bounds, a 360-block LEZ discovery
window. The profile is configuration only (`LEZ_BTC_*` and
`LEZ_LEZ_DISCOVERY_MAX_BLOCKS` in `runtime.env`, rendered into each Node's
`btc-role.json`); the protocol accepts any values with a positive margin and
`later >= earlier + margin`, and Bitcoin refund maturity is a block count, so
regtest mines past it.

The LEZ discovery window sets how soon a refund can start when the Maker never
locks: the Taker asserts the Maker lock's absence only once every block of the
window is finalized, so a window of 120 blocks at the devnet's 10-second slots
means the refund becomes eligible about 20 minutes after the take. The window
must cover the Maker cutoff (60 blocks on the fast profile) with room to spare.

## Settlement chains

Swaps settle on one permanent pair of chains, the way they must against real
Bitcoin and LEZ networks — chain lifecycle is not the swap's job:

* **Bitcoin**: the standing `bitcoin-core` regtest chain (`txindex`,
  `txospenderindex`), continuously mined by `btc-miner`. Swap transactions are
  native chain transactions, visible in the Bitcoin explorer.
* **LEZ**: the standing `bedrock` / `sequencer` / `indexer` chain, whose genesis
  funds four persistent wallets (Munich, Basel, Zurich, Limmat). Balances
  accumulate across swaps; the UI's wallet ledger shows true opening → closing
  values, not per-run genesis allocations.

Both carry `org.logos-co.atomic-swaps.{run,scope,component}` labels
(`market-btc-0001` / `market-lez-0001`). The one-time bootstrap — escrow
program deployment and the four wallet vault claims — is idempotent:

```sh
./scripts/from-scratch.sh --only stack    # runs it inside one throwaway builder container
```

The bootstrap runs `scripts/market-bootstrap.sh` in one throwaway container on
the stack's network with the escrow deployer and the vault-claim tool: the
`lez-builder:local` image with the tools from `provision/data` on a developer
host (`from-scratch.sh`), or the published `lez-tools` image (compose profile
`tools`, `start.sh`). Both tools accept only literal-loopback URLs, so the
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
| UI regressions | the two Basecamp suites against the live Maker and Taker Nodes |

The BTC view can be driven automatically:

```sh
docker compose --env-file runtime/runtime.env run --rm --no-deps --entrypoint node \
  basecamp-ui /ui-tests/verify.mjs taker
```

For a completed reverse-flow evidence file, select its exact UI contract while
running either role suite:

```sh
M3_UI_DIRECTION=TakerSellsLez docker compose --env-file runtime/runtime.env \
  run --rm --no-deps --entrypoint node basecamp-ui /ui-tests/verify.mjs taker
```

## Services

| Service | Image | Notes |
|---|---|---|
| `bitcoin-core` | `images/bitcoin-core` | official Core 31.1 binaries (from the checksum/Guix-verified archive used by the repo's e2e flow), digest-pinned Chainguard `glibc-dynamic`, `txindex=1` + `txospenderindex=1` (the actors' lock observation needs the spender index), healthchecked, settlement-chain labels |
| `bitcoin-init` | debian | one-shot datadir chown to the nonroot runtime uid |
| `btc-miner` | `images/btc-miner` | regtest coinbase miner over JSON-RPC, `MINE_INTERVAL` 120s so ambient blocks do not race swap confirmations |
| `btc-explorer` | `images/btc-explorer` | btc-rpc-explorer 3.4.0, patched for an exhausted regtest halving schedule (post-era subsidy is 0; upstream returns `undefined` and 500s the homepage) |
| `bedrock` | pinned multi-arch digest `91d6c5…` | LEZ v0.2 consensus node (exact pin from the repo's compose) |
| `sequencer` / `indexer` | `images/lez-services` | built from pinned `logos-execution-zone` v0.2.0 (a58fbce), native rebuild + arm64 r0vm |
| `lez-explorer` | `images/lez-explorer` | zero-dependency Node proxy + UI over the indexer RPC (`getBlocks/…/getAccount`); search resolves every exported swap's transaction hashes (`runtime/evidence/`) |
| `maker-init` / `taker-init` | debian | one-shot volume chowns; Taker reads only `maker-delivery-identity.pub`, never Maker private state |
| `maker-node` | `images/maker-node` | Maker-only image (payloads stripped at build): canonical Node, CLI, Chat gateway, `lez-btc-maker-actor`, the LEZ v0.2 role sidecar program (spawned per swap) and `node-entrypoint.sh`; the entrypoint forwards loopback 18443/3040/8779 to Core, sequencer and indexer because the actor, wallet client and sidecars accept only literal-loopback endpoints |
| `taker-node` | `images/taker-node` | Taker-only image: canonical Node, CLI, Chat gateway, registry initializer, `lez-btc-taker-actor`, the role sidecar and `node-entrypoint.sh`; same loopback forwarders as the Maker |
| `tools` (profile `tools`) | `images/lez-tools` | one-shot only (`docker compose --profile tools run --rm tools '…'`): the escrow deployer, the vault-claim tool and the identity tool for `start.sh`'s bootstrap; runs as the host user with the market root mounted, never as a service |
| `basecamp-ui` | `images/basecamp-ui` | portable Basecamp 0.2.0-RC3 **inspector twin** + role install trees + qt-mcp; Xvfb/fluxbox/x11vnc; runs as the Node uid (4713) so the owner-only socket checks pass. Both desks read their market from the Nodes (`apps/basecamp/common/node_market.cpp`): offers, takes, the Taker's lock and claim, and the Maker's automatic progress |

The UI reaches the role Nodes through shared named socket volumes mounted
read-only in Basecamp. The C++ backends enforce
`uid == socket owner && mode 0600`, hence the shared uid.

All services: `restart: unless-stopped`, log rotation, most are `read_only` +
`cap_drop: ALL` + `no-new-privileges`; ports published on loopback only. No
service holds the host Docker socket.

## Native-arm64 notes

- LEZ services, r0vm (risc0 v3.0.5 tag), maker-node binaries, and bitcoind are
  aarch64 — rebuilt from the exact upstream pins (the repo's evidence pins are
  x86_64; the Logos rapidsnark fork's `rapidsnark-linux-aarch64-pic-v0.0.8`
  release provides the arm64 prover libs).
- The bedrock image digest is upstream multi-arch and resolves to arm64 here.
- Basecamp + role packages were built with the module-builder flake for
  aarch64-linux; the `bin-bundle-dir-inspector` output is self-contained.
- The `linux-arm64-dev` variant tags in the locally built role packages were
  renamed to `linux-arm64` (variant file + manifest keys) so the release
  Basecamp accepts them.

## Layout

```
compose.yaml               the stack (image names: ${LEZ_IMAGE_PREFIX}-<service>:${LEZ_IMAGE_TAG})
scripts/start.sh           prebuilt path: pull the release images, identities, stack, bootstrap, UI suites
scripts/package-dist.sh    assemble the release bundle (deploy/ from git archive + release.env)
scripts/from-scratch.sh    everything from an empty host (prerequisites, sources, payloads, runner, stack)
scripts/up.sh              one-command bring-up (+ UI verification); LEZ_IMAGES=pull skips the build
scripts/swap-through-ui.sh one complete swap driven through the two Basecamp apps
scripts/market-bootstrap.sh one-time settlement-chain bootstrap (one throwaway builder container)
scripts/stage-basecamp-package.sh stage one Nix-built package or module into the UI image
scripts/export-node-evidence.py publish a completed swap's public evidence from the Nodes
scripts/reset-swaps.sh     forget every persisted swap on both Nodes (after an actor rebuild)
scripts/verify-all.sh      containers, chains, explorers, Node market, UI suites
scripts/down.sh            stop / --wipe
scripts/gen-config.sh      renders runtime/ (LEZ configs, bitcoin.conf, secrets) — idempotent
scripts/btc-miner.sh       regtest mining loop
images/                    one dir per image (Dockerfile + payloads)
  basecamp-ui/assets/      portable Basecamp bundle, role trees, qt-mcp framework
  lez-tools/               escrow deployer, vault-claim and identity tools (profile "tools")
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
`node-entrypoint.sh` renders each role's inputs into its state volume — the
LEZ identity (`runtime/lez/<role>`, the Maker is wallet `maker-munich-01`, the
Taker `taker-zurich-01`), the Bitcoin RPC cookie and, once
`market-bootstrap.sh` has recorded the escrow deployment in
`runner-work/market/market-bootstrap.env`, the `btc-role.json` that names the
Bitcoin network, endpoints, wallet, policy, LEZ chain identity and the sidecar
program. Chains are chosen only by those inputs. Each Node spawns one LEZ role
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
wallets, identities and the stored route terms alone.

## Demo boundary

The checked component inventory and exact support claims live in
`profiles/local-btc-demo-v1.json`; every Cargo executable is classified in
`executables.json`. Validate both with `../scripts/check-runtime-profiles.py`.

| State | Owner | Backup class |
|---|---|---|
| `lez-maker-state` | Maker Node | Critical: DB and Delivery signing identity |
| `lez-taker-state` | Taker Node | Critical after admission |
| `lez-bitcoin-core-data` | Bitcoin Core | Critical for retained local chain evidence |
| `runtime/{bedrock,indexer,sequencer}` | LEZ devnet | Recreatable before funding; preserve with retained chain evidence |
| market root (`LEZ_MARKET_ROOT`) | Market bootstrap | Critical: wallet identities and the escrow deployment manifest |
| `runtime/evidence` | `export-node-evidence.py` | Recreatable from the Nodes' swap directories |

The two Basecamp desks drive the Nodes' fixed, genuine LEZ/BTC lifecycle and
present its completed evidence. They do not yet expose arbitrary amounts,
external wallets, public networks, or production signing; each Node settles as
one configured identity. Both Nodes listen only on owner-only Unix sockets; no
component on the stack holds the host Docker socket.
