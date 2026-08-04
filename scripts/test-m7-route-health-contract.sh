#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

fail() {
  echo "M7 route-health contract failed: $*" >&2
  exit 1
}

readonly source="crates/maker-node/src/route_health.rs"
readonly daemon="crates/maker-node/src/bin/lez-maker-daemon.rs"
readonly policy_test="crates/maker-node/tests/route_health.rs"
readonly adr="docs/architecture/0150-withdraw-only-unhealthy-route-advertisements.md"

for path in "$source" "$daemon" "$policy_test" "$adr"; do
  [[ -s "$path" ]] || fail "missing ${path}"
done

for literal in \
  'pub struct ProcessRouteHealthProbe' \
  'pub trait MakerRouteHealthProbe' \
  'route_health_withdrawal_request_id' \
  'list_discoverable_maker_offers' \
  'MakerOfferStatus::Withdrawn' \
  'MakerOfferStatus::Reserved' \
  'daemon_periodically_withdraws_without_a_health_request' \
  'unhealthy_route_is_fail_closed_and_reconciliation_is_pair_scoped'; do
  rg -Fq "$literal" crates/maker-node/src crates/maker-node/tests \
    || fail "missing route-health evidence token: ${literal}"
done

rg -Fq 'route_health_tasks.spawn_blocking' "$daemon" \
  || fail "semantic health workers are not isolated from the async RPC loop"
rg -Fq 'MissedTickBehavior::Skip' "$daemon" \
  || fail "periodic reconciliation can accumulate missed work"
rg -Fq 'route_health_poll_milliseconds' "$daemon" \
  || fail "daemon does not expose bounded reconciliation cadence"
rg -Fq './scripts/test-m7-route-health-contract.sh' scripts/run-ci-quality-gates.sh \
  || fail "CI quality gates do not run this contract"
rg -Fq './scripts/test-m7-route-health-contract.sh' scripts/test-ci-hardening-policy.sh \
  || fail "CI hardening policy does not pin this contract"

echo "M7 route-health contract passed"
