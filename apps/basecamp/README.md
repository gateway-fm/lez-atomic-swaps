# Basecamp role packages

This directory builds two independent Logos Basecamp 0.2 `ui_qml` packages.
The Maker console configures local routes and uses existing Maker actor methods.
The Taker route browses authenticated offers, admits a prepared swap, monitors
it, and exposes the generation-fenced terminal controls. Each QML view is
unprivileged. Its process-isolated C++ backend calls a fixed role allowlist over
an owner-only Unix socket. Negotiation messages use pinned Logos Chat `v0.2.2`
and its followed Delivery runtime for signed offer broadcasts plus peer-to-peer E2EE transport; the Rust
stores, signatures, role contributions, and chain-identity checks remain the
protocol authority.

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
nix build --no-update-lock-file .#maker -o result-maker
nix build --no-update-lock-file .#taker -o result-taker
nix build --no-update-lock-file .#maker-lgx -o result-maker-lgx
nix build --no-update-lock-file .#taker-lgx -o result-taker-lgx
nix build --no-update-lock-file .#maker-install -o result-maker-install
nix build --no-update-lock-file .#taker-install -o result-taker-install
nix build --no-update-lock-file .#maker-integration-test
nix build --no-update-lock-file .#taker-integration-test
```

The `*-lgx` outputs are the distributable package archives. The `*-install`
outputs are developer-install trees for an isolated Basecamp `--user-dir`.
Do not put both roles into one evidence directory: separate user directories
make role discovery, socket authority, and cleanup auditable.

## Install into isolated Basecamp user directories

Set absolute paths owned by the current user:

```sh
export M6_ROOT="${TMPDIR:-/tmp}/lez-m6-manual-$UID"
export M6_MAKER_USER="$M6_ROOT/basecamp-maker"
export M6_TAKER_USER="$M6_ROOT/basecamp-taker"
install -d -m 0700 "$M6_ROOT" "$M6_MAKER_USER" "$M6_TAKER_USER"
cp -a result-maker-install/. "$M6_MAKER_USER/"
cp -a result-taker-install/. "$M6_TAKER_USER/"
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

## Run one app-lifetime Logos Chat session

Build `lez-logos-chat-gateway`. Each role runs its own endpoint; the endpoint
contains only bounded in-memory queues and one pinned direct conversation.
Start it with the app and stop it when the app exits. Chat identity,
conversation, and history are intentionally session-scoped; the countersigned
agreement and replay authority stay durable in the Rust stores.

Maker terminal (the Maker daemon's existing `--chat-socket` remains the final
owner-local authority):

```sh
export LEZ_LOGOS_CHAT_PRESET=logos.test
export LEZ_LOGOS_CHAT_GATEWAY_SOCKET="$M6_ROOT/runtime-maker/logos-chat-control.sock"
"$RUNNER_ROOT/target/release/lez-logos-chat-gateway" endpoint \
  --role maker \
  --control-socket "$LEZ_LOGOS_CHAT_GATEWAY_SOCKET" \
  --maker-chat-socket "$M6_ROOT/runtime-maker/chat.sock" &
maker_chat_pid=$!
trap 'kill -INT "$maker_chat_pid" 2>/dev/null || true; wait "$maker_chat_pid" 2>/dev/null || true' EXIT
```

Taker terminal (configure `lez-taker-service`'s `chat_socket` to the proxy path
before starting that service):

```sh
export LEZ_LOGOS_CHAT_PRESET=logos.test
export LEZ_LOGOS_CHAT_GATEWAY_SOCKET="$M6_ROOT/runtime-taker/logos-chat-control.sock"
export LEZ_LOGOS_CHAT_PROXY_SOCKET="$M6_ROOT/runtime-taker/logos-chat-proxy.sock"
"$RUNNER_ROOT/target/release/lez-logos-chat-gateway" endpoint \
  --role taker \
  --control-socket "$LEZ_LOGOS_CHAT_GATEWAY_SOCKET" \
  --proxy-socket "$LEZ_LOGOS_CHAT_PROXY_SOCKET" &
taker_chat_pid=$!
trap 'kill -INT "$taker_chat_pid" 2>/dev/null || true; wait "$taker_chat_pid" 2>/dev/null || true' EXIT
```

Open both apps with separate Basecamp `--user-dir` values. In the Maker app,
refresh Chat and copy its current address. Paste that address into the Taker
app and connect. Closing either app calls `chat.shutdown()`; also stop its
paired gateway endpoint so the next launch starts with no stale binding.
If an unintended peer binds first, use **Reset Chat** while no request is in
flight, then share/paste the intended current address again. The owner-local
reset clears the gateway binding, outbox, and response cache without changing
any durable agreement state.

## Run the Maker package as a real user

