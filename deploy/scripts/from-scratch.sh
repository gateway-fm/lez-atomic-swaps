#!/usr/bin/env bash
# from-scratch.sh — bring the whole local LEZ ↔ BTC swap environment up from
# nothing, reproducibly, on an arm64 host (macOS with Docker Desktop, or Linux
# with Docker): prerequisites, pinned sources, every image payload built in
# throwaway containers, the settlement market, the stack, and the Basecamp UI.
#
#   deploy/scripts/from-scratch.sh [--workspace DIR] [--swap] [--only PHASE]
#
# Every phase is idempotent: it checks what already exists and does only the
# missing work, so a rerun after a failure continues where it stopped.
#
#   host      docker, jq, git, curl, openssl, xxd (Homebrew on macOS)
#   sources   pinned checkouts next to this repo (the "workspace")
#   nix       Basecamp bundle, qt-mcp, both role packages, Chat + Delivery
#   rust      Node binaries and Bitcoin actors in the pinned rust image
#   build     LEZ services, r0vm, rapidsnark, the escrow artifact, the LEZ
#             sidecar and the wallet identities, each in one `docker run --rm`
#             of the ephemeral builder image (deploy/builder)
#   stage     Bitcoin Core, LEZ services, r0vm, sidecar into the image contexts
#   stack     gen-config → compose build → up → market bootstrap → UI suites
#   swap      (--swap) one full BTC → LEZ swap through the two Basecamp apps
#
# Long cold steps on Apple silicon: Nix closures (~15 min from the Logos cache),
# LEZ services (~40 min), r0vm (~30 min), cargo-risczero (~1.5 h), the guest
# ELF (~10 min). Registries and build targets live in named Docker volumes
# (lez-build-*), outputs in the provision directory, so a rerun takes minutes.
# No long-lived container is left behind and nothing holds the host Docker
# socket except the reproducible guest build, for the duration of that run.
set -euo pipefail

DEPLOY_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_ROOT="$(cd "$DEPLOY_ROOT/.." && pwd)"
WORKSPACE="$(cd "$REPO_ROOT/.." && pwd)"
RUN_SWAP=0
ONLY=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --workspace) WORKSPACE="$(mkdir -p "$2" && cd "$2" && pwd)"; shift 2 ;;
    --swap) RUN_SWAP=1; shift ;;
    --only) ONLY="$2"; shift 2 ;;
    -h|--help) sed -n 2,26p "${BASH_SOURCE[0]}"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 64 ;;
  esac
done

# ---- pins -------------------------------------------------------------------
readonly LEZ_SOURCE_TAG=v0.2.0
readonly LEZ_SOURCE_COMMIT=a58fbce2ff48c58b7bb5001b1a27e64b9596ee3a
readonly BASECAMP_TAG=0.2.0
readonly BASECAMP_COMMIT=48b26c0d33573b5dd3695ae5868b04328f79e5c6
readonly NIX_IMAGE="nixos/nix:2.30.2@sha256:7894650fb65234b35c80010e6ca44149b70a4a216118a6b7e5c6f6ae377c8d21"
readonly RUST_IMAGE="rust:1.96.0-bookworm"
readonly RISC0_TAG=v3.0.5
readonly RAPIDSNARK_URL="https://github.com/logos-blockchain/logos-blockchain-rust-rapidsnark/releases/download/rapidsnark-pic-v0.0.8/rapidsnark-linux-aarch64-pic-v0.0.8.zip"
readonly RAPIDSNARK_LIB_SHA256="43553a6ae796621c63837829fb8b35c46cd8f0ffdb1d88f3761eb10ddbe59657"
# The escrow guest's pinned digests come from the artifact verifier of the
# commit under test, so the artifact, the market bootstrap and the swaps agree
# on one program.
pinned() { sed -n "s/^ *${1}=\"\([0-9a-f]\{64\}\)\".*/\1/p" "$REPO_ROOT/scripts/verify-lez-v02-provisional.sh" | head -1; }
ESCROW_PROGRAM_ID="${ESCROW_PROGRAM_ID:-$(pinned expected_image_id)}"
GUEST_ELF_SHA256="$(pinned expected_elf_sha256)"
[[ "$ESCROW_PROGRAM_ID" =~ ^[0-9a-f]{64}$ && "$GUEST_ELF_SHA256" =~ ^[0-9a-f]{64}$ ]] || { echo "cannot read the escrow pins" >&2; exit 1; }
readonly ESCROW_PROGRAM_ID GUEST_ELF_SHA256
readonly BUILDER_IMAGE=lez-builder:local
readonly WALLETS=(maker-munich-01 maker-basel-02 taker-zurich-01 taker-limmat-02)

