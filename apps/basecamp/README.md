# Basecamp role packages

This directory builds two independent Logos Basecamp 0.2 `ui_qml` packages.
The Maker desk publishes wallet-owned local BTC/LEZ inventory and performs only
Maker actions. The Taker desk browses that order book, takes offers into its
selected wallet, and performs only Taker actions. Together they gate the fixed
local M3 LEZ/Bitcoin runner at the four real actor boundaries and present its
completed transaction and balance evidence. Each QML view is unprivileged. Its
process-isolated C++ backend calls a fixed role allowlist over
an owner-only Unix socket. The negotiation path uses pinned Logos Chat `v0.2.2`
and its pinned Delivery runtime for signed public offer broadcasts plus the
peer-to-peer E2EE transport; the Rust
stores, signatures, role contributions, and chain-identity checks remain the
protocol authority. The public release exposes only the Bitcoin corridor;
future asset corridors return with their own milestone releases.

## Prerequisites and external resources

- Nix with flakes enabled, or the digest-pinned `nixos/nix` container recorded
  in the M6 evidence packet;
- the repository Rust binaries for the real-service exercises;
- Logos Chat `v0.2.2` and its followed Delivery/module-builder inputs, exactly
  resolved by `flake.lock`;
- Basecamp tag `0.2.0`, exact commit
  `48b26c0d33573b5dd3695ae5868b04328f79e5c6` (internal `0.2.0-RC3`);
- enough disk for the measured roughly 2.75 GB Basecamp closure plus build
  intermediates, using a dedicated Nix store when other work shares the host.

A cold Nix build fetches immutable GitHub flake inputs and immutable NARs from
`cache.nixos.org`. DNS, GitHub, or the Nix cache can therefore delay or fail
setup. Once the closure exists, the product tests run with no container network.
They use no public RPC, faucet, explorer, public funds, or public deployment.
The full chain corridors separately use isolated local LEZ v0.2 and foreign
Regtest nodes with deterministic local funds.

## Build all role outputs

From this directory, with Nix flakes enabled:

```sh
nix build --no-update-lock-file .#lez-maker-ui -o result-lez-maker-ui
nix build --no-update-lock-file .#lez-taker-ui -o result-lez-taker-ui
nix build --no-update-lock-file .#lez-maker-ui-lgx -o result-lez-maker-ui-lgx
nix build --no-update-lock-file .#lez-taker-ui-lgx -o result-lez-taker-ui-lgx
nix build --no-update-lock-file .#lez-maker-ui-install -o result-lez-maker-ui-install
nix build --no-update-lock-file .#lez-taker-ui-install -o result-lez-taker-ui-install
nix build --no-update-lock-file .#lez-maker-ui-integration-test
nix build --no-update-lock-file .#lez-taker-ui-integration-test
```

The `*-lgx` outputs are the distributable package archives. The `*-install`
outputs are developer-install trees for an isolated Basecamp `--user-dir`.
Do not put both roles into one evidence directory: separate user directories
make role discovery, socket authority, and cleanup auditable.

From the repository root, the same two official integration checks run through
the pinned Nix environment used by CI:

```sh
npm run test:m6:basecamp
```

## Install into isolated Basecamp user directories

Set absolute paths owned by the current user:

```sh
export M6_ROOT="${TMPDIR:-/tmp}/lez-m6-manual-$UID"
export M6_MAKER_USER="$M6_ROOT/basecamp-maker"
export M6_TAKER_USER="$M6_ROOT/basecamp-taker"
install -d -m 0700 "$M6_ROOT" "$M6_MAKER_USER" "$M6_TAKER_USER"
cp -a result-lez-maker-ui-install/. "$M6_MAKER_USER/"
cp -a result-lez-taker-ui-install/. "$M6_TAKER_USER/"
```

Build pinned Basecamp from its exact source checkout and record the output path:

```sh
git clone --branch 0.2.0 https://github.com/logos-co/logos-basecamp.git "$M6_ROOT/basecamp-src"
test "$(git -C "$M6_ROOT/basecamp-src" rev-parse HEAD)" = 48b26c0d33573b5dd3695ae5868b04328f79e5c6
nix build --no-update-lock-file --no-accept-flake-config \
  "path:$M6_ROOT/basecamp-src#app" -o "$M6_ROOT/basecamp"
export M6_BASECAMP_BIN="$M6_ROOT/basecamp/bin/LogosBasecamp"
```

The clone and cold Nix fetch are setup-only public dependencies. Pinning the
commit prevents silent source drift; it does not make an unavailable cache
reliable. Production distribution additionally remains subject to LOGOS-025.

## Run app-lifetime Logos Chat sessions and offer discovery

