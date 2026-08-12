#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly test_file="crates/maker-node/tests/maker_actor_supervisor.rs"
readonly decision="docs/architecture/0197-compose-maker-all-pair-actions.md"
readonly manual="docs/manual-user-flows.md"

fail() {
  echo "M7 Maker all-pair action matrix failed: $*" >&2
  exit 1
}

for path in "$test_file" "$decision" "$manual"; do
  [[ -f "$path" ]] || fail "missing ${path}"
done

for token in \
  'all_pair_manual_actions_execute_semantic_commands_and_replay_after_restart' \
  'label: "btc-claim"' \
  'label: "btc-refund"' \
  'label: "xmr-claim"' \
  'label: "xmr-refund"' \
  'label: "zec-claim"' \
  'label: "zec-refund"' \
  'spawn_matrix_daemon(&database, &socket, &ready, false)' \
  'spawn_matrix_daemon(&database, &socket, &ready, true)' \
  'assert_eq!(replay["was_replay"], true)' \
  'MakerActorManualActionState::Completed' \
  'MakerActorScheduleState::Terminal'; do
  rg -Fq -- "$token" "$test_file" || fail "matrix is missing ${token}"
done

rg -Fq 'Flow 1ZN: Repeat every Maker claim and refund action' "$manual" ||
  fail "manual all-pair Maker flow is missing"
rg -Fq 'sequenceDiagram' "$decision" || fail "architecture sequence is missing"

cargo test -p lez-maker-node --test maker_actor_supervisor \
  all_pair_manual_actions_execute_semantic_commands_and_replay_after_restart \
  -- --exact

echo "M7 Maker all-pair action matrix passed"