# Workspace layout. Hosts provisioned before the ephemeral builder keep their
# market root under runner-work/; a fresh workspace uses market/ directly.
LEZ_SOURCE="$WORKSPACE/lez-source"
[[ -d "$LEZ_SOURCE/.git" ]] || [[ ! -d "$WORKSPACE/runner-work/lez-source/.git" ]] || LEZ_SOURCE="$WORKSPACE/runner-work/lez-source"
MARKET_ROOT="$WORKSPACE/market"
[[ -d "$MARKET_ROOT/identities" ]] || [[ ! -d "$WORKSPACE/runner-work/market/identities" ]] || MARKET_ROOT="$WORKSPACE/runner-work/market"
PROVISION="$WORKSPACE/provision/data"
BASECAMP_SRC="$WORKSPACE/basecamp"
ASSETS="$DEPLOY_ROOT/images/basecamp-ui/assets"

log() { printf '\n[%s %s] %s\n' "$(date -u +%H:%M:%S)" "${PHASE:-}" "$*"; }
fail() { echo "from-scratch failed: $*" >&2; exit 1; }
phase_wanted() { [[ -z "$ONLY" || "$ONLY" == "$1" ]]; }

# ---- host ---------------------------------------------------------------------
phase_host() {
  PHASE=host
  if [[ "$(uname -m)" != "arm64" && "$(uname -m)" != "aarch64" ]]; then
    fail "this environment is pinned to arm64 (native LEZ, r0vm, Basecamp builds)"
  fi
  if [[ "$(uname -s)" == "Darwin" ]]; then
    command -v brew >/dev/null || fail "install Homebrew first: https://brew.sh"
    for formula in jq git curl openssl; do
      command -v "$formula" >/dev/null || { log "installing $formula"; brew install "$formula"; }
    done
    if ! command -v docker >/dev/null; then
      log "installing Docker Desktop"
      brew install --cask docker
    fi
    if ! docker info >/dev/null 2>&1; then
      log "starting Docker Desktop"
      open -a Docker
    fi
  else
    for tool in docker jq git curl openssl xxd; do
      command -v "$tool" >/dev/null || fail "install $tool (apt: docker.io jq git curl openssl xxd)"
    done
  fi
  command -v xxd >/dev/null || fail "xxd is required (part of vim on macOS)"
  for _ in $(seq 1 60); do docker info >/dev/null 2>&1 && break; sleep 5; done
  docker info >/dev/null 2>&1 || fail "the Docker daemon did not come up"
  docker compose version >/dev/null || fail "docker compose plugin is required"
  log "docker $(docker version -f '{{.Server.Version}}') ready; workspace $WORKSPACE"
}

# ---- sources -------------------------------------------------------------------
clone_pinned() { # clone_pinned <url> <ref> <commit> <dir>
  local url="$1" ref="$2" commit="$3" dir="$4"
  if [[ ! -d "$dir/.git" ]]; then
    log "cloning $url@$ref"
    git clone --quiet --branch "$ref" --single-branch "$url" "$dir"
  fi
  [[ "$(git -C "$dir" rev-parse HEAD)" == "$commit" ]] || fail "$dir is not at $commit"
}