Build the role-fixed Chat gateways directly from this repository. Each role runs its own
endpoint. The Taker endpoint keeps one direct conversation and a bounded signed
offer index; the Maker endpoint keeps up to 32 direct conversations so
competing Takers receive separately correlated results. Start an endpoint with
its app and stop it when the app exits. Chat identity, conversations, history,
and the Taker discovery index are intentionally session-scoped; signed offer
state, the countersigned agreement, and replay authority stay durable in the
Rust stores.

Maker terminal (the Maker Node's existing `--chat-socket` is the final
owner-local authority):

```sh
export LEZ_LOGOS_CHAT_PRESET=logos.test
export LEZ_LOGOS_CHAT_GATEWAY_SOCKET="$M6_ROOT/runtime-maker/logos-chat-control.sock"
"$RUNNER_ROOT/target/release/lez-maker-chat-gateway" endpoint \
  --control-socket "$LEZ_LOGOS_CHAT_GATEWAY_SOCKET" \
  --maker-chat-socket "$M6_ROOT/runtime-maker/chat.sock" &
maker_chat_pid=$!
trap 'kill -INT "$maker_chat_pid" 2>/dev/null || true; wait "$maker_chat_pid" 2>/dev/null || true' EXIT
```

Taker terminal (configure `lez-taker-node`'s `chat_socket` to the proxy path
before starting that service):

```sh
export LEZ_LOGOS_CHAT_PRESET=logos.test
export LEZ_LOGOS_CHAT_GATEWAY_SOCKET="$M6_ROOT/runtime-taker/logos-chat-control.sock"
export LEZ_LOGOS_CHAT_PROXY_SOCKET="$M6_ROOT/runtime-taker/logos-chat-proxy.sock"
"$RUNNER_ROOT/target/release/lez-taker-chat-gateway" endpoint \
  --control-socket "$LEZ_LOGOS_CHAT_GATEWAY_SOCKET" \
  --proxy-socket "$LEZ_LOGOS_CHAT_PROXY_SOCKET" &
taker_chat_pid=$!
trap 'kill -INT "$taker_chat_pid" 2>/dev/null || true; wait "$taker_chat_pid" 2>/dev/null || true' EXIT
```

Open both apps with separate Basecamp `--user-dir` values. Once Chat's shared
Delivery node is online, the Maker signs its current offer states and current
app-lifetime Chat address, broadcasts them on
`/lez-atomic-swaps/1/offers/json`, and repeats every 10 seconds. The Taker's
**Browse authenticated offers** action reads only its verified in-memory
Delivery index and automatically creates a private conversation with the
selected offer's signed address; no pasted address or filesystem offer index is
part of this production Basecamp path. Closing either app calls
`chat.shutdown()`; also stop its paired gateway endpoint so the next launch
starts with no stale binding. **Reset Chat** remains an idle-only recovery
control and does not change durable offer or agreement state.

## Run the Maker package as a real user

Build and start the real Maker Node the way `deploy/images/maker-node/node-entrypoint.sh`
does (see the manual test walkthrough in [`deploy/README.md`](../../deploy/README.md)). Its owner RPC defaults
to `/run/lez/maker/node.sock`; a run-private absolute socket is safer for
parallel local work. Verify its directory is mode 0700, its socket is mode 0600,
and both are owned by your effective UID. Then launch:

```sh
export LEZ_MAKER_RPC_SOCKET="$M6_ROOT/runtime-maker/node.sock"
export M6_BASECAMP_USER_DIR="$M6_MAKER_USER"
../../scripts/m6-basecamp-launch-wrapper.sh
```

In Basecamp, open **LEZ / BTC Maker**, select **Munich Vault 01**, and click
**Publish offer** three times. Select **Basel Vault 02** and publish twice.
Each click creates exactly one independently takeable offer. Inventory is
indexed to each wallet; a pending offer can
only be withdrawn from the wallet that created it. When the Taker accepts an
offer, use **Fund 1,000 LEZ** and later **Claim Bitcoin** only when those actions
appear on that Maker wallet's swap row.

The route, history, monitor, claim, and refund controls below the primary desk
are the advanced prepared-service lane.

The backend reads the current pair and local-price revisions, then the route
click writes through `maker_local_route_save_v1`. Policy, price, and its replay
result commit in one SQLite transaction; a stale later write rolls the whole
operation back. Saving already-current terms returns their durable revisions
without manufacturing another mutation.

## Run the Taker package as a real user

Set `LEZ_M3_BTC_EVIDENCE_FILE` to the absolute path of a secret-free evidence
file produced by `deploy/full-swap/export-ui-evidence.sh`. The wallet market and
four actor-owned actions are supplied by the Docker deployment's bounded
controller at `LEZ_BTC_DEMO_RPC_SOCKET`; a standalone package without that
mode-0600 socket remains a safe evidence viewer and shows the market offline.
For the optional
prepared-corridor controls, also create the strict owner-private Taker Node role
configuration the way `deploy/images/taker-node/node-entrypoint.sh` renders it,
start `lez-taker-node` as the current user, and select its mode-0600 socket:

