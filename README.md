# lez-atomic-swaps — M3+ app branch

Orphan branch holding the user-checkable M3+ slice of the LEZ atomic-swap
delivery: the real Basecamp Maker/Taker UI packages, the clickable HTML
prototypes, and the minimal M3 diagrams. Content was extracted from `main`
(licensing and app trees) at commit `5c384a5` of the private
`mandrigin/lez-atomic-swaps` repository.

Licensing: MIT OR Apache-2.0, unchanged from the source repository
(`LICENSE-MIT`, `LICENSE-APACHE`).

## What is here

| Path | What it is |
|---|---|
| `apps/m6-prototypes/` | Deterministic, secret-free HTML prototypes for the Maker and Taker journeys |
| `apps/basecamp/` | The real `ui_qml` Basecamp packages (Maker console, Taker route) built with the pinned `logos-module-builder` via nix |
| `apps/preview/` | Standalone preview host that loads the **real** QML view sources with stubbed backends — runs natively on macOS, no nix |
| `docs/diagrams.md` | Architecture diagram, swap flow diagram, atomicity explanation |
| `docs/m3-local-poc-operator-guide.md` | The M3 operator guide (full PoC recipe) |
| `deploy/` | Dockerized local LEZ/BTC stack, completed-run evidence UI, and VNC demo |

## Run the UI

For the genuine local M3 LEZ/BTC demo, use the Docker stack:

```sh
cd deploy
./scripts/up.sh
./scripts/prepare-btc-m3-demo.sh
# then open vnc://127.0.0.1:5901 (password: lezswap)
```

Open **LEZ / BTC Maker** and **LEZ / BTC Taker** as two separate desks. On the
Maker desk, select Munich Vault 01 and click **Publish offer** three times, then
select Basel Vault 02 and click it twice. On the Taker desk, select a Taker wallet and take one
or more pending offers. Each active swap advances through four explicit,
role-owned actions: **Taker locks BTC → Maker funds LEZ → Taker claims LEZ →
Maker claims BTC**. Accepted offers queue safely while one genuine M3 runner
uses the local chains. Completion publishes all five transaction hashes plus
opening/closing BTC and LEZ balances, principal movements, and Bitcoin fees.

The profiles are local demo wallet aliases. Each real run still creates fresh,
run-owned signing keys; this milestone does not persist production wallet keys.
The operator fallback remains `prepare-btc-m3-demo.sh --rerun`.

### 1. HTML prototypes (fastest)

```sh
scripts/run-prototypes.sh
# open the loopback URL it prints (maker.html / taker.html)
```

### 2. Real QML views on the stub host (native, no nix)

Requires Qt 6 (`brew install qt` on macOS; `CMAKE_PREFIX_PATH` elsewhere).

```sh
scripts/run-real-ui.sh
```

Loads `apps/basecamp/{maker,taker}/src/qml/Main.qml` **unmodified** into a
window with a Maker/Taker switcher. The host object `logos` and both role
backends are stubs returning canned sample JSON; no daemon, no Delivery/Chat,
no chain nodes are contacted. This is for visual review of the production
views; it is not a swap execution environment.

### 3. Production-faithful Basecamp packages (nix)

Per `apps/basecamp/README.md`, with the pinned `nixos/nix` container digest
and dedicated nix-store volume from the M6 evidence packet. On Apple-silicon
Macs run the container with `--platform linux/amd64` to match the pinned
evidence exactly.

```sh
cd apps/basecamp
nix build --no-update-lock-file .#maker -o result-maker
nix build --no-update-lock-file .#taker -o result-taker
```

## Building the Rust workspace on a Mac

The workspace targets Linux (e.g. `lez-swap-store` uses `openat2`); on macOS
use the container:

```sh
docker run --rm -v "$PWD":/workspace -w /workspace rust:1.96.0 \
    cargo check --workspace --locked
```

(Verified: all 21 crates check clean in ~1 minute on arm64.)
