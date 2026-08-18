# Basecamp role packages

This directory builds two independent Logos Basecamp 0.2 `ui_qml` packages.
The Maker desk publishes wallet-owned local BTC/LEZ inventory and performs only
Maker actions. The Taker desk browses that order book, takes offers into its
selected wallet, and performs only Taker actions. Together they gate the fixed
local M3 LEZ/Bitcoin runner at the four real actor boundaries and present its
completed transaction and balance evidence. The older prepared-corridor
controls remain available as an advanced lane. Each QML view is
unprivileged. Its process-isolated C++ backend calls a fixed role allowlist over
an owner-only Unix socket.

## Prerequisites and external resources

- Nix with flakes enabled, or the digest-pinned `nixos/nix` container recorded
  in the M6 evidence packet;
- the repository Rust binaries for the real-service exercises;
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
prepared-corridor controls, also create the strict owner-private Taker service
configuration described in
[Flow 1Y](../../docs/manual-user-flows.md#flow-1y-run-the-actual-taker-owner-service-and-prepared-acceptance),
start `lez-taker-service` as the current user, and select its mode-0600 socket:

```sh
export LEZ_TAKER_RPC_SOCKET="$M6_ROOT/runtime-taker/taker.sock"
export LEZ_M3_BTC_EVIDENCE_FILE="$PWD/../../deploy/full-swap/evidence-m5arm-08180005-ui.json"
export M6_BASECAMP_USER_DIR="$M6_TAKER_USER"
../../scripts/m6-basecamp-launch-wrapper.sh
```

In Basecamp, open **LEZ / BTC Taker**, select Zurich Wallet 01 or Limmat Wallet
02, and take a pending Maker offer. Use **Lock 0.01000000 BTC** and later
**Claim 1,000 LEZ** only when those actions appear. Move to the Maker desk for
the intervening Maker actions. After the fourth action, verify terminal revision
`4 · completed`, two Bitcoin plus three LEZ transaction hashes, and the wallet
balance proof showing opening → closing balances, signed deltas, and BTC fees.

For the separate optional prepared Zcash service lane:

1. click **Service health**, choose `Zcash / TakerSellsLez`, and click
   **Browse authenticated offers**;
2. review the automatically selected newest offer; its ID, compressed Maker
   identity, signed-envelope SHA-256, foreign units, and expected LEZ units are
   populated into the exact review form without manual transcription;
3. click **Confirm and initiate** once and retain the returned swap ID;
4. repeat the unchanged click to observe exact durable replay;
5. use the automatically adopted current swap ID and click **Monitor**;
6. use **Claim** or **Refund** only when monitor advertises that exact action and
   generation. For transparent ZEC claims, follow the displayed shielding
   reminder in the wallet after the swap.

Initiation commits registry authority before Chat/filesystem effects. A lost
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
- wallet-indexed Maker inventory and the Taker order book;
- Taker completed M3 BTC evidence, including all five unique transaction IDs;
- Taker health and optional offer list through `lez-taker-service`;
- prepared Taker initiation, exact replay, list, monitor, and a durable registry
  assertion for request `taker-ui-initiate-001`.

The optional prepared-service Basecamp run stops at admitted/monitored ZEC swap
composition. Its terminal Claim and Refund remain separate service/actor
certificates. The Dockerized BTC microapp is different: its four role-owned
actions gate the actual-node M3 workflow and publish that fresh run's
transaction and balance proofs.
See [ADR 0147](../../docs/architecture/0147-isolate-basecamp-role-packages-over-owner-services.md)
and the [M6 package evidence](../../docs/evidence/m6-basecamp-role-packages-20260804.json).

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