```sh
export LEZ_TAKER_RPC_SOCKET="$M6_ROOT/runtime-taker/node.sock"
export LEZ_M3_BTC_EVIDENCE_FILE="$PWD/../../deploy/full-swap/evidence-m5arm-08180005-ui.json"
export M6_BASECAMP_USER_DIR="$M6_TAKER_USER"
../../scripts/m6-basecamp-launch-wrapper.sh
```

In Basecamp, open **LEZ / BTC Taker**, select Zurich Wallet 01 or Limmat Wallet
02, and take a pending Maker offer. Move between the two desks only when that
role's next action appears:

| Direction | Maker composer | Role-owned action order |
|---|---|---|
| BTC → LEZ (`TakerSellsForeign`) | Sell LEZ | Taker: Lock BTC → Maker: Fund LEZ → Taker: Claim LEZ → Maker: Claim Bitcoin |
| LEZ → BTC (`TakerSellsLez`) | Sell BTC | Taker: Lock LEZ → Maker: Lock BTC → Taker: Claim Bitcoin → Maker: Claim LEZ |

After the fourth action, verify terminal revision `4 · completed`, two Bitcoin
plus three LEZ transaction hashes, and the wallet balance proof showing opening
→ closing balances, signed deltas, and BTC fees.

Production Basecamp initiation re-verifies that exact live signed Delivery
proof; the filesystem source remains a legacy CLI/offline seam. Initiation
commits registry authority before Chat/Delivery effects. A lost
response is recovered by the same request ID and immutable payload; changing a
field conflicts. Claim and Refund are mutually exclusive at the registry and
actor-journal layers.

## Automated product reproduction

The repository-owned static gate is fast and needs no Nix closure:

```sh
cd ../..
npm run test:m6:basecamp:contract
```

The official standalone checks are the two `nix build` integration outputs
above. The actor-real product test is
`apps/basecamp/tests/basecamp-role-product.mjs`; it is run by the M6 isolated
evidence recipe with `LOGOS_QT_MCP` pointing at the pinned Basecamp MCP test
framework, one role install tree, and one real owner socket. It covers:

- both missing-service fail-closed paths;
- Maker health, atomic route save, and history through `lez-maker-node`;
- wallet-indexed Maker inventory and the Taker order book;
- Taker completed M3 BTC evidence, including all five unique transaction IDs;
- Taker health plus BTC browsing through the gateway's live signed Delivery index;
- prepared Taker initiation, exact replay, list, monitor, and a durable registry
  assertion for the digest-derived `taker-ui-initiate-*` request.

For the prepared Taker case, `M6_TAKER_FIXTURE_JSON` also carries
`logos_offer_announcement_base64`: a freshly signed, unexpired announcement
obtained from the Maker snapshot RPC. The harness admits that exact proof through
`LEZ_LOGOS_CHAT_GATEWAY_SOCKET` before browsing, so confirm-time refresh tests
the production live index without internet access or a filesystem offer fixture.

The Dockerized BTC microapp's four role-owned actions gate the actual-node M3
workflow and publish that fresh run's transaction and balance proofs.
See [ADR 0147](../../docs/architecture/0147-isolate-basecamp-role-packages-over-owner-services.md)
and the [M6 package evidence](../../docs/evidence/m6-basecamp-role-packages-20260804.json).

The Chat negotiation process E2E uses only Unix-domain sockets and deterministic
local role roots. From the repository root it can be run after a one-time
dependency warm-up with Cargo networking disabled:

```sh
CARGO_NET_OFFLINE=true cargo test -p lez-maker-node \
  --test btc_chat_process \
  independent_role_roots_complete_chat_v2_without_fixture_actor_authority \
  -- --exact
```

For a physical network-denial check, run that same command in the dedicated
runner image with Docker `--network none`, a read-only source mount, and a
task-private target directory. The test launches only its own Maker Node,
Maker/Taker gateway endpoints, and the gateway's Unix-only local relay; it does
not use Chat's Delivery network, public RPC, or any running Compose service.

## Cleanup

After stopping only the processes started for this run:

```sh
rm -rf "$M6_ROOT"
rm -f result-lez-maker-ui result-lez-taker-ui result-lez-maker-ui-lgx result-lez-taker-ui-lgx
rm -f result-lez-maker-ui-install result-lez-taker-ui-install
```

Do not run a global Docker or Nix prune on a shared machine. Remove only the
dedicated container, volume, image reference, result links, and temporary path
created for the run after verifying their exact names.
