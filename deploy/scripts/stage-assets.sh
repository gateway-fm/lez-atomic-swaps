#!/usr/bin/env bash
# stage-assets.sh — reconstruct the binary payloads that are not committed
# (see .gitignore). Each block documents the exact provenance. Run from
# deploy/ on the arm64 host; adjust source paths if your build tree differs.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

log() { printf '[stage %s] %s\n' "$(date -u +%H:%M:%S)" "$*"; }

# ---- bitcoin-core/bin: official Core 31.1 aarch64 release archive ----------
if [[ ! -x images/bitcoin-core/bin/bitcoind ]]; then
    log "fetching Bitcoin Core 31.1 aarch64"
    tmp="$(mktemp -d /tmp/lez-stage-btc.XXXXXX)"
    curl -fsSL -o "$tmp/btc.tar.gz" \
        https://bitcoincore.org/bin/bitcoin-core-31.1/bitcoin-31.1-aarch64-linux-gnu.tar.gz
    mkdir -p images/bitcoin-core/bin
    tar -xzf "$tmp/btc.tar.gz" -C "$tmp" \
        bitcoin-31.1/bin/bitcoind bitcoin-31.1/bin/bitcoin-cli
    mv "$tmp"/bitcoin-31.1/bin/* images/bitcoin-core/bin/
    rm -rf "$tmp"
fi

# ---- lez-services: pinned logos-execution-zone v0.2.0 (a58fbce) ------------
# Native arm64 release build with rust 1.94.0 and the Logos rapidsnark fork's
# rapidsnark-linux-aarch64-pic-v0.0.8 prebuilt libs (RAPIDSNARK_LIB_DIR).
# r0vm is built from the risc0 v3.0.5 git tag (no arm64-linux release asset).
if [[ ! -x images/lez-services/sequencer_service ]]; then
    cat >&2 <<'MISSING'
Build LEZ v0.2 services natively first (see deploy/README "Native-arm64 notes"):
  git clone --branch v0.2.0 https://github.com/logos-blockchain/logos-execution-zone.git
  cargo +1.94.0 build --locked --release -p sequencer_service -p indexer_service
  cargo install --path risc0/r0vm --locked   # from the risc0 v3.0.5 tag
Then copy:
  <target>/release/sequencer_service  -> images/lez-services/
  <target>/release/indexer_service    -> images/lez-services/
  <r0vm>                              -> images/lez-services/
MISSING
    exit 1
fi

# ---- role Nodes: separate native-arm64 image payloads ---------------------
if [[ ! -x images/maker-node/lez-maker-node \
   || ! -x images/maker-node/lez-maker-cli \
   || ! -x images/maker-node/lez-maker-chat-gateway \
   || ! -x images/maker-node/lez-runtime-healthcheck \
   || ! -x images/taker-node/lez-taker-node \
   || ! -x images/taker-node/lez-taker-cli \
   || ! -x images/taker-node/lez-taker-chat-gateway \
   || ! -x images/taker-node/lez-taker-registry-init \
   || ! -x images/taker-node/lez-runtime-healthcheck \
   || ! -x images/lez-services/lez-runtime-healthcheck ]]; then
    cat >&2 <<'MISSING'
Build the workspace binaries natively (repo root):
  cargo build --locked -p lez-maker-node --bins -p lez-taker-node --bins \
    -p lez-runtime-healthcheck
Then copy target/debug/{lez-maker-cli,lez-maker-node,lez-maker-chat-gateway,
lez-runtime-healthcheck} -> images/maker-node/ and
target/debug/{lez-taker-cli,lez-taker-node,lez-taker-chat-gateway,
lez-taker-registry-init,lez-runtime-healthcheck} -> images/taker-node/.
Also copy target/debug/lez-runtime-healthcheck -> images/lez-services/.
MISSING
    exit 1
fi

# ---- basecamp-ui/assets: nix outputs (flake, aarch64-linux) ----------------
if [[ ! -d images/basecamp-ui/assets/bundle ]]; then
    cat >&2 <<'MISSING'
Build with the pinned flakes (nix, experimental-features enabled):
  nix build path:../basecamp#bin-bundle-dir-inspector -o bundle
  nix build path:apps/basecamp#lez-maker-ui-install -o maker-user
  nix build path:apps/basecamp#lez-taker-ui-install -o taker-user
  nix build path:../basecamp#logos-qt-mcp -o qt-mcp
  nix build github:logos-co/logos-chat-module/v0.2.2#lgx -o chat-lgx
  nix build github:logos-co/logos-chat-module/v0.2.2#delivery_module-lgx -o delivery-lgx
Then copy:
  bundle/     -> images/basecamp-ui/assets/bundle
  qt-mcp/     -> images/basecamp-ui/assets/qt-mcp
and stage the role packages plus the modules they depend on (this pins the
variant tag):
  scripts/stage-basecamp-package.sh maker-user maker
  scripts/stage-basecamp-package.sh taker-user taker
  scripts/stage-basecamp-package.sh chat-lgx/*.lgx module
  scripts/stage-basecamp-package.sh delivery-lgx/*.lgx module
MISSING
    exit 1
fi

log "all payloads staged"
