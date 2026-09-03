#!/usr/bin/env bash
# swap-through-ui.sh — one complete BTC → LEZ swap driven through the two
# Basecamp apps, one role-owned action per app run, against the running stack:
#   Taker takes an offer → Taker locks BTC → Maker funds LEZ →
#   Taker claims LEZ → Maker claims BTC → proof published.
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1
set -a; source runtime/runtime.env; set +a
export BTC_RPC_PASSWORD

log() { printf '\n[%s] %s\n' "$(date -u +%H:%M:%S)" "$*"; }
ui() { # ui <role> [ENV=VALUE...]
  local role="$1"; shift; local envs=()
  for kv in "$@"; do envs+=(-e "$kv"); done
  docker compose --env-file runtime/runtime.env run --rm --no-deps "${envs[@]}" \
    --entrypoint node basecamp-ui /ui-tests/verify.mjs "$role" 2>&1 |
    grep -E '✓|✗|interactive|Expected|passed|failed|has not|Error' | grep -viE 'locale'
}
swap_state() {
  docker exec -i lez-btc-demo-controller python3 - <<'PY'
import http.client, socket, json
class C(http.client.HTTPConnection):
    def connect(self):
        self.sock = socket.socket(socket.AF_UNIX); self.sock.connect('/run/lez-btc-demo/controller.sock')
c = C('localhost')
c.request("POST", "/", body=json.dumps({"jsonrpc": "2.0", "id": 1, "method": "btc_market_snapshot_v1",
          "params": [{"schema_version": 2, "role": "taker", "wallet_id": "taker-zurich-01"}]}),
          headers={"content-type": "application/json"})
for s in json.loads(c.getresponse().read())["result"].get("swaps", []):
    print("  ", {k: s.get(k) for k in ("ui_swap_id", "state", "action_role", "action_required", "progress_percent", "error", "run_id") if s.get(k) is not None})
PY
}
step() { log "$1"; shift; ui "$@"; swap_state; }
bash scripts/repair-indexer.sh || exit 1
step "1/5 Taker takes an offer (the runner prepares the swap)" taker PREPARE_INTERACTIVE_BTC=1
step "2/5 Taker: Lock 0.01 BTC" taker INTERACTIVE_ACTION=lock_btc
step "3/5 Maker: Fund 1,000 LEZ (waits for the BTC lock to confirm)" maker INTERACTIVE_ACTION=fund_lez
step "4/5 Taker: Claim 1,000 LEZ (waits for LEZ funding to finalize)" taker INTERACTIVE_ACTION=claim_lez
step "5/5 Maker: Claim Bitcoin (waits for the revealing claim)" maker INTERACTIVE_ACTION=claim_btc
log "waiting for the swap to complete and the proof to publish"
for _ in $(seq 1 60); do
  state="$(swap_state)"; echo "$state"
  [[ "$state" == *"'completed'"* ]] && break
  [[ "$state" == *"'failed'"* ]] && { echo "swap failed" >&2; exit 1; }
  sleep 20
done
[[ "$state" == *"'completed'"* ]] || { echo "swap did not complete in time" >&2; exit 1; }
log "published evidence run: $(jq -r '.run_id' runtime/m3-btc-ui-evidence.json)"
