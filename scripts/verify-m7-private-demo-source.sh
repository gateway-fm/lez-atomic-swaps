#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

fail() {
  echo "M7 private demo source verification failed: $*" >&2
  exit 1
}

(( $# == 2 )) || fail "provide pair and scenario"
readonly pair="$1" scenario="$2"
readonly source_map="docs/evidence/m7-private-demo-sources.json"

command -v jq >/dev/null || fail "jq is required"
command -v sha256sum >/dev/null || fail "sha256sum is required"
[[ "$pair" == xmr || "$pair" == zec ]] || fail "pair must be xmr or zec"
[[ "$scenario" == happy || "$scenario" == refund || "$scenario" == concurrent ]] ||
  fail "scenario must be happy, refund, or concurrent"
[[ -f "$source_map" && ! -L "$source_map" ]] || fail "source map is missing or unsafe"

entry="$(jq -ce --arg pair "$pair" --arg scenario "$scenario" '
  select(
    .schema_version == 1 and .kind == "m7_private_demo_source_map" and
    .privacy == "private_local_stealth" and
    .runtime_external_resources == {public_rpc:false,public_peer:false,faucet:false,
      public_funds:false,public_deployment:false} and
    (.entries | length) == 6 and
    ([.entries[] | [.pair,.scenario] | join(":")] | unique | length) == 6
  )
  | [.entries[] | select(.pair == $pair and .scenario == $scenario)] as $matches
  | select(($matches | length) == 1)
  | $matches[0]
' "$source_map")" || fail "source-map entry is invalid or missing"

while IFS=$'\t' read -r path expected_sha; do
  [[ "$path" =~ ^docs/evidence/[a-zA-Z0-9._-]+[.]json$ ]] || fail "unsafe evidence path"
  [[ -f "$path" && ! -L "$path" ]] || fail "evidence source is missing: ${path}"
  [[ "$(sha256sum "$path" | cut -d ' ' -f 1)" == "$expected_sha" ]] ||
    fail "evidence hash mismatch: ${path}"
done < <(jq -r '.source_files[] | [.path,.sha256] | @tsv' <<<"$entry")

case "${pair}:${scenario}" in
  xmr:happy) ./scripts/test-m7-taker-claim-actual-certificate.sh >/dev/null ;;
  xmr:refund) ./scripts/test-m7-maker-refund-process-kill-actual-certificate.sh >/dev/null ;;
  xmr:concurrent) ./scripts/test-m7-xmr-accepted-concurrency-actual-certificate.sh >/dev/null ;;
  zec:happy) ./scripts/test-m7-zec-accepted-process-kill-actual-certificate.sh >/dev/null ;;
  zec:refund) ./scripts/test-m7-zec-first-lock-refund-actual-certificate.sh >/dev/null ;;
  zec:concurrent) ./scripts/test-m7-zec-concurrent-demo-baseline.sh >/dev/null ;;
esac

jq -cS --argjson entry "$entry" --arg map_sha "$(sha256sum "$source_map" | cut -d ' ' -f 1)" '
  {schema_version:1,kind:"m7_private_demo_source_proof",result:"passed",
   source_map_sha256:$map_sha,entry:$entry,
   joined_actual_node_concurrency:($entry.evidence_model == "joined_actual_nodes")}
' <<<"{}"
