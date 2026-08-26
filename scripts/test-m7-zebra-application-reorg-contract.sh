#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly source="crates/maker-node/tests/zebra_runtime_restart.rs"

fail() {
  echo "M7 Zebra application reorg continuation contract failed: $*" >&2
  exit 1
}

for required in \
  'M7_ZEBRA_APPLICATION_REORG_EVIDENCE' \
  'm7_actual_zebra_application_reorg_continuation' \
  'funding_removed' \
  'funding_remined' \
  'swap_resumed' \
  'removal_revision' \
  'restored_revision' \
  'runtime_external_resources' \
  'public_rpc_used'; do
  rg -Fq -- "$required" "$source" || fail "source omits evidence field: $required"
done

cargo +1.96.0 test --locked --offline -p lez-maker-node \
  --test zebra_runtime_restart --no-run

echo "M7 Zebra application reorg continuation contract passed"