phase_sources() {
  PHASE=sources
  mkdir -p "$PROVISION" "$MARKET_ROOT"
  chmod 0700 "$MARKET_ROOT"
  clone_pinned https://github.com/logos-blockchain/logos-execution-zone.git \
    "$LEZ_SOURCE_TAG" "$LEZ_SOURCE_COMMIT" "$LEZ_SOURCE"
  clone_pinned https://github.com/logos-co/logos-basecamp.git \
    "$BASECAMP_TAG" "$BASECAMP_COMMIT" "$BASECAMP_SRC"
  log "sources pinned"
}

# ---- nix -----------------------------------------------------------------------
# Builds run in the pinned Nix image with a persistent store volume. The image's
# own /nix seeds the volume on first use; /bin/sh is relinked because the
# image's link can predate the store the volume carries.
nix_build() { # nix_build <output-link-name> <flake-ref> [extra nix args...]
  local name="$1" ref="$2"; shift 2
  docker run --rm -v lez-nix-store-arm:/nix -v "$REPO_ROOT:/workspace:ro" \
    -v "$BASECAMP_SRC:/src/basecamp:ro" -v "$ASSETS/.nix-out:/out" \
    -e NIX_CURL_FLAGS='--user-agent lez-from-scratch/1.0' "$NIX_IMAGE" bash -c '
      set -e
      [[ -x /bin/sh ]] || ln -sf "$(ls -d /nix/store/*-bash-5*/bin/bash | head -1)" /bin/sh
      # Unsandboxed builds refuse to start while a stray HOME from an earlier
      # derivation exists; clear it and retry.
      for attempt in 1 2 3 4; do
        rm -rf /homeless-shelter
        nix --extra-experimental-features "nix-command flakes" \
          --option build-users-group "" --option sandbox false \
          build -L --accept-flake-config --no-update-lock-file "$@" && exit 0
      done
      exit 1' _ "$ref" -o "/out/$name" "$@" 2>&1 | grep -v '^warning:' | tail -5
  [[ -L "$ASSETS/.nix-out/$name" ]] || fail "nix build of $ref produced no output link"
}

nix_export() { # nix_export <output-link-name> <destination-dir>
  local name="$1" dest="$2"
  rm -rf "$dest"; mkdir -p "$dest"
  docker run --rm -v lez-nix-store-arm:/nix -v "$ASSETS/.nix-out:/out:ro" "$NIX_IMAGE" \
    bash -c 'tar -C "$(readlink -f "/out/$1")" -cf - .' _ "$name" | tar -C "$dest" -xf -
  chmod -R u+rwX "$dest"
}

phase_nix() {
  PHASE=nix
  mkdir -p "$ASSETS/.nix-out"
  local chat_rev; chat_rev="$(jq -r '.nodes.chat_module.locked.rev' "$REPO_ROOT/apps/basecamp/flake.lock")"
  if [[ ! -x "$ASSETS/bundle/bin/LogosBasecamp" ]]; then
    log "building the Basecamp inspector bundle (path:/src/basecamp#bin-bundle-dir-inspector)"
    nix_build bundle "path:/src/basecamp#bin-bundle-dir-inspector"
    nix_export bundle "$ASSETS/bundle"
  fi
  if [[ ! -d "$ASSETS/qt-mcp" ]]; then
    log "building qt-mcp"
    nix_build qt-mcp "path:/src/basecamp#logos-qt-mcp"
    nix_export qt-mcp "$ASSETS/qt-mcp"
  fi
  local role
  for role in maker taker; do
    if [[ ! -f "$ASSETS/$role-user/plugins/lez_atomic_swap_$role/manifest.json" ]]; then
      log "building the $role package"
      nix_build "$role-install" "path:/workspace/apps/basecamp#lez-$role-ui-install"
      nix_export "$role-install" "$ASSETS/.nix-out/$role-user"
      bash "$DEPLOY_ROOT/scripts/stage-basecamp-package.sh" "$ASSETS/.nix-out/$role-user" "$role"
    fi
  done
  local module output
  for module in chat_module:lgx delivery_module:delivery_module-lgx; do
    output="${module#*:}"; module="${module%%:*}"
    if [[ ! -f "$ASSETS/bundle/modules/$module/manifest.json" ]]; then
      log "building $module from logos-chat-module@$chat_rev"
      nix_build "$module" "github:logos-co/logos-chat-module/$chat_rev#$output"
      nix_export "$module" "$ASSETS/.nix-out/$module"
      bash "$DEPLOY_ROOT/scripts/stage-basecamp-package.sh" \
        "$(find "$ASSETS/.nix-out/$module" -name '*.lgx' | head -1)" module
    fi
  done
  log "Basecamp payloads staged"
}

