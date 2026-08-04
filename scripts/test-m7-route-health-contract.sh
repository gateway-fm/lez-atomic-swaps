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
readonly outage_runner="scripts/run-m7-unaffected-pair-outage-poc.sh"
readonly handoff_runner="scripts/run-m5-zec-chat-handoff.sh"
readonly corridor_runner="scripts/run-m2-taker-sells-lez-poc.sh"
readonly semantic_probe="scripts/probe-local-json-rpc-health.sh"
readonly actual_node_certificate="docs/evidence/m7-unaffected-pair-outage-2c63218-20260804.json"

for path in "$source" "$daemon" "$policy_test" "$adr" "$outage_runner" \
  "$handoff_runner" "$corridor_runner" "$semantic_probe" "$actual_node_certificate"; do
  [[ -s "$path" ]] || fail "missing ${path}"
done

for literal in 'getblockchaininfo' 'getblockhash' \
  '029f11d80ef9765602235e1bc9727e3eb6ba20839319f761fee920d63401e327' \
  '0f9188f13cb7b2c71f2a335e3a4fc328bf5beb436012afca590b1a11466e2206'; do
  rg -Fq "$literal" "$semantic_probe" ||
    fail "semantic local-node probe is missing ${literal}"
done

for literal in \
  'BITCOIN_CORE_E2E_MODE=service' \
  'BITCOIN_CORE_E2E_KEEP_RUNNING=1' \
  'mkdir -m 0700 "$proof_root/bin"' \
  'install -m 0500 "$health_program_source" "$health_program"' \
  '[[ "$health_sha256" == "$health_source_sha256" ]]' \
  'readonly health_source_sha256 health_sha256' \
  'docker container stop' \
  'm7-bitcoin-healthy-before-stop.json' \
  'm7-bitcoin-unavailable-after-stop.json' \
  'M7_ROUTE_HEALTH_CONFIG' \
  'run-m5-zec-application-poc.sh' \
  'unaffected_pair_swap_completed'; do
  rg -Fq "$literal" "$outage_runner" \
    || fail "missing actual-node outage token: ${literal}"
done

for literal in \
  '--route-health-config' \
  '--route-health-poll-milliseconds' \
  'length == 2 and any(.[];' \
  '.value.route.pair == "Bitcoin" and .value.enabled == false' \
  '.value.route.pair == "Zcash" and .value.enabled == true' \
  'm7-route-health-before-swap.json' \
  'm7-route-health-after-restart.json'; do
  rg -Fq -- "$literal" "$handoff_runner" \
    || fail "missing handoff route-health token: ${literal}"
done

for literal in 'M7_ROUTE_HEALTH_CONFIG' '--route-health-config'; do
  rg -Fq -- "$literal" "$corridor_runner" \
    || fail "missing corridor route-health token: ${literal}"
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

jq -e '
  .schema_version == 1
  and .kind == "m7_unaffected_pair_actual_node_outage_poc"
  and .result == "passed"
  and .repository_commit == "2c63218542c0ce9d53df521b5fd88bf46693fcb8"
  and .absent_route == {pair:"Bitcoin",direction:"TakerSellsForeign",actual_local_node:true}
  and .surviving_route == {pair:"Zcash",direction:"TakerSellsLez",actual_local_node:true}
  and .absent_route_failed_closed == true
  and .route_isolation_survived_maker_restart == true
  and .unaffected_pair_swap_completed == true
  and .atomic_claim_order_observed == true
  and ([.evidence_sha256[] | test("^[0-9a-f]{64}$")] | all)
  and (.evidence_sha256 | length) == 6
  and .runtime_external_resources == []
  and .public_rpc_used == false and .faucet_used == false and .public_funds_used == false
' "$actual_node_certificate" >/dev/null || fail "checked actual-node outage certificate is invalid"
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
