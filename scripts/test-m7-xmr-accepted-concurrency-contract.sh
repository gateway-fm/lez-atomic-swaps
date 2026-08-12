#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly process_test="crates/maker-node/tests/daemon_actor_supervisor_process.rs"

fail() {
  echo "M7 XMR accepted-concurrency contract failed: $*" >&2
  exit 1
}

for required in \
  'fn daemon_leases_two_accepted_xmr_applications_concurrently_across_restart()' \
  'two accepted XMR applications must hold concurrent leases' \
  'distinct XMR actor configurations' \
  'distinct XMR actor state databases' \
  'accepted XMR restart must preserve terminal isolation'; do
  rg -Fq -- "$required" "$process_test" ||
    fail "process test is missing invariant: $required"
done

cargo +1.96.0 test --locked --offline -p lez-maker-node \
  --test daemon_actor_supervisor_process \
  daemon_leases_two_accepted_xmr_applications_concurrently_across_restart \
  -- --exact

rg -Fq './scripts/test-m7-xmr-accepted-concurrency-contract.sh' scripts/run-ci-quality-gates.sh ||
  fail "contract is absent from the quality runner"
rg -Fq './scripts/test-m7-xmr-accepted-concurrency-contract.sh' scripts/test-ci-hardening-policy.sh ||
  fail "CI policy does not pin the functional concurrency contract"

echo "M7 XMR accepted-concurrency contract passed"
