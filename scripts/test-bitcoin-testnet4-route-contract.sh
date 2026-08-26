#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly manifest="crates/btc-core-adapter/Cargo.toml"
readonly adapter="crates/btc-core-adapter/src/lib.rs"
readonly transport="crates/btc-core-adapter/src/http.rs"

rg -Fqx "jsonrpsee-http-client = { version = \"=0.26.0\", default-features = false, features = [\"tls\"] }" \
  "$manifest"
rg -Fq "Testnet4Networked" "$adapter"
rg -Fq "Network::Testnet4" "$adapter"
rg -Fq "CoreRpcRoute::ExactHttpsBasic" "$adapter"
rg -Fq "ensure_route_compatible" "$adapter"
rg -Fq "new_exact_https_basic_gateway" "$transport"
rg -Fq "connect_profiled" "$transport"
rg -Fq "NonAllowlistedHttpsEndpoint" "$transport"

cargo test --locked -p lez-btc-core-adapter --test core_contract \
  testnet4_profile_requires_exact_chain_network_and_pinned_genesis
cargo test --locked -p lez-btc-core-adapter --test http_security \
  testnet4_exact_https_

echo "Bitcoin Testnet4 route contracts passed without public network calls"
