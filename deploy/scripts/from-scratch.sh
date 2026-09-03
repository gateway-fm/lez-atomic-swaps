#!/usr/bin/env bash
# from-scratch.sh — bring the whole local LEZ ↔ BTC swap environment up from
# nothing, reproducibly, on an arm64 host (macOS with Docker Desktop, or Linux
# with Docker): prerequisites, pinned sources, every image payload, the
# external swap runner, the settlement market, the stack, and the Basecamp UI.
#
#   deploy/scripts/from-scratch.sh [--workspace DIR] [--swap] [--only PHASE]
#
# Every phase is idempotent: it checks what already exists and does only the
# missing work, so a rerun after a failure continues where it stopped.
#
#   host      docker, jq, git, curl, openssl, xxd (Homebrew on macOS)
#   sources   pinned checkouts next to this repo (the "workspace")
#   nix       Basecamp bundle, qt-mcp, both role packages, Chat + Delivery
#   rust      Maker/Taker Node binaries in the pinned rust image
#   runner    lez-runner-arm image + container, provisioned with LEZ services,
#             r0vm, rapidsnark, the escrow artifact, and warm cargo caches
#   stage     Bitcoin Core, LEZ services, r0vm into the image contexts
#   stack     gen-config → compose build → up → market bootstrap → UI suites
#   swap      (--swap) one full BTC → LEZ swap through the two Basecamp apps
#
# Long cold steps on Apple silicon: Nix closures (~15 min from the Logos cache),
# LEZ services (~40 min), r0vm (~30 min), cargo-risczero (~1.5 h), the guest
# ELF (~10 min). Everything is cached for the next run.
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
# LEZ program identity of the escrow guest this repository pins.
readonly ESCROW_PROGRAM_ID="${ESCROW_PROGRAM_ID:-b7f8727893174a29bd776eacbfdd9773e0510ebdac43102cb7e93ba4fa0b0433}"
# The runner scripts address these container paths; volumes keep them.
readonly RUNNER_SERVICES_DIR=/tmp/lez-v02-services-a58fbce2-20260713
readonly RUNNER_ARTIFACT_DIR=/tmp/lez-m3-artifact-arm
readonly RUNNER_TOOL_DIR=/tmp/lez-v02-provisional-tools
readonly RUNNER_TARGET_DIR=/tmp/lez-v02-provisional-target
readonly WALLETS=(maker-munich-01 maker-basel-02 taker-zurich-01 taker-limmat-02)

RUNNER_WORK="$WORKSPACE/runner-work"
RUNNER_REPO="$RUNNER_WORK/repo"
LEZ_SOURCE="$RUNNER_WORK/lez-source"
MARKET_ROOT="$RUNNER_WORK/market"
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
  mkdir -p "$RUNNER_WORK" "$PROVISION" "$MARKET_ROOT"
  chmod 0700 "$MARKET_ROOT"
  # The runner executes this repository's scripts and actors at the same commit
  # the images are built from.
  if [[ ! -d "$RUNNER_REPO/.git" ]]; then
    log "cloning this repository for the runner"
    git clone --quiet "$REPO_ROOT" "$RUNNER_REPO"
  fi
  local head; head="$(git -C "$REPO_ROOT" rev-parse HEAD)"
  if [[ "$(git -C "$RUNNER_REPO" rev-parse HEAD)" != "$head" ]]; then
    log "moving the runner checkout to $head"
    git -C "$RUNNER_REPO" fetch --quiet "$REPO_ROOT" "$head"
    git -C "$RUNNER_REPO" checkout --quiet --detach "$head"
  fi
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
    maker-node/lez-runtime-healthcheck taker-node/lez-taker-node taker-node/lez-taker-cli
    taker-node/lez-taker-chat-gateway taker-node/lez-taker-registry-init
    taker-node/lez-runtime-healthcheck lez-services/lez-runtime-healthcheck)
  local missing=0 b
  for b in "${bins[@]}"; do [[ -x "$DEPLOY_ROOT/images/$b" ]] || missing=1; done
  if [[ "$missing" == 1 ]]; then
    log "building the role Node binaries in $RUST_IMAGE"
    docker run --rm -v "$REPO_ROOT:/workspace" -v lez-rust-cache:/cache -w /workspace \
      -e CARGO_HOME=/cache/cargo-home -e CARGO_TARGET_DIR=/cache/target "$RUST_IMAGE" bash -c '
        set -e
        cargo build --locked -p lez-maker-node --bins -p lez-taker-node --bins -p lez-runtime-healthcheck 2>&1 | tail -3
        for b in '"${bins[*]}"'; do install -m 0755 "/cache/target/debug/${b#*/}" "deploy/images/$b"; done
        chown -R "$(stat -c %u:%g deploy)" deploy/images/maker-node deploy/images/taker-node deploy/images/lez-services'
  fi
  log "Node binaries staged"
}

