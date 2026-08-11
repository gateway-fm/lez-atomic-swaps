#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly runner="scripts/run-m4-actual-claim-poc.sh"
readonly taker="crates/maker-node/src/bin/lez-taker.rs"

fail() {
  echo "M7 Taker-claim process-kill contract failed: $*" >&2
  exit 1
}

for path in "$runner" "$taker"; do
  test -f "$path" || fail "missing $path"
done

for token in \
  'M7_XMR_CLAIM_PROCESS_KILL_AFTER_SUBMISSION' \
  'm7_xmr_claim_process_kill' \
  '--features test-crash-hooks' \
  'LEZ_TAKER_TEST_PAUSE_AFTER_INVOKED_STEP' \
  'LEZ_TAKER_TEST_PAUSE_MARKER' \
  'authorize_lez_tag14' \
  'paused_after_invoked_before_stdout' \
  'kill -KILL -- "-${crashed_taker_group}"' \
  'post_restart_route:"observe_only"' \
  'automatic_submission_retry:false' \
  'release_journal_unchanged_after_restart:true'; do
  rg -Fq -- "$token" "$runner" "$taker" || fail "missing claim crash invariant: $token"
done

rg -Fq 'M7_XMR_CLAIM_PROCESS_KILL_AFTER_SUBMISSION=1 requires M7_XMR_SEMANTIC_CLAIM=1' "$runner" ||
  fail "claim crash mode is not bound to semantic-claim mode"
rg -Fq 'feature = "test-crash-hooks"' "$taker" ||
  fail "Taker pause hook is not compile-time gated"

bash -n "$runner"
echo "M7 Taker-claim process-kill contract passed"