Build and start the real Maker daemon using the normal Maker configuration from
[the manual user flows](../../docs/manual-user-flows.md). Its owner RPC defaults
to `/run/lez-atomic-swaps/maker.sock`; a run-private absolute socket is safer for
parallel local work. Verify its directory is mode 0700, its socket is mode 0600,
and both are owned by your effective UID. Then launch:

```sh
export LEZ_MAKER_RPC_SOCKET="$M6_ROOT/runtime-maker/maker.sock"
export M6_BASECAMP_USER_DIR="$M6_MAKER_USER"
../../scripts/m6-basecamp-launch-wrapper.sh
```

In Basecamp:

1. open **LEZ Atomic Swap Maker** and confirm **Backend connected**;
2. click **Check service**;
3. select the pair and direction, enter exact atomic-unit limits and price,
   then click **Save route atomically**;
4. click **Refresh swap history**;
5. for an existing role-owned swap, enter its ID and generation before using
   **Monitor**, **Claim**, or **Refund**.

The route click calls only `maker_local_route_save_v1`. Policy, price, and its
replay result commit in one SQLite transaction; a stale later write rolls the
whole operation back.

## Run the Taker package as a real user

Create the strict owner-private Taker service configuration described in
[Flow 1Y](../../docs/manual-user-flows.md#flow-1y-run-the-actual-taker-owner-service-and-prepared-acceptance),
start `lez-taker-service` as the current user, and select its mode-0600 socket:

```sh
export LEZ_TAKER_RPC_SOCKET="$M6_ROOT/runtime-taker/taker.sock"
export M6_BASECAMP_USER_DIR="$M6_TAKER_USER"
../../scripts/m6-basecamp-launch-wrapper.sh
```

In Basecamp:

1. open **LEZ Atomic Swap Taker** and confirm **Backend connected**;
2. click **Service health**, choose the pair/direction, and click
   **Browse authenticated offers**;
3. review the automatically populated offer ID, compressed Maker identity,
   signed-envelope SHA-256, foreign atomic units, and expected LEZ atomic
   units; Basecamp retains the exact signed live announcement as admission
   proof without exposing private actor material;
4. click **Confirm and initiate** once and retain the returned swap ID;
5. repeat the unchanged click to observe exact durable replay;
6. click **List my swaps**, enter the swap ID, and click **Monitor**;
7. use **Claim** or **Refund** only when monitor advertises that exact action and
   generation. For transparent ZEC claims, follow the displayed shielding
   reminder in the wallet after the swap.

Production Basecamp initiation re-verifies the exact live signed Delivery proof;
the filesystem source remains a legacy CLI/offline seam. Initiation commits
registry authority before Chat/Delivery effects. A lost
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
- Maker health, atomic route save, and history through `lez-maker-daemon`;
- Taker health plus browsing through the gateway's live signed Delivery index;
- prepared Taker initiation, exact replay, list, monitor, and a durable registry
  assertion for the digest-derived `taker-ui-initiate-*` request.

For the prepared Taker case, `M6_TAKER_FIXTURE_JSON` also carries
`logos_offer_announcement_base64`: a freshly signed, unexpired announcement
obtained from the Maker snapshot RPC. The harness admits that exact proof through
`LEZ_LOGOS_CHAT_GATEWAY_SOCKET` before browsing, so confirm-time refresh tests
the production live index without internet access or a filesystem offer fixture.

This Basecamp run stops at admitted/monitored swap composition. Terminal
actual-node Claim and Refund are retained as separate service/actor certificates
so the evidence does not pretend the UI run itself produced those transactions.
See [ADR 0147](../../docs/architecture/0147-isolate-basecamp-role-packages-over-owner-services.md)
and the [M6 package evidence](../../docs/evidence/m6-basecamp-role-packages-20260804.json).

The Chat negotiation process E2E uses only Unix-domain sockets and deterministic
local role roots. After a one-time dependency warm-up it runs with Cargo
networking disabled:

```sh
CARGO_NET_OFFLINE=true cargo test -p lez-maker-node \
  --test btc_chat_process \
  independent_role_roots_complete_chat_v2_without_fixture_actor_authority \
  -- --exact
```

For a physical network-denial check, run that same command in the dedicated
runner image with Docker `--network none`, a read-only source mount, and a
task-private target directory. The test launches only its own Maker daemon,
Maker/Taker gateway endpoints, and the gateway's Unix-only local relay; it does
not use Chat's Delivery network, public RPC, or any running Compose service.

## Cleanup

After stopping only the processes started for this run:

```sh
rm -rf "$M6_ROOT"
rm -f result-maker result-taker result-maker-lgx result-taker-lgx
rm -f result-maker-install result-taker-install
```

Do not run a global Docker or Nix prune on a shared machine. Remove only the
dedicated container, volume, image reference, result links, and temporary path
created for the run after verifying their exact names.
