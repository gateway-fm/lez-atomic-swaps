# lez-swap-stack — fully dockerized local LEZ ↔ BTC swap environment

One compose project: LEZ v0.2 devnet + Bitcoin Core regtest + explorers for
both chains + the canonical Maker/Taker Nodes + the real Basecamp UI
(drivable over VNC and by automated end-to-end tests). All native arm64,
started with one command.

## From scratch

One command takes an arm64 host (macOS with Docker Desktop, or Linux with
Docker) from nothing to the running stack with the Basecamp UI verified:

```sh
./scripts/from-scratch.sh          # prerequisites → pinned sources → payloads → runner → stack → UI suites
./scripts/from-scratch.sh --swap   # …and one full BTC → LEZ swap through the two Basecamp apps
```

It installs the host tools (Homebrew on macOS), clones the pinned
`logos-execution-zone` and `logos-basecamp` next to this repository, builds the
Basecamp bundle, both role packages and the Chat/Delivery modules in the pinned
Nix image, builds the Node binaries in the pinned Rust image, provisions the
`lez-runner-arm` container (LEZ services, r0vm, rapidsnark, the escrow
artifact, warm cargo caches, the four wallet identities), stages every image
payload, then runs `up.sh`, the market bootstrap, both UI suites and
`verify-all.sh`. Every phase is idempotent and resumable (`--only <phase>`).
The cold path builds several Rust toolchains' worth of code and takes a few
hours on Apple silicon; a rerun takes minutes.

