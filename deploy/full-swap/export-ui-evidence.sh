#!/usr/bin/env bash
# Export the public, secret-free evidence needed by the Basecamp M3 BTC view.
set -euo pipefail

if [[ $# -lt 1 || $# -gt 2 ]]; then
  echo "usage: $0 <m3-evidence-directory> [output-json]" >&2
  exit 64
fi

evidence_dir="$(cd "$1" && pwd -P)"
output="${2:-$(dirname "$0")/m3-btc-ui-evidence.json}"
direction=taker_sells_foreign

main="$evidence_dir/m3-actor-local-poc.json"
effects="$evidence_dir/${direction}-actual-effects.json"
btc_lock="$evidence_dir/${direction}-bitcoin-lock-confirmed.json"
btc_anchor="$evidence_dir/${direction}-bitcoin-funding-anchor.json"
btc_claim="$evidence_dir/${direction}-bitcoin-followup-claim-confirmed.json"
lez_init="$evidence_dir/${direction}-lez-initialization-finality.json"
lez_funding="$evidence_dir/${direction}-lez-funding-finality.json"
lez_claim="$evidence_dir/${direction}-lez-revealing-claim-finality.json"

for file in "$main" "$effects" "$btc_lock" "$btc_anchor" "$btc_claim" \
  "$lez_init" "$lez_funding" "$lez_claim"; do
  [[ -f "$file" && ! -L "$file" ]] || {
    echo "missing or unsafe M3 evidence file: $file" >&2
    exit 1
  }
done

jq -e '
  .result == "passed"
  and .application.pair == "bitcoin"
  and .application.direction == "taker_sells_foreign"
  and .directions[0].terminal_phase == "completed"
  and .directions[0].terminal_revision == 4
  and .private_material_disclosed == false
' "$main" >/dev/null

jq -e '
  (.bitcoin_effect_ids | length) == 2
  and (.lez_effect_ids | length) == 3
  and .expected_unique_effects == {bitcoin:2,lez:3}
' "$effects" >/dev/null

output_parent="$(cd "$(dirname "$output")" && pwd -P)"
tmp="$(mktemp "$output_parent/.m3-btc-ui-evidence.XXXXXX")"
trap 'rm -f "$tmp"' EXIT

jq -n \
  --slurpfile main "$main" \
  --slurpfile effects "$effects" \
  --slurpfile btc_lock "$btc_lock" \
  --slurpfile btc_anchor "$btc_anchor" \
  --slurpfile btc_claim "$btc_claim" \
  --slurpfile lez_init "$lez_init" \
  --slurpfile lez_funding "$lez_funding" \
  --slurpfile lez_claim "$lez_claim" '
  def proof($id): "http://127.0.0.1:3003/#/evidence/tx/" + $id;
  {
    schema_version: 1,
    kind: "m3_btc_ui_evidence",
    source_kind: $main[0].kind,
    source: "certified_local_run_snapshot",
    result: $main[0].result,
    run_id: $main[0].run_id,
    completed_at: $main[0].completed_at,
    repository_commit: $main[0].repository_commit,
    pair: "Bitcoin",
    direction: "TakerSellsForeign",
    journey: $main[0].journey,
    terminal: {
      phase: $main[0].directions[0].terminal_phase,
      revision: $main[0].directions[0].terminal_revision
    },
    amounts: {
      bitcoin_sats: 1000000,
      bitcoin_display: "0.01000000 BTC",
      lez_units: 1000,
      lez_display: "1,000 LEZ units"
    },
    networks: {
      bitcoin: "Bitcoin Core 31.1 · regtest",
      lez: "LEZ v0.2.0 · private local"
    },
    effect_counts: {
      bitcoin: ($effects[0].bitcoin_effect_ids | length),
      lez: ($effects[0].lez_effect_ids | length),
      total: (($effects[0].bitcoin_effect_ids | length) + ($effects[0].lez_effect_ids | length))
    },
    replay_resubmission_count: $main[0].replay_resubmission_count,
    private_material_disclosed: $main[0].private_material_disclosed,
    stage_two_evidence_sha256: $main[0].directions[0].stage_two_evidence_sha256,
    effects: [
      {
        sequence: 1, chain: "Bitcoin", actor: "Taker", kind: "first_lock",
        label: "Taker first lock", transaction_id: $btc_lock[0].result.txid,
        amount: "0.01000000 BTC", block_height: $btc_anchor[0].containing_block_height,
        block_hash: $btc_lock[0].result.blockhash,
        confirmations: $btc_lock[0].result.confirmations, finality: "Confirmed",
        explorer_url: proof($btc_lock[0].result.txid)
      },
      {
        sequence: 2, chain: "LEZ", actor: "Maker", kind: "initialization",
        label: "Escrow initialization", transaction_id: $lez_init[0].transaction_id,
        amount: "Escrow authority", block_height: $lez_init[0].containing_block_id,
        block_hash: $lez_init[0].containing_block_hash,
        confirmations: null, finality: $lez_init[0].bedrock_status,
        explorer_url: proof($lez_init[0].transaction_id)
      },
      {
        sequence: 3, chain: "LEZ", actor: "Maker", kind: "funding",
        label: "Maker second lock", transaction_id: $lez_funding[0].transaction_id,
        amount: "1,000 LEZ units", block_height: $lez_funding[0].containing_block_id,
        block_hash: $lez_funding[0].containing_block_hash,
        confirmations: null, finality: $lez_funding[0].bedrock_status,
        explorer_url: proof($lez_funding[0].transaction_id)
      },
      {
        sequence: 4, chain: "LEZ", actor: "Taker", kind: "revealing_claim",
        label: "Taker revealing claim", transaction_id: $lez_claim[0].transaction_id,
        amount: "1,000 LEZ units", block_height: $lez_claim[0].containing_block_id,
        block_hash: $lez_claim[0].containing_block_hash,
        confirmations: null, finality: $lez_claim[0].bedrock_status,
        explorer_url: proof($lez_claim[0].transaction_id)
      },
      {
        sequence: 5, chain: "Bitcoin", actor: "Maker", kind: "followup_claim",
        label: "Maker follow-up claim", transaction_id: $btc_claim[0].result.txid,
        amount: "0.00999000 BTC", block_height: null,
        block_hash: $btc_claim[0].result.blockhash,
        confirmations: $btc_claim[0].result.confirmations, finality: "Confirmed",
        explorer_url: proof($btc_claim[0].result.txid)
      }
    ]
  }
' >"$tmp"

jq -e '
  .kind == "m3_btc_ui_evidence"
  and .result == "passed"
  and .terminal == {phase:"completed",revision:4}
  and (.effects | length) == 5
  and ([.effects[].transaction_id] | unique | length) == 5
  and ([.effects[] | select(.chain == "Bitcoin")] | length) == 2
  and ([.effects[] | select(.chain == "LEZ")] | length) == 3
  and ([.effects[] | select(.finality == "Confirmed" or .finality == "Finalized")] | length) == 5
  and .private_material_disclosed == false
' "$tmp" >/dev/null

chmod 0644 "$tmp"
mv "$tmp" "$output"
trap - EXIT
printf 'wrote %s\n' "$output"