# ---- rust ----------------------------------------------------------------------
phase_rust() {
  PHASE=rust
  local bins=(maker-node/lez-maker-node maker-node/lez-maker-cli maker-node/lez-maker-chat-gateway
    maker-node/lez-runtime-healthcheck maker-node/lez-btc-maker-actor
    taker-node/lez-taker-node taker-node/lez-taker-cli
    taker-node/lez-taker-chat-gateway taker-node/lez-taker-registry-init
    taker-node/lez-runtime-healthcheck taker-node/lez-btc-taker-actor
    lez-services/lez-runtime-healthcheck)
  local missing=0 b
  for b in "${bins[@]}"; do [[ -x "$DEPLOY_ROOT/images/$b" ]] || missing=1; done
  if [[ "$missing" == 1 ]]; then
    log "building the role Node binaries and Bitcoin actors in $RUST_IMAGE"
    docker run --rm -v "$REPO_ROOT:/workspace" -v lez-rust-cache:/cache -w /workspace \
      -e CARGO_HOME=/cache/cargo-home -e CARGO_TARGET_DIR=/cache/target "$RUST_IMAGE" bash -c '
        set -e
        cargo build --locked -p lez-maker-node --bins -p lez-taker-node --bins -p lez-runtime-healthcheck -p btc-reference-actor 2>&1 | tail -3
        for b in '"${bins[*]}"'; do install -m 0755 "/cache/target/debug/${b#*/}" "deploy/images/$b"; done
        chown -R "$(stat -c %u:%g deploy)" deploy/images/maker-node deploy/images/taker-node deploy/images/lez-services'
  fi
  log "Node binaries staged"
}

# ---- build ---------------------------------------------------------------------
# Every step is one throwaway container of the builder image. Registries and
# build targets persist in named volumes; outputs land in $PROVISION.
builder_run() { # builder_run [docker run flags...] -- <bash script>
  local flags=()
  while [[ "$1" != "--" ]]; do flags+=("$1"); shift; done; shift
  docker run --rm -v "$REPO_ROOT:/workspace" -v "$LEZ_SOURCE:/lez-source" -v "$PROVISION:/provision" \
    -v lez-build-cargo:/cache/cargo -v lez-build-target:/cache/target \
    -e CARGO_HOME=/cache/cargo -e CARGO_TARGET_DIR=/cache/target/workspace \
    -e RAPIDSNARK_LIB_DIR=/provision/rapidsnark-arm \
    -e BINDGEN_EXTRA_CLANG_ARGS=-I/usr/lib/gcc/aarch64-linux-gnu/13/include \
    -w /workspace "${flags[@]}" "$BUILDER_IMAGE" bash -c "set -e; $*"
}
# Outputs are written as root inside the container (on Linux; Docker Desktop
# maps them to the host user already); hand the builder's own directories to
# the host user and leave anything else in the provision root alone.
own_provision() {
  docker run --rm -v "$PROVISION:/provision" "$BUILDER_IMAGE" bash -c \
    'for d in rapidsnark-arm lez-services tools-arm escrow-artifact sidecar risc0; do [[ -e /provision/$d ]] && chown -R "$1" "/provision/$d" 2>/dev/null; done; true' _ "$(id -u):$(id -g)"
}

