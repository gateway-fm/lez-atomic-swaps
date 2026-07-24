#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

export LC_ALL=C
readonly runner="scripts/run-m2-taker-sells-lez-poc.sh"

fail() {
  echo "M5 application contract failed: $*" >&2
  exit 1
}

[[ -x scripts/run-m5-zec-application-poc.sh ]] || fail "M5 wrapper is not executable"
rg -Fq 'export M5_APPLICATION_MODE=1' scripts/run-m5-zec-application-poc.sh ||
  fail "M5 wrapper does not force application mode"

projection_function="$(sed -n \
  '/^prove_m5_terminal_operator_projection() {$/,/^}$/p' "$runner")"
[[ -n "$projection_function" ]] || fail "terminal operator projection function is missing"

for required in \
  '--terminal-zec-maker-state-db' \
  '--terminal-zec-swap-id' \
  '--terminal-zec-claim-key-id' \
  '--terminal-zec-claim-key-file' \
  'm5-history-after-terminal-restart.json' \
  'm5-status-after-terminal-restart.json' \
  'm5-terminal-operator-projection.json' \
  'chain_rpc_used_during_import: false' \
  'private_material_disclosed: false'; do
  rg -Fq -- "$required" <<<"$projection_function" ||
    fail "terminal projection is missing contract: ${required}"
done

[[ "$(rg -Fc '.phase == "Completed"' <<<"$projection_function")" == 2 ]] ||
  fail "history and status must use the real maker RPC terminal enum spelling"
if rg -Fq '.phase == "completed"' <<<"$projection_function"; then
  fail "runner must not compare maker RPC phases to actor-status spelling"
fi

history_fixture='[{"id":"m5-contract-swap","phase":"Completed"}]'
status_fixture='{"id":"m5-contract-swap","phase":"Completed"}'
jq -e --arg swap m5-contract-swap '
  length == 1 and .[0].id == $swap and .[0].phase == "Completed"
' <<<"$history_fixture" >/dev/null || fail "valid history fixture was rejected"
jq -e --arg swap m5-contract-swap '
  .id == $swap and .phase == "Completed"
' <<<"$status_fixture" >/dev/null || fail "valid status fixture was rejected"

maker_terminal_line="$(rg -n -F '${evidence_dir}/maker-status-final.json' "$runner" | tail -1 | cut -d: -f1)"
taker_terminal_line="$(rg -n -F '${evidence_dir}/taker-status-final.json' "$runner" | tail -1 | cut -d: -f1)"
projection_call_line="$(rg -n '^  prove_m5_terminal_operator_projection$' "$runner" |
  cut -d: -f1)"
[[ "$maker_terminal_line" =~ ^[0-9]+$ && "$taker_terminal_line" =~ ^[0-9]+$ &&
   "$projection_call_line" =~ ^[0-9]+$ ]] || fail "terminal ordering anchors are missing"
(( maker_terminal_line < projection_call_line && taker_terminal_line < projection_call_line )) ||
  fail "operator projection must run only after both role actors are terminal"

echo "M5 application terminal-projection contract passed"
