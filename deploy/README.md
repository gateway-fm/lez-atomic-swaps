# lez-swap-stack — fully dockerized local LEZ ↔ BTC swap environment

One compose project: LEZ v0.2 devnet + Bitcoin Core regtest + explorers for
both chains + the real maker daemon and taker service + the real Basecamp UI
(drivable over VNC and by automated end-to-end tests). All native arm64,
started with one command.

## Quick start

```sh
./scripts/up.sh                    # config → build → start → wait → UI verification
./scripts/prepare-btc-m3-demo.sh   # publish the certified completed M3 BTC run
./scripts/down.sh                  # stop   (--wipe removes all state)
```

`up.sh` ends with the repo-style UI verification (real Basecamp driven through
its QML inspector against the live daemon/service). Skip with `SKIP_UI_VERIFY=1`.

| What | Where |
|---|---|
| Bitcoin regtest RPC | `http://127.0.0.1:18443` (user `lezrpc`, password in `runtime/runtime.env`), auto-mining every 15 s |
| BTC explorer | http://127.0.0.1:3002 |
| LEZ explorer + M3 evidence | http://127.0.0.1:3003/#/evidence |
| **Basecamp UI (VNC)** | **`vnc://127.0.0.1:5901`** (password `lezswap`; override with `VNC_PASSWORD`) |
| Maker daemon | `docker exec lez-maker-node lez-maker --socket /run/lez/maker.sock health` |
| UI verify | `docker compose run --rm --no-deps --entrypoint node basecamp-ui /ui-tests/verify.mjs [maker\|taker]` |
| Switch UI role | `BASECAMP_ROLE=taker docker compose up -d basecamp-ui` |
| Logs | `docker compose logs -f <service>` |

For the M3 demo, run `prepare-btc-m3-demo.sh`, open the VNC URL, and choose
**LEZ / BTC Settlement**. The dark Swiss-poster view opens on a completed
`Bitcoin / TakerSellsForeign` run and exposes all five public transaction
identities: two Bitcoin effects and three LEZ effects. Each card includes its
chain, actor, amount, block identity, and finality. **Open local proof** links
to the read-only evidence explorer at `http://127.0.0.1:3003/#/evidence`.

The default command publishes the repository's certified, secret-free snapshot
of run `m5arm-08180005`. To generate new transactions first, use the already
provisioned native-arm64 runner:

```sh
./scripts/prepare-btc-m3-demo.sh --rerun
```

That normally takes about five minutes, executes the real LEZ/BTC application
flow on isolated local chains, exports only its public evidence, and refreshes
the UI. An existing run can be imported with
`--from-run <m3-evidence-directory>`.

The BTC view can be driven automatically:

```sh
docker compose run --rm --no-deps --entrypoint node \
  basecamp-ui /ui-tests/verify.mjs taker
```

`prepare-ui-swap.sh` remains available as an optional M6 prepared-service
exercise for Zcash. It verifies offer discovery, Maker Chat acceptance, and
actor provisioning, but stops at `not_activated`; it is not the M3 BTC demo.

## Services

| Service | Image | Notes |
|---|---|---|
| `bitcoin-core` | `images/bitcoin-core` | official Core 31.1 binaries (from the checksum/Guix-verified archive used by the repo's e2e flow), distroless, `txindex=1`, healthchecked |
| `bitcoin-init` | debian | one-shot datadir chown to the distroless uid |
| `btc-miner` | `images/btc-miner` | regtest coinbase miner over JSON-RPC |
| `btc-explorer` | `images/btc-explorer` | btc-rpc-explorer 3.4.0 |
| `bedrock` | pinned multi-arch digest `91d6c5…` | LEZ v0.2 consensus node (exact pin from the repo's compose) |
| `sequencer` / `indexer` | `images/lez-services` | built from pinned `logos-execution-zone` v0.2.0 (a58fbce), native rebuild + arm64 r0vm |
| `lez-explorer` | `images/lez-explorer` | zero-dependency Node proxy + UI over the indexer RPC (`getBlocks/…/getAccount`) |
| `maker-init` / `taker-init` | debian | one-shot volume chowns (0700 socket dirs, 0600 taker config) |
| `maker-node` | `images/maker-node` | real `lez-maker-daemon` + CLIs; owner socket on a shared volume |
| `taker-service` | `images/maker-node` | real `lez-taker-service`: health, authenticated offer discovery, Chat acceptance, durable admission, list, monitor, and fenced terminal actions |
| `basecamp-ui` | `images/basecamp-ui` | portable Basecamp 0.2.0-RC3 **inspector twin** + role install trees + qt-mcp; Xvfb/fluxbox/x11vnc; runs as the daemon uid (4713) so the owner-only socket checks pass |

The UI reaches the daemon through shared named volumes (`maker_socket`,
`taker_socket`) mounted read-only at `/run/lez-maker` / `/run/lez-taker`; the
C++ backends enforce `uid == socket owner && mode 0600`, hence the shared uid.

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
assets/lez-source/         pinned v0.2.0 config templates (bedrock, sequencer, indexer)
assets/taker-service.json  taker service startup config (no delivery sources)
ui-tests/verify.mjs        end-to-end UI test (maker + taker) via the QML inspector
runtime/                   generated state (gitignored; wiped by --wipe)
```

## Demo boundary

The primary M3 view is a read-only presentation of a genuinely completed local
LEZ/BTC run; it does not claim that pressing a Basecamp button broadcast those
transactions. `prepare-btc-m3-demo.sh --rerun` is the reproducible path that
creates fresh chain effects before republishing them. The optional prepared ZEC
lane is separate: it exercises real signed-offer verification, Maker Chat,
receipt binding, and durable actor provisioning, then ends at `not_activated`
without submitting funded chain effects.