phase_build() {
  PHASE=build
  if ! docker image inspect "$BUILDER_IMAGE" >/dev/null 2>&1; then
    log "building the ephemeral builder image"
    docker build -q -t "$BUILDER_IMAGE" "$DEPLOY_ROOT/builder" >/dev/null
  fi
  docker container inspect lez-runner-arm >/dev/null 2>&1 &&
    log "note: the retired lez-runner-arm container is still present; docker rm -f lez-runner-arm once its outputs are in $PROVISION"

  # rapidsnark prover libraries (the Logos fork's aarch64 release)
  if [[ ! -f "$PROVISION/rapidsnark-arm/librapidsnark.a" ]]; then
    log "fetching rapidsnark aarch64 libraries"
    builder_run -- "tmp=\$(mktemp -d); curl -fsSL '$RAPIDSNARK_URL' -o \$tmp/r.zip; unzip -qo \$tmp/r.zip -d \$tmp/r;
      mkdir -p /provision/rapidsnark-arm; cp \$(find \$tmp/r -name '*.a') /provision/rapidsnark-arm/"
  fi
  printf '%s  %s\n' "$RAPIDSNARK_LIB_SHA256" "$PROVISION/rapidsnark-arm/librapidsnark.a" | shasum -a 256 --check --strict --quiet ||
    fail "rapidsnark library digest mismatch"

  # LEZ v0.2 services, native release build with rust 1.94.0
  if [[ ! -x "$PROVISION/lez-services/sequencer_service" || ! -x "$PROVISION/lez-services/indexer_service" ]]; then
    log "building LEZ v0.2 services (rust 1.94.0, release, locked; ~40 min cold)"
    builder_run -- "cd /lez-source; CARGO_TARGET_DIR=/cache/target/lez cargo +1.94.0 build --locked --release \
      --package sequencer_service --package indexer_service 2>&1 | tail -2;
      mkdir -p /provision/lez-services; install -m 0755 /cache/target/lez/release/sequencer_service /cache/target/lez/release/indexer_service /provision/lez-services/"
  fi

  # r0vm from the risc0 tag (no arm64 release asset exists)
  if [[ ! -x "$PROVISION/tools-arm/bin/r0vm" ]]; then
    log "building r0vm from risc0 $RISC0_TAG (~30 min cold)"
    builder_run -- "[[ -d /provision/risc0/.git ]] || git clone --quiet --depth 1 --branch $RISC0_TAG https://github.com/risc0/risc0.git /provision/risc0;
      cd /provision/risc0 && CARGO_TARGET_DIR=/cache/target/r0vm cargo +1.96.0 install --path risc0/r0vm --locked --root /provision/tools-arm 2>&1 | tail -2"
  fi
  [[ "$(builder_run -- '/provision/tools-arm/bin/r0vm --version')" == "risc0-r0vm ${RISC0_TAG#v}" ]] || fail "r0vm is not ${RISC0_TAG#v}"

  # the escrow artifact: deployer + guest ELF at the commit's pinned digest.
  # The reproducible guest build runs inside risc0's pinned builder image, so
  # this one step gets the host Docker socket for the duration of the run.
  local guest_elf="$PROVISION/escrow-artifact/riscv-guest/lez-zec-escrow-v02-methods/lez-zec-escrow-v02-guest/riscv32im-risc0-zkvm-elf/docker/zec_escrow_v02.bin"
  if [[ ! -x "$PROVISION/escrow-artifact/debug/lez-zec-escrow-v02-deployer" || "$(shasum -a 256 "$guest_elf" 2>/dev/null | cut -c1-64)" != "$GUEST_ELF_SHA256" ]]; then
    log "building the escrow artifact for this commit (cargo-risczero from source, then the pinned guest; ~1.5 h cold)"
    builder_run -v /var/run/docker.sock:/var/run/docker.sock -- "rm -rf /provision/escrow-artifact/docker-guest-source /provision/escrow-artifact/riscv-guest /provision/escrow-artifact/debug/lez-zec-escrow-v02-deployer;
      mkdir -p /tmp/lez-risc0-home/toolchains/v1.94.1-rust-aarch64-unknown-linux-gnu && printf '[default_versions]\nrust = \"1.94.1\"\n' > /tmp/lez-risc0-home/settings.toml;
      RUN_ID=arm-rebuild LEZ_NATIVE_TOOLS=1 LEZ_V02_NATIVE_R0VM=/provision/tools-arm/bin/r0vm LEZ_V02_ARTIFACT_TARGET_DIR=/provision/escrow-artifact \
      LEZ_V02_TOOL_DIR=/provision/tools-arm LEZ_V02_SOURCE_DIR=/lez-source CARGO_TARGET_DIR=/cache/target/provisional CARGO_BUILD_JOBS=6 \
      scripts/verify-lez-v02-provisional.sh 2>&1 | tail -3"
  fi
  [[ "$(shasum -a 256 "$guest_elf" | cut -c1-64)" == "$GUEST_ELF_SHA256" ]] || fail "escrow guest ELF digest mismatch"

  # the LEZ v0.2 sidecar, the vault-claim tool and the identity tool (link libpython3.12)
  if [[ ! -x "$PROVISION/sidecar/lez-v02-bridge-poc" || ! -x "$PROVISION/sidecar/lez-v02-vault-claim-poc" || ! -x "$PROVISION/sidecar/lez-v02-local-actor-identity" ]]; then
    log "building the LEZ sidecar and its tools"
    builder_run -- "CARGO_TARGET_DIR=/cache/target/sidecar cargo +1.96.0 build --locked --manifest-path compat/lez-v0_2-sidecar/Cargo.toml \
        --bin lez-v02-bridge-poc --bin lez-v02-vault-claim-poc --example lez-v02-local-actor-identity 2>&1 | tail -2;
      mkdir -p /provision/sidecar; install -m 0755 /cache/target/sidecar/debug/lez-v02-bridge-poc /cache/target/sidecar/debug/lez-v02-vault-claim-poc \
        /cache/target/sidecar/debug/examples/lez-v02-local-actor-identity /provision/sidecar/"
  fi
  own_provision

  # persistent wallet identities the market and the LEZ genesis share
  local wallet
  for wallet in "${WALLETS[@]}"; do
    if [[ ! -f "$MARKET_ROOT/identities/$wallet/identity.json" ]]; then
      log "provisioning the $wallet identity"
      mkdir -p "$MARKET_ROOT/identities" && chmod 0700 "$MARKET_ROOT/identities"
      rm -rf "$MARKET_ROOT/identities/$wallet"
      docker run --rm -v "$PROVISION/sidecar:/tools:ro" -v "$MARKET_ROOT/identities:/identities" --user "$(id -u):$(id -g)" \
        "$BUILDER_IMAGE" /tools/lez-v02-local-actor-identity --output-directory "/identities/$wallet" >/dev/null
      cp "$MARKET_ROOT/identities/$wallet/identity.json" "$MARKET_ROOT/identities/$wallet.json"
    fi
  done
  log "artifacts built into $PROVISION"
}