# ---- runner --------------------------------------------------------------------
runner_exec() { docker exec -u lez "${RUNNER_ENV[@]}" lez-runner-arm bash -lc "$*"; }

phase_runner() {
  PHASE=runner
  if ! docker image inspect lez-runner-arm:latest >/dev/null 2>&1; then
    log "building the runner image"
    docker build -q -t lez-runner-arm:latest --build-arg UID="$(id -u)" --build-arg GID="$(id -g)" \
      -f "$DEPLOY_ROOT/full-swap/runner-arm.Dockerfile" "$DEPLOY_ROOT" >/dev/null
  fi
  if ! docker container inspect lez-runner-arm >/dev/null 2>&1; then
    log "starting the runner container"
    docker run -d --name lez-runner-arm --network host --group-add 0 \
      -v /var/run/docker.sock:/var/run/docker.sock \
      -v "$RUNNER_WORK:$RUNNER_WORK" -v "$PROVISION:/provision" \
      -v lez-runner-cargo-registry:/home/lez/.cargo/registry \
      -v lez-runner-cargo-git:/home/lez/.cargo/git \
      -v lez-runner-docker:/home/lez/.docker \
      -v "lez-runner-services:$RUNNER_SERVICES_DIR" \
      -v "lez-runner-artifact:$RUNNER_ARTIFACT_DIR" \
      -v "lez-runner-tools:$RUNNER_TOOL_DIR" \
      -v "lez-runner-target:$RUNNER_TARGET_DIR" \
      lez-runner-arm:latest sleep infinity >/dev/null
    docker exec lez-runner-arm bash -c "chown -R lez /home/lez/.cargo /home/lez/.docker \
      $RUNNER_SERVICES_DIR $RUNNER_ARTIFACT_DIR $RUNNER_TOOL_DIR $RUNNER_TARGET_DIR /provision"
  fi
  docker start lez-runner-arm >/dev/null
  RUNNER_ENV=(-e LEZ_V02_SOURCE_DIR="$LEZ_SOURCE" -e RAPIDSNARK_LIB_DIR=/provision/rapidsnark-arm
    -e BINDGEN_EXTRA_CLANG_ARGS=-I/usr/lib/gcc/aarch64-linux-gnu/13/include
    -e LEZ_V02_NATIVE_R0VM=/provision/tools-arm/bin/r0vm)

  # rapidsnark prover libraries (the Logos fork's aarch64 release)
  if [[ ! -f "$PROVISION/rapidsnark-arm/librapidsnark.a" ]]; then
    log "fetching rapidsnark aarch64 libraries"
    runner_exec "set -e; tmp=\$(mktemp -d); curl -fsSL '$RAPIDSNARK_URL' -o \$tmp/r.zip; unzip -qo \$tmp/r.zip -d \$tmp/r;
      mkdir -p /provision/rapidsnark-arm; cp \$(find \$tmp/r -name '*.a') /provision/rapidsnark-arm/; rm -rf \$tmp"
  fi
  runner_exec "printf '%s  %s\n' $RAPIDSNARK_LIB_SHA256 /provision/rapidsnark-arm/librapidsnark.a | sha256sum --check --strict --quiet"

  # LEZ v0.2 services, native release build with rust 1.94.0
  if [[ ! -x "$PROVISION/lez-services/sequencer_service" || ! -x "$PROVISION/lez-services/indexer_service" ]]; then
    log "building LEZ v0.2 services (rust 1.94.0, release, locked; ~40 min cold)"
    runner_exec "set -e; cd '$LEZ_SOURCE'; CARGO_TARGET_DIR=/provision/build-arm cargo +1.94.0 build --locked --release \
      --package sequencer_service --package indexer_service 2>&1 | tail -2;
      mkdir -p /provision/lez-services; install -m 0755 /provision/build-arm/release/{sequencer_service,indexer_service} /provision/lez-services/"
  fi
  runner_exec "mkdir -p $RUNNER_SERVICES_DIR/release && cp /provision/lez-services/{sequencer_service,indexer_service} $RUNNER_SERVICES_DIR/release/"

  # r0vm from the risc0 v3.0.5 tag (no arm64 release asset exists)
  if [[ ! -x "$PROVISION/tools-arm/bin/r0vm" ]]; then
    log "building r0vm from risc0 $RISC0_TAG (~30 min cold)"
    runner_exec "set -e; [[ -d /provision/risc0/.git ]] || git clone --quiet --depth 1 --branch $RISC0_TAG https://github.com/risc0/risc0.git /provision/risc0;
      cd /provision/risc0 && CARGO_TARGET_DIR=$RUNNER_TARGET_DIR/r0vm cargo +1.96.0 install --path risc0/r0vm --locked --root /provision/tools-arm 2>&1 | tail -2"
  fi
  [[ "$(runner_exec '/provision/tools-arm/bin/r0vm --version')" == "risc0-r0vm 3.0.5" ]] || fail "r0vm is not 3.0.5"

  # buildx: risc0's Docker guest build exports with `docker build --output`
  if ! runner_exec 'docker buildx version >/dev/null 2>&1'; then
    log "installing the docker buildx plugin in the runner"
    runner_exec 'set -e; tag=$(curl -fsSL https://api.github.com/repos/docker/buildx/releases/latest | sed -n "s/.*\"tag_name\": *\"\([^\"]*\)\".*/\1/p" | head -1);
      mkdir -p ~/.docker/cli-plugins; curl -fsSL "https://github.com/docker/buildx/releases/download/$tag/buildx-$tag.linux-arm64" -o ~/.docker/cli-plugins/docker-buildx; chmod +x ~/.docker/cli-plugins/docker-buildx'
  fi
  # risc0-build asks rzup for a default Rust toolchain even for Docker guest builds
  runner_exec 'mkdir -p /tmp/lez-risc0-home/toolchains/v1.94.1-rust-aarch64-unknown-linux-gnu && printf "[default_versions]\nrust = \"1.94.1\"\n" > /tmp/lez-risc0-home/settings.toml'

  # the escrow artifact: deployer + digest-checked guest ELF
  if ! runner_exec "test -x $RUNNER_ARTIFACT_DIR/debug/lez-zec-escrow-v02-deployer && test -f $RUNNER_ARTIFACT_DIR/riscv-guest/lez-zec-escrow-v02-methods/lez-zec-escrow-v02-guest/riscv32im-risc0-zkvm-elf/docker/zec_escrow_v02.bin"; then
    log "building the escrow artifact (cargo-risczero from source, then the pinned guest; ~1.5 h cold)"
    runner_exec "set -e; cd '$RUNNER_REPO'; rm -rf $RUNNER_ARTIFACT_DIR/docker-guest-source;
      RUN_ID=arm-rebuild LEZ_NATIVE_TOOLS=1 LEZ_V02_ARTIFACT_TARGET_DIR=$RUNNER_ARTIFACT_DIR LEZ_V02_TOOL_DIR=$RUNNER_TOOL_DIR \
      CARGO_TARGET_DIR=$RUNNER_TARGET_DIR CARGO_BUILD_JOBS=6 scripts/verify-lez-v02-provisional.sh 2>&1 | tail -3"
  fi

  # cargo caches: the swap runs build offline
  log "warming the runner's cargo caches"
  runner_exec "set -e; cd '$RUNNER_REPO'; cargo fetch --locked -q; cargo fetch --locked -q --manifest-path compat/lez-v0_2-sidecar/Cargo.toml;
    cargo +1.96.0 build -q --locked --offline -p btc-local-poc-provision -p btc-reference-actor -p lez-adaptor-role-runner --bins;
    cargo +1.96.0 build -q --locked --offline -p lez-maker-node --bins;
    cargo +1.96.0 build -q --locked --offline -p lez-btc-swap-sdk --example btc-core-p2tr-fixture;
    cargo +1.96.0 build -q --locked --offline -p lez-bridge-client --example m3_witnessed_lez_operator;
    cargo +1.96.0 build -q --locked --offline --manifest-path compat/lez-v0_2-sidecar/Cargo.toml --bin lez-v02-bridge-poc --bin lez-v02-vault-claim-poc \
      --bin lez-v02-native-escrow-poc --example lez-v02-local-actor-identity --example lez-v02-account-id"
  runner_exec "cd '$LEZ_SOURCE' && cargo fetch --locked -q"

  # persistent wallet identities the market and the LEZ genesis share
  local wallet
  for wallet in "${WALLETS[@]}"; do
    if [[ ! -f "$MARKET_ROOT/identities/$wallet/identity.json" ]]; then
      log "provisioning the $wallet identity"
      runner_exec "mkdir -m 0700 -p '$MARKET_ROOT/identities' && rm -rf '$MARKET_ROOT/identities/$wallet' &&
        '$RUNNER_REPO/compat/lez-v0_2-sidecar/target/debug/examples/lez-v02-local-actor-identity' --output-directory '$MARKET_ROOT/identities/$wallet' >/dev/null &&
        cp '$MARKET_ROOT/identities/$wallet/identity.json' '$MARKET_ROOT/identities/$wallet.json'"
    fi
  done
  log "runner provisioned"
}

