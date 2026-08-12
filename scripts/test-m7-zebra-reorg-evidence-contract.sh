#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly source="crates/zec-swap-sdk/tests/zebra_regtest.rs"

fail() {
  echo "M7 Zebra reorg evidence contract failed: $*" >&2
  exit 1
}

for required in \
  'M7_ZEBRA_REORG_EVIDENCE' \
  'm7_actual_zebra_competing_fork' \
  'old_branch_detached' \
  'replacement_branch_canonical' \
  'shared_refund_survived_reorg' \
  'conflicting_refund_replaced_claim' \
  'public_rpc_used' \
  'runtime_external_resources'; do
  rg -Fq -- "$required" "$source" || fail "source omits evidence field: $required"
done

cargo +1.96.0 test --locked --offline -p lez-zec-swap-sdk \
  --test zebra_regtest --no-run

echo "M7 Zebra reorg evidence contract passed"