# ---- stage ---------------------------------------------------------------------
phase_stage() {
  PHASE=stage
  install -m 0755 "$PROVISION/lez-services/sequencer_service" "$PROVISION/lez-services/indexer_service" \
    "$PROVISION/tools-arm/bin/r0vm" "$DEPLOY_ROOT/images/lez-services/"
  install -m 0755 "$PROVISION/sidecar/lez-v02-bridge-poc" "$DEPLOY_ROOT/images/maker-node/"
  install -m 0755 "$PROVISION/sidecar/lez-v02-bridge-poc" "$DEPLOY_ROOT/images/taker-node/"
  (cd "$DEPLOY_ROOT" && bash scripts/stage-assets.sh)
}

# ---- stack ---------------------------------------------------------------------
# One throwaway container on the stack's network runs the bootstrap: the
# deployer and the vault-claim tool accept only literal-loopback URLs, so the
# container forwards 127.0.0.1:3040/8779 to sequencer/indexer for the run.
market_bootstrap() {
  docker run --rm --network lez-swap-chains --user "$(id -u):$(id -g)" \
    -v "$PROVISION:/provision:ro" -v "$MARKET_ROOT:/market" -v "$DEPLOY_ROOT/scripts:/scripts:ro" \
    -e MARKET_ROOT=/market -e ESCROW_PROGRAM_ID="$ESCROW_PROGRAM_ID" \
    -e DEPLOYER=/provision/escrow-artifact/debug/lez-zec-escrow-v02-deployer \
    -e VAULT_CLAIM_BIN=/provision/sidecar/lez-v02-vault-claim-poc \
    "$BUILDER_IMAGE" bash -c 'socat TCP-LISTEN:3040,bind=127.0.0.1,fork,reuseaddr TCP:sequencer:3040 &
      socat TCP-LISTEN:8779,bind=127.0.0.1,fork,reuseaddr TCP:indexer:8779 &
      sleep 1; bash /scripts/market-bootstrap.sh'
}

