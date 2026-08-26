#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly source="crates/xmr-swap-sdk/src/sdk.rs"
readonly example="crates/xmr-swap-sdk/examples/lifecycle-wiring.rs"

fail() {
  echo "M7 XMR SDK facade contract failed: $*" >&2
  exit 1
}

[[ -s "$source" ]] || fail "missing ${source}"
[[ -s "$example" ]] || fail "missing ${example}"

for literal in \
  'pub struct XmrPairSdk' \
  'pub struct ActiveXmrSwap' \
  'pub struct XmrLifecycleIdentityV1' \
  'pub trait XmrRoleActorPort' \
  'pub struct XmrNegotiationEnvelopeV1' \
  'pub enum XmrLifecycleCommandV1' \
  'pub enum XmrLifecyclePhaseV1' \
  'pub async fn publish_offer' \
  'pub async fn discover_offers' \
  'pub async fn negotiate' \
  'pub async fn activate' \
  'pub async fn resume' \
  'pub async fn claim' \
  'pub async fn refund'; do
  rg -Fq "$literal" "$source" || fail "missing public lifecycle token: ${literal}"
done

rg -Fq 'Role-fixed actor owns durable effect journals' "$source" \
  || fail "actor durability boundary is undocumented"
rg -Fq 'no discovery or negotiation capability' "$source" \
  || fail "post-lock transport erasure is undocumented"
rg -Fq 'lifecycle-wiring' crates/xmr-swap-sdk/Cargo.toml \
  || fail "external-wiring example is not an explicit Cargo target"
rg -Fq './scripts/test-m7-xmr-sdk-facade-contract.sh' scripts/run-ci-quality-gates.sh \
  || fail "CI quality gates do not run this contract"

echo "M7 XMR public lifecycle facade contract passed"