Two macOS specifics. If Docker Desktop keeps a Docker Hub login in the
Keychain (`credsStore: "desktop"` in `~/.docker/config.json`), every pull and
every BuildKit `FROM` lookup asks the Keychain, and macOS raises a prompt in
the terminal's name; unattended runs then hang or fail with
`DeadlineExceeded`. Click "Always Allow" once, or `docker logout` so public
pulls stay anonymous. And the Docker VM disk fills up from build caches over
time: `docker builder prune` and `docker image prune` are safe, volumes are
not (the market and the runner's caches live there).

## Quick start

```sh
./scripts/up.sh                    # config → build → start → wait → UI verification
./scripts/prepare-btc-m3-demo.sh   # seed evidence + arm the in-UI BTC runner
./scripts/down.sh                  # stop   (--wipe removes all state)
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

For the M3 demo, run `prepare-btc-m3-demo.sh` and open the VNC URL. The two
Basecamp microapps are the control surface:

1. Open **LEZ / BTC Maker**, select **Munich Vault 01**, and click **Publish
   offer** three times.
2. Select **Basel Vault 02** and click **Publish offer** twice. Each click creates
   one offer; switching
   back shows that each wallet retained its own inventory.
3. Open **LEZ / BTC Taker**, select a Taker wallet, and click **Take offer** on
   one or more order-book rows. Accepted offers are indexed to that Taker wallet
   and queue while the local runner handles one swap at a time.
4. Advance either active swap like two real users. Each dashboard exposes only
   its own ready action:

   | Direction | Maker composer | Role-owned action order |
   |---|---|---|
   | BTC → LEZ (`TakerSellsForeign`) | Sell LEZ | Taker: Lock BTC → Maker: Fund LEZ → Taker: Claim LEZ → Maker: Claim Bitcoin |
   | LEZ → BTC (`TakerSellsLez`) | Sell BTC | Taker: Lock LEZ → Maker: Lock BTC → Taker: Claim Bitcoin → Maker: Claim LEZ |

   The direction changes assets and authorities, not the four-gate lifecycle.
5. After completion, inspect the five transaction hashes and the wallet balance
   proof: BTC/LEZ opening → closing values, signed deltas, principal, and both
   Bitcoin fees. **Open local proof** links to the explorer at
   `http://127.0.0.1:3003/#/evidence`.

The default preparation command also publishes the certified, secret-free
snapshot of run `m5arm-08180005`, so the proof layout is useful before the first
new run. The command-line equivalent remains available:

```sh
./scripts/prepare-btc-m3-demo.sh --rerun
```

Each interactive swap executes the real LEZ/BTC application flow against the
**long-standing settlement chains** (see below), exports only its public
evidence, and refreshes the UI. Preparation takes seconds and a full four-gate
swap a couple of minutes, since no chain is provisioned per swap. An existing
run can be imported with `--from-run <m3-evidence-directory>`.

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
set -a
source runtime/runtime.env
set +a
docker cp scripts/market-bootstrap.sh lez-runner-arm:/tmp/lez-market-bootstrap.sh
docker exec \
  -e REPO_ROOT="$LEZ_M3_RUNNER_REPO_IN_CONTAINER" \
  -e MARKET_ROOT="$(dirname "$LEZ_M3_RUNNER_REPO_IN_CONTAINER")/market" \
  -e BTC_RPC_PASSWORD="$BTC_RPC_PASSWORD" \
  lez-runner-arm bash /tmp/lez-market-bootstrap.sh
```

Each run attaches with `LEZ_M3_ATTACH=1` plus `LEZ_ATTACH_BTC_RUN`,
`LEZ_ATTACH_LEZ_RUN`, the two `LEZ_ATTACH_*_IDENTITY_DIR` wallet directories,
and `LEZ_ATTACH_BOOTSTRAP_MANIFEST`. Attached chains are never torn down: the run's cleanup
attestation records `cleanup_scope:
secure_state_root_only_attached_chains_retained`. Wallet identities and the
bootstrap manifest live in `runner-work/market/`; deleting them would strand the
funded accounts.

## Verification

```sh
./scripts/verify-all.sh          # everything below, in one run
```

| Stage | What it proves |
|---|---|
| containers | every service is up |
| settlement chains | both chains are advancing and the Bitcoin spender index is present |
| `verify-explorers.py` | each certified swap's transactions are *displayed* — Bitcoin ones in a real block on the Bitcoin explorer (rendered content, hidden markup excluded), LEZ ones as live transactions with program and accounts. Runs predating the settlement chains are checked against the certified proof endpoint, since their chains no longer exist |
| `verify-market.py` | controller validation, idempotent replay, request-identity reuse, wallet ownership, offer lifecycle, role gating and the wallet ledger |
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
| `lez-explorer` | `images/lez-explorer` | zero-dependency Node proxy + UI over the indexer RPC (`getBlocks/…/getAccount`); search resolves any certified run's transaction hashes |
| `maker-init` / `taker-init` | debian | one-shot volume chowns; Taker reads only `maker-delivery-identity.pub`, never Maker private state |
| `maker-node` | `images/maker-node` | Maker-only image: canonical Node, CLI, Chat gateway, `lez-btc-maker-actor`, the LEZ v0.2 role sidecar program (spawned per swap) and `node-entrypoint.sh`; the entrypoint forwards loopback 18443/3040/8779 to Core, sequencer and indexer because the actor, wallet client and sidecars accept only literal-loopback endpoints |
| `taker-node` | `images/taker-node` | Taker-only image: canonical Node, CLI, Chat gateway, registry initializer, `lez-btc-taker-actor`, the role sidecar and `node-entrypoint.sh`; same loopback forwarders as the Maker |
| `btc-demo-launcher` | `images/btc-demo-launcher` | local-demo-only allowlisted `RunSwapJobV1` boundary; sole component with the Docker socket |
| `btc-demo-controller` | `images/btc-demo-controller` | unprivileged owner-local SQLite wallet market; calls the launcher over a mode-0600 UDS and publishes validated evidence |
| `basecamp-ui` | `images/basecamp-ui` | portable Basecamp 0.2.0-RC3 **inspector twin** + role install trees + qt-mcp; Xvfb/fluxbox/x11vnc; runs as the Node uid (4713) so the owner-only socket checks pass. Both desks read their market from the Nodes (`apps/basecamp/common/node_market.cpp`): offers, takes, the Taker's lock and claim, and the Maker's automatic progress; the demo controller is no longer on their path |

The UI reaches the role Nodes through shared named socket volumes mounted
read-only in Basecamp. The C++ backends enforce
`uid == socket owner && mode 0600`, hence the shared uid.

All services: `restart: unless-stopped`, log rotation, most are `read_only` +
`cap_drop: ALL` + `no-new-privileges`; ports published on loopback only.

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
compose.yaml               the stack
scripts/from-scratch.sh    everything from an empty host (prerequisites, sources, payloads, runner, stack)
scripts/up.sh              one-command bring-up (+ UI verification)
scripts/swap-through-ui.sh one complete swap driven through the two Basecamp apps
scripts/market-bootstrap.sh one-time settlement-chain bootstrap, runs inside the runner
scripts/stage-basecamp-package.sh stage one Nix-built package or module into the UI image
scripts/prepare-btc-m3-demo.sh publish/rerun completed LEZ/BTC M3 evidence
scripts/down.sh            stop / --wipe
scripts/gen-config.sh      renders runtime/ (LEZ configs, bitcoin.conf, secrets) — idempotent
scripts/btc-miner.sh       regtest mining loop
images/                    one dir per image (Dockerfile + payloads)
  basecamp-ui/assets/      portable Basecamp bundle, role trees, qt-mcp framework
  btc-demo-controller/     fixed-method owner-local bridge to the M3 runner
assets/lez-source/         pinned v0.2.0 config templates (bedrock, sequencer, indexer)
ui-tests/verify.mjs        end-to-end UI test (maker + taker) via the QML inspector
  runtime/                   generated state (gitignored; wiped by --wipe)
```

The external `runner-work` root must be mounted into `lez-runner-arm` at the
same absolute path recorded in `runtime/runtime.env`. The runner talks to the
host Docker daemon, so nested bind sources such as `runner-work/lez-source`
must resolve identically on the host and inside that container.

## Node-owned Bitcoin swaps (ADR 0213)

Both Nodes settle a BTC↔LEZ swap themselves: no runner, no demo controller.
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
`lez-miner`, which imports the deterministic mining key
(`rawtr(cMahea7zqjxrtgAbB7LSGbcQUr1uX1ojuat9jZodMN87JcbXMTcA)`) so the
mined coins fund the Taker; regtest's subsidy is exhausted past height 11k, so
mining to a fresh address yields nothing. Stage-1 limit: an aborted take
leaves its swap directory and sidecar behind; the next take is a new
reservation with its own sidecar, so nothing needs to be reset.

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
| `runner-work/market` and per-run `.e2e` roots | External evidence runner | Critical identities plus retained evidence |
| `lez-btc-demo-socket/market.sqlite3` | BTC demo controller | Demo book; not chain authority |

The primary M3 microapps gate the repository's fixed, genuine local LEZ/BTC
actor workflow and then present its completed evidence. They do not yet expose
arbitrary amounts, external wallets, public networks, or production signing.
The named profiles are local wallet-index aliases backed by fresh run-owned
signing keys, not persisted production wallets. The controller has no TCP
listener and accepts only its fixed wallet-market and role-action methods over
a mode-0600 owner socket. It has no Docker socket; the separate local-demo
launcher owns that authority and rejects jobs outside the frozen v1 schema.
