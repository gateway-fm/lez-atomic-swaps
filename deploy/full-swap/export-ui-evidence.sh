#!/usr/bin/env bash
# Export the public, secret-free evidence needed by the Basecamp M3 BTC view.
# LEZ_UI_EVIDENCE_DIRECTION selects the swap route (taker_sells_foreign or
# taker_sells_lez); the forward route remains the default.
set -euo pipefail

if [[ $# -lt 1 || $# -gt 2 ]]; then
  echo "usage: $0 <m3-evidence-directory> [output-json]" >&2
  exit 64
fi

evidence_dir="$(cd "$1" && pwd -P)"
output="${2:-$(dirname "$0")/m3-btc-ui-evidence.json}"
direction="${LEZ_UI_EVIDENCE_DIRECTION:-taker_sells_foreign}"
case "$direction" in
  taker_sells_foreign) ui_direction="TakerSellsForeign" ;;
  taker_sells_lez) ui_direction="TakerSellsLez" ;;
  *)
    echo "LEZ_UI_EVIDENCE_DIRECTION must be taker_sells_foreign or taker_sells_lez" >&2
    exit 64
    ;;
esac

main="$evidence_dir/m3-actor-local-poc.json"
effects="$evidence_dir/${direction}-actual-effects.json"
btc_lock="$evidence_dir/${direction}-bitcoin-lock-confirmed.json"
btc_anchor="$evidence_dir/${direction}-bitcoin-funding-anchor.json"
lez_init="$evidence_dir/${direction}-lez-initialization-finality.json"
lez_funding="$evidence_dir/${direction}-lez-funding-finality.json"

if [[ "$direction" == "taker_sells_foreign" ]]; then
  btc_claim="$evidence_dir/${direction}-bitcoin-followup-claim-confirmed.json"
  lez_claim="$evidence_dir/${direction}-lez-revealing-claim-finality.json"
else
  # Taker sells LEZ: the Taker locks LEZ first, the Maker locks Bitcoin
  # second, the Taker reveals on Bitcoin, and the Maker follows up on LEZ.
  btc_claim="$evidence_dir/${direction}-bitcoin-revealing-claim-confirmed.json"
  lez_claim="$evidence_dir/${direction}-lez-followup-claim-finality.json"
fi

for file in "$main" "$effects" "$btc_lock" "$btc_anchor" "$btc_claim" \
  "$lez_init" "$lez_funding" "$lez_claim"; do
  [[ -f "$file" && ! -L "$file" ]] || {
    echo "missing or unsafe M3 evidence file: $file" >&2
    exit 1
  }
done

jq -e --arg direction "$direction" '
  .result == "passed"
  and .application.pair == "bitcoin"
  and .application.direction == $direction
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
  --slurpfile lez_claim "$lez_claim" \
  --arg ui_direction "$ui_direction" '
  def proof($id): "http://127.0.0.1:3003/#/evidence/tx/" + $id;
  def bitcoin_effect($seq; $actor; $kind; $label; $doc):
    {sequence:$seq, chain:"Bitcoin", actor:$actor, kind:$kind, label:$label,
     transaction_id:$doc.result.txid, amount:null,
     block_height:null, block_hash:$doc.result.blockhash,
     confirmations:$doc.result.confirmations, finality:"Confirmed",
     explorer_url:proof($doc.result.txid)};
  def lez_effect($seq; $actor; $kind; $label; $doc):
    {sequence:$seq, chain:"LEZ", actor:$actor, kind:$kind, label:$label,
     transaction_id:$doc.transaction_id, amount:null,
     block_height:$doc.containing_block_id, block_hash:$doc.containing_block_hash,
     confirmations:null, finality:$doc.bedrock_status,
     explorer_url:proof($doc.transaction_id)};
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
    direction: $ui_direction,
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
    effects: []
  }
  | if $ui_direction == "TakerSellsForeign" then .effects = [
      (bitcoin_effect(1; "Taker"; "first_lock"; "Taker first lock"; $btc_lock[0])
        | .amount = "0.01000000 BTC"
        | .block_height = $btc_anchor[0].containing_block_height),
      (lez_effect(2; "Maker"; "initialization"; "Escrow initialization"; $lez_init[0])
        | .amount = "Escrow authority"),
      (lez_effect(3; "Maker"; "funding"; "Maker second lock"; $lez_funding[0])
        | .amount = "1,000 LEZ units"),
      (lez_effect(4; "Taker"; "revealing_claim"; "Taker revealing claim"; $lez_claim[0])
        | .amount = "1,000 LEZ units"),
      (bitcoin_effect(5; "Maker"; "followup_claim"; "Maker follow-up claim"; $btc_claim[0])
        | .amount = "0.00999000 BTC")
    ] else .effects = [
      (lez_effect(1; "Taker"; "first_lock"; "Taker first lock"; $lez_funding[0])
        | .amount = "1,000 LEZ units"),
      (lez_effect(2; "Taker"; "initialization"; "Escrow initialization"; $lez_init[0])
        | .amount = "Escrow authority"),
      (bitcoin_effect(3; "Maker"; "second_lock"; "Maker second lock"; $btc_lock[0])
        | .amount = "0.01000000 BTC"
        | .block_height = $btc_anchor[0].containing_block_height),
      (bitcoin_effect(4; "Taker"; "revealing_claim"; "Taker revealing claim"; $btc_claim[0])
        | .amount = "0.00999000 BTC"),
      (lez_effect(5; "Maker"; "followup_claim"; "Maker follow-up claim"; $lez_claim[0])
        | .amount = "1,000 LEZ units")
    ] end
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