# ---- stage ---------------------------------------------------------------------
phase_stage() {
  PHASE=stage
  install -m 0755 "$PROVISION/lez-services/sequencer_service" "$PROVISION/lez-services/indexer_service" \
    "$PROVISION/tools-arm/bin/r0vm" "$DEPLOY_ROOT/images/lez-services/"
  (cd "$DEPLOY_ROOT" && bash scripts/stage-assets.sh)
}

# ---- stack ---------------------------------------------------------------------
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
    LEZ_M3_RUNNER_REPO="$RUNNER_REPO" LEZ_WALLET_IDENTITIES="$MARKET_ROOT/identities" \
      SKIP_UI_VERIFY=1 bash scripts/up.sh && break
    [[ "$attempt" -lt 3 ]] || fail "up.sh did not succeed in three attempts"
    log "up.sh failed (registry deadline?); retrying in 30 s"
    sleep 30
  done
  set -a; source runtime/runtime.env; set +a
  bash scripts/repair-indexer.sh
  log "market bootstrap (escrow program, vault claims, attach manifests)"
  docker cp scripts/market-bootstrap.sh lez-runner-arm:/tmp/lez-market-bootstrap.sh
  docker exec -e REPO_ROOT="$RUNNER_REPO" -e MARKET_ROOT="$MARKET_ROOT" \
    -e BTC_RPC_PASSWORD="$BTC_RPC_PASSWORD" -e ESCROW_PROGRAM_ID="$ESCROW_PROGRAM_ID" \
    -e DEPLOYER="$RUNNER_ARTIFACT_DIR/debug/lez-zec-escrow-v02-deployer" \
    lez-runner-arm bash /tmp/lez-market-bootstrap.sh | tail -4
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

for p in host sources nix rust runner stage stack; do
  phase_wanted "$p" && "phase_$p"
done
if [[ "$RUN_SWAP" == 1 ]] || [[ "$ONLY" == swap ]]; then phase_swap; fi
log "done"
