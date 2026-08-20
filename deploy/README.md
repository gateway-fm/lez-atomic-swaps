# lez-swap-stack — fully dockerized local LEZ ↔ BTC swap environment

One compose project: LEZ v0.2 devnet + Bitcoin Core regtest + explorers for
both chains + the real maker daemon and taker service + the real Basecamp UI
(drivable over VNC and by automated end-to-end tests). All native arm64,
started with one command.

## Quick start

```sh
./scripts/up.sh                    # config → build → start → wait → UI verification
./scripts/prepare-btc-m3-demo.sh   # seed evidence + arm the in-UI BTC runner
./scripts/down.sh                  # stop   (--wipe removes all state)
```

`up.sh` ends with the repo-style UI verification (real Basecamp driven through
its QML inspector against the live daemon/service). Skip with `SKIP_UI_VERIFY=1`.

| What | Where |
|---|---|
| Bitcoin regtest RPC | `http://127.0.0.1:18443` (user `lezrpc`, password in `runtime/runtime.env`), auto-mining every 120 s |
| BTC explorer | http://127.0.0.1:3002 |
| LEZ explorer + M3 evidence | http://127.0.0.1:3003/#/evidence |
| **Basecamp UI (VNC)** | **`vnc://127.0.0.1:5901`** (password `lezswap`; override with `VNC_PASSWORD`) |
| Maker daemon | `docker exec lez-maker-node lez-maker --socket /run/lez/maker.sock health` |
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
4. Advance the active swap like two real users: **Taker: Lock BTC → Maker: Fund
   LEZ → Taker: Claim LEZ → Maker: Claim Bitcoin**. Each dashboard exposes only
   its own ready action.
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
and `LEZ_ATTACH_BOOTSTRAP_MANIFEST` (patches `0017`/`0018` in
`full-swap/patches/`). Attached chains are never torn down: the run's cleanup
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
| UI regressions | the two Basecamp suites against the live daemon and service |

The BTC view can be driven automatically:

```sh
docker compose --env-file runtime/runtime.env run --rm --no-deps --entrypoint node \
  basecamp-ui /ui-tests/verify.mjs taker
```

`prepare-ui-swap.sh` remains available as an optional M6 prepared-service
exercise for Zcash. It verifies offer discovery, Maker Chat acceptance, and
actor provisioning, but stops at `not_activated`; it is not the M3 BTC demo.

## Services

| Service | Image | Notes |
|---|---|---|
| `bitcoin-core` | `images/bitcoin-core` | official Core 31.1 binaries (from the checksum/Guix-verified archive used by the repo's e2e flow), distroless, `txindex=1` + `txospenderindex=1` (the actors' lock observation needs the spender index), healthchecked, settlement-chain labels |
| `bitcoin-init` | debian | one-shot datadir chown to the distroless uid |
| `btc-miner` | `images/btc-miner` | regtest coinbase miner over JSON-RPC, `MINE_INTERVAL` 120s so ambient blocks do not race swap confirmations |
| `btc-explorer` | `images/btc-explorer` | btc-rpc-explorer 3.4.0, patched for an exhausted regtest halving schedule (post-era subsidy is 0; upstream returns `undefined` and 500s the homepage) |
| `bedrock` | pinned multi-arch digest `91d6c5…` | LEZ v0.2 consensus node (exact pin from the repo's compose) |
| `sequencer` / `indexer` | `images/lez-services` | built from pinned `logos-execution-zone` v0.2.0 (a58fbce), native rebuild + arm64 r0vm |
| `lez-explorer` | `images/lez-explorer` | zero-dependency Node proxy + UI over the indexer RPC (`getBlocks/…/getAccount`); search resolves any certified run's transaction hashes |
| `maker-init` / `taker-init` | debian | one-shot volume chowns (0700 socket dirs, 0600 taker config) |
| `maker-node` | `images/maker-node` | real `lez-maker-daemon` + CLIs; owner socket on a shared volume |
| `taker-service` | `images/maker-node` | real `lez-taker-service`: health, authenticated offer discovery, Chat acceptance, durable admission, list, monitor, and fenced terminal actions |
| `btc-demo-controller` | `images/btc-demo-controller` | owner-local SQLite wallet market: create/withdraw/take plus four role-gated M3 actions; queues accepted swaps and publishes validated transaction and balance evidence |
| `basecamp-ui` | `images/basecamp-ui` | portable Basecamp 0.2.0-RC3 **inspector twin** + role install trees + qt-mcp; Xvfb/fluxbox/x11vnc; runs as the daemon uid (4713) so the owner-only socket checks pass |

The UI reaches the role services and demo controller through shared named
socket volumes mounted read-only in Basecamp. The C++ backends enforce
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
scripts/up.sh              one-command bring-up (+ UI verification)
scripts/prepare-btc-m3-demo.sh publish/rerun completed LEZ/BTC M3 evidence
scripts/prepare-ui-swap.sh optional ZEC Chat + actor-provisioning service lane
scripts/down.sh            stop / --wipe
scripts/gen-config.sh      renders runtime/ (LEZ configs, bitcoin.conf, secrets) — idempotent
scripts/btc-miner.sh       regtest mining loop
images/                    one dir per image (Dockerfile + payloads)
  basecamp-ui/assets/      portable Basecamp bundle, role trees, qt-mcp framework
  btc-demo-controller/     fixed-method owner-local bridge to the M3 runner
assets/lez-source/         pinned v0.2.0 config templates (bedrock, sequencer, indexer)
assets/taker-service.json  taker service startup config (no delivery sources)
ui-tests/verify.mjs        end-to-end UI test (maker + taker) via the QML inspector
  runtime/                   generated state (gitignored; wiped by --wipe)
```

The external `runner-work` root must be mounted into `lez-runner-arm` at the
same absolute path recorded in `runtime/runtime.env`. The runner talks to the
host Docker daemon, so nested bind sources such as `runner-work/lez-source`
must resolve identically on the host and inside that container.

## Demo boundary

The primary M3 microapps gate the repository's fixed, genuine local LEZ/BTC
actor workflow and then present its completed evidence. They do not yet expose
arbitrary amounts, external wallets, public networks, or production signing.
The named profiles are local wallet-index aliases backed by fresh run-owned
signing keys, not persisted production wallets. The controller has no TCP
listener and accepts only its fixed wallet-market and role-action methods over
a mode-0600 owner socket. Its Docker socket access is local-demo infrastructure
and is not a production boundary.
The optional prepared ZEC
lane is separate: it exercises real signed-offer verification, Maker Chat,
receipt binding, and durable actor provisioning, then ends at `not_activated`
without submitting funded chain effects.
