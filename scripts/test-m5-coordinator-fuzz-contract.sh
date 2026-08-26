#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly fuzz_manifest="fuzz/Cargo.toml"
readonly fuzz_target="fuzz/fuzz_targets/coordinator.rs"
readonly smoke_runner="scripts/run-m5-coordinator-fuzz-smoke.sh"
readonly workflow=".github/workflows/ci.yml"

for path in \
  "$fuzz_manifest" \
  "fuzz/Cargo.lock" \
  "$fuzz_target" \
  "$smoke_runner" \
  "fuzz/deny.toml"; do
  test -f "$path" || {
    echo "missing M5 coordinator-fuzz file: $path" >&2
    exit 1
  }
done

rg -Fqx 'cargo-fuzz = true' "$fuzz_manifest"
rg -Fqx 'libfuzzer-sys = "=0.4.13"' "$fuzz_manifest"
rg -Fqx 'lez-swap-core = { path = "../crates/swap-core", version = "=0.1.0" }' "$fuzz_manifest"
rg -Fqx 'serde_json = "=1.0.150"' "$fuzz_manifest"

rg -Fq 'fuzz_target!(|data: &[u8]|' "$fuzz_target"
for token in \
  'Pair::Bitcoin' \
  'Pair::Monero' \
  'Pair::Zcash' \
  'SwapDirection::TakerSellsForeign' \
  'SwapDirection::TakerSellsLez' \
  'observe_taker_lock' \
  'observe_maker_lock' \
  'observe_revealing_claim' \
  'observe_followup_claim' \
  'refund_maker_leg' \
  'refund_taker_leg' \
  'serde_json::to_vec' \
  'terminal_snapshot'; do
  rg -Fq "$token" "$fuzz_target"
done

test "$(find fuzz/corpus/coordinator -maxdepth 1 -type f | wc -l)" -ge 6
rg -Fq 'FUZZ_SMOKE_RUNS:-512' "$smoke_runner"
rg -Fq 'cp -a fuzz/corpus/coordinator/. "$corpus_dir/"' "$smoke_runner"
rg -Fq 'coordinator "$corpus_dir"' "$smoke_runner"
rg -Fq 'FUZZ_MAX_LEN:-512' "$smoke_runner"
rg -Fq 'FUZZ_TIMEOUT_SECONDS:-2' "$smoke_runner"
rg -Fq 'nightly-2026-07-01' "$smoke_runner"

rg -Fq 'coordinator fuzz smoke' "$workflow"
rg -Fq 'cargo install cargo-fuzz --version 0.13.2 --locked' "$workflow"
rg -Fq './scripts/run-m5-coordinator-fuzz-smoke.sh' "$workflow"
rg -Fq 'manifest-path: fuzz/Cargo.toml' "$workflow"
rg -Fq 'libfuzzer-sys@0.4.13' fuzz/deny.toml

echo "M5 coordinator fuzz contract passed"
