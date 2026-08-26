#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
vector_root="${repo_root}/crates/btc-swap-sdk/tests/vectors/bitcoin-bips-8c369ac8"
checksums="${vector_root}/SHA256SUMS"
provenance="${vector_root}/PROVENANCE.md"
adaptor_fixture="${repo_root}/crates/btc-swap-sdk/tests/vectors/lez-btc-adaptor-v1.json"
readonly adaptor_fixture_sha256="e3bee341393c482589bdf4a92d8e0096046430bc9e44f2741933fff866a76ab2"

fail() {
  echo "M3 cryptographic vector contract failed: $*" >&2
  exit 1
}

[[ -d "$vector_root" ]] || fail "missing immutable vector directory"
[[ -f "$checksums" && ! -L "$checksums" ]] || fail "missing regular SHA256SUMS"
[[ -f "$provenance" && ! -L "$provenance" ]] || fail "missing regular PROVENANCE.md"
[[ -f "$adaptor_fixture" && ! -L "$adaptor_fixture" ]] ||
  fail "missing regular swap-specific adaptor fixture"

expected_files=(
  PROVENANCE.md
  SHA256SUMS
  bip-0327/det_sign_vectors.json
  bip-0327/key_agg_vectors.json
  bip-0327/key_sort_vectors.json
  bip-0327/nonce_agg_vectors.json
  bip-0327/nonce_gen_vectors.json
  bip-0327/sig_agg_vectors.json
  bip-0327/sign_verify_vectors.json
  bip-0327/tweak_vectors.json
  bip-0340/LICENSE
  bip-0340/test-vectors.csv
)

mapfile -t actual_files < <(
  cd "$vector_root"
  find . -type f -print | sed 's#^\./##' | LC_ALL=C sort
)
mapfile -t sorted_expected < <(
  printf '%s\n' "${expected_files[@]}" | LC_ALL=C sort
)
[[ "${actual_files[*]}" == "${sorted_expected[*]}" ]] ||
  fail "vector directory contains missing or unreviewed files"

while IFS= read -r relative; do
  path="${vector_root}/${relative}"
  [[ -f "$path" && ! -L "$path" ]] || fail "${relative} is not a regular file"
done < <(printf '%s\n' "${expected_files[@]}")

(
  cd "$vector_root"
  sha256sum -c SHA256SUMS >/dev/null
) || fail "immutable vector checksum mismatch"

for path in "${vector_root}"/bip-0327/*.json; do
  jq -e . "$path" >/dev/null || fail "invalid JSON: ${path#"${vector_root}/"}"
done

[[ "$(find "${vector_root}/bip-0327" -maxdepth 1 -type f -name '*.json' | wc -l)" -eq 8 ]] ||
  fail "the complete eight-file BIP-327 corpus is required"
[[ "$(wc -l <"${vector_root}/bip-0340/test-vectors.csv")" -eq 20 ]] ||
  fail "the BIP-340 corpus must contain one header and nineteen vectors"

rg -Fq '8c369ac8e60629ac6c032ffe21bb5ec5b35213d7' "$provenance" ||
  fail "provenance is missing the immutable bitcoin/bips commit"
rg -Fq 'BIP-327: BSD-3-Clause' "$provenance" ||
  fail "provenance is missing the BIP-327 license"
rg -Fq 'BIP-340: BSD-2-Clause' "$provenance" ||
  fail "provenance is missing the BIP-340 license"

printf '%s  %s\n' "$adaptor_fixture_sha256" "$adaptor_fixture" |
  sha256sum -c - >/dev/null || fail "swap-specific adaptor fixture checksum mismatch"
jq -e '
  .schema_version == 1
  and .fixture_id == "lez-btc-taproot-adaptor-v1"
  and .license == "MIT OR Apache-2.0"
  and .public_fixture_only == true
  and .construction == "BIP327_MUSIG2_BIP340_ADAPTOR_BIP341_TAPROOT"
  and .role_order == ["maker", "taker"]
  and (.expected.adaptor_presignature | length) == 130
  and (.expected.final_signature | length) == 128
  and .expected.witness_items == 1
  and .expected.witness_bytes == 64
' "$adaptor_fixture" >/dev/null || fail "swap-specific adaptor fixture invariants failed"

echo "M3 official and swap-specific cryptographic vector checksums passed"