phase_stack() {
  PHASE=stack
  cd "$DEPLOY_ROOT"
  # BuildKit resolves every FROM against its registry with a short deadline;
  # pull the base images first, with retries, so a slow registry cannot fail
  # the build of payloads that are already staged.
  local image
  for image in $(grep -h '^FROM' images/*/Dockerfile | awk '{print $2}' | sort -u); do
    docker image inspect "$image" >/dev/null 2>&1 && continue
    log "pulling $image"
    for _ in 1 2 3; do docker pull -q "$image" >/dev/null && break; sleep 10; done
    docker image inspect "$image" >/dev/null 2>&1 || fail "cannot pull $image"
  done
  log "gen-config, image builds, and stack start"
  local attempt
  for attempt in 1 2 3; do
    LEZ_MARKET_ROOT="$MARKET_ROOT" LEZ_WALLET_IDENTITIES="$MARKET_ROOT/identities" \
      SKIP_UI_VERIFY=1 bash scripts/up.sh && break
    [[ "$attempt" -lt 3 ]] || fail "up.sh did not succeed in three attempts"
    log "up.sh failed (registry deadline?); retrying in 30 s"
    sleep 30
  done
  set -a; source runtime/runtime.env; set +a
  bash scripts/repair-indexer.sh
  log "market bootstrap (escrow program, vault claims, bootstrap manifest)"
  market_bootstrap | tail -4
  log "Basecamp suites against both Nodes (the Maker suite also seeds the order book)"
  local role
  for role in maker taker; do
    docker exec lez-basecamp-ui node /ui-tests/verify.mjs "$role" 2>&1 | grep -E '✓|✗|passed' || fail "$role UI suite failed"
  done
  bash scripts/verify-all.sh 2>&1 | grep -E 'OK|FAIL|checks|failed' || fail "verify-all.sh reported a failed stage"
}

phase_swap() {
  PHASE=swap
  log "one full BTC → LEZ swap through the two Basecamp apps"
  bash "$DEPLOY_ROOT/scripts/swap-through-ui.sh"
}

for p in host sources nix rust build stage stack; do
  phase_wanted "$p" && "phase_$p"
done
if [[ "$RUN_SWAP" == 1 ]] || [[ "$ONLY" == swap ]]; then phase_swap; fi
log "done"
