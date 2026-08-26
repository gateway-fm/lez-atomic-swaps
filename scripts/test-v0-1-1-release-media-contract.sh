#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

fail() {
  echo "v0.1.1 release-media compatibility failed: $*" >&2
  exit 1
}

hash_file() {
  local path="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$path" | awk '{print $1}'
  else
    shasum -a 256 "$path" | awk '{print $1}'
  fi
}

assert_release_asset() {
  local path="$1"
  local expected="$2"
  local actual
  [[ -f "$path" && ! -L "$path" ]] || fail "missing or unsafe asset: ${path}"
  actual="$(hash_file "$path")"
  [[ "$actual" == "$expected" ]] || fail "published asset changed: ${path}"
}

assert_release_asset media/lez-btc-m1-m3-m6-submission.html \
  5c90bfa301d43c7aadb072ad5e8e87e0476666d449af33aee199f37ca09ef233
assert_release_asset media/lez-btc-m1-m3-m6-submission.pdf \
  d084df2cec8d23cb9982c0e237f81d9c882554b78d059b308f0d001c17454637
assert_release_asset media/lez-btc-ui-swap-demo.mp4 \
  bb5ec36055e5f8a53f367d6bf03147e4eb78f17a601d305260b30d8229275703
assert_release_asset media/lez-btc-ui-swap-demo.en.vtt \
  ea89663fc45c8fd2881b28fc509841eca86d5739ea16086b1fda1694b353b49c

readonly evidence=docs/evidence/m3-btc-ui-run-m5arm-0825151914.json
jq -e '
  .schema_version == 1
  and .kind == "m3_btc_ui_evidence"
  and .result == "passed"
  and .run_id == "m5arm-0825151914"
  and .source_kind == "m5_btc_application_local_poc"
  and .private_material_disclosed == false
  and .journey == "claim"
  and .direction == "TakerSellsForeign"
  and .pair == "Bitcoin"
  and .amounts.bitcoin_sats == 1000000
  and .amounts.lez_units == 1000
  and .effect_counts == {bitcoin:2,lez:3,total:5}
  and .replay_resubmission_count == 0
  and .terminal == {phase:"completed",revision:4}
  and [.effects[] | [.chain,.kind]] == [
    ["Bitcoin","first_lock"],
    ["LEZ","initialization"],
    ["LEZ","funding"],
    ["LEZ","revealing_claim"],
    ["Bitcoin","followup_claim"]
  ]
' "$evidence" >/dev/null || fail "walkthrough evidence no longer proves its claims"

readonly deck=media/lez-btc-m1-m3-m6-submission.html
for claim in \
  'LEZ ⇄ Bitcoin' \
  'Broadcast to discover. Chat to negotiate.' \
  '/lez-atomic-swaps/1/offers/json' \
  'BIP-341' \
  'claim_native_witnessed' \
  'refund_native' \
  'Basecamp experience'; do
  rg -Fq -- "$claim" "$deck" || fail "published deck claim is missing: ${claim}"
done

for source_contract in \
  'crates/btc-swap-sdk/src/p2tr.rs:verify_taproot_commitment' \
  'crates/btc-swap-sdk/src/sdk.rs:verify_adaptor_presignature' \
  'compat/lez-v0.2-provisional/escrow/src/lib.rs:claim_native_witnessed' \
  'compat/lez-v0.2-provisional/escrow/src/lib.rs:refund_native' \
  'crates/maker-node/src/run_local_delivery.rs:/lez-atomic-swaps/1/offers/json'; do
  path="${source_contract%%:*}"
  term="${source_contract#*:}"
  [[ -f "$path" ]] || fail "deck implementation source is missing: ${path}"
  rg -Fq -- "$term" "$path" || fail "deck implementation term is missing: ${term}"
done

for caption in \
  'The Maker publishes exact terms' \
  'The Taker locks the exact joint P2TR Bitcoin output' \
  'Only after those Bitcoin checks pass does the Maker fund 1,000 LEZ' \
  'Only after those LEZ checks pass does the Taker claim LEZ' \
  'The Maker claims the fixed Bitcoin payout' \
  'five actions, zero replays'; do
  rg -Fq -- "$caption" media/lez-btc-ui-swap-demo.en.vtt \
    || fail "walkthrough caption contract is missing: ${caption}"
done

echo "v0.1.1 deck and walkthrough remain compatible with the current BTC/Basecamp implementation"
