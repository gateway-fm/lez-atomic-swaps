#!/usr/bin/env bash
# Publish a completed, public LEZ/BTC run into the Basecamp M3 evidence view.
set -euo pipefail

DEPLOY_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$DEPLOY_ROOT"

if [[ -f runtime/runtime.env ]]; then
  set -a
  source runtime/runtime.env
  set +a
fi

mode="${1:---certified}"
runner_name="${LEZ_M3_RUNNER_CONTAINER:-lez-runner-arm}"
runner_repo="${LEZ_M3_RUNNER_REPO:-/Users/mandrigin/Desktop/las-logos/runner-work/repo}"
output="$DEPLOY_ROOT/runtime/m3-btc-ui-evidence.json"

mkdir -p runtime

case "$mode" in
  --certified)
    cp full-swap/evidence-m5arm-08180005-ui.json "$output"
    ;;
  --from-run)
    [[ $# == 2 ]] || { echo "usage: $0 --from-run <m3-evidence-directory>" >&2; exit 64; }
    bash full-swap/export-ui-evidence.sh "$2" "$output"
    ;;
  --rerun)
    [[ $# == 1 ]] || { echo "usage: $0 --rerun" >&2; exit 64; }
    docker inspect "$runner_name" >/dev/null 2>&1 || {
      echo "local ARM runner is unavailable: $runner_name" >&2
      exit 1
    }
    echo "starting a fresh real LEZ/BTC run (normally about five minutes)…"
    docker cp full-swap/run-full-swap.sh "$runner_name:/tmp/lez-run-full-btc-ui.sh"
    docker exec "$runner_name" bash /tmp/lez-run-full-btc-ui.sh
    latest="$(find "$runner_repo/.e2e" -mindepth 1 -maxdepth 1 -type d -name 'm5arm-*' -print \
      | sort | tail -1)"
    [[ -n "$latest" ]] || { echo "fresh BTC run evidence was not found" >&2; exit 1; }
    bash full-swap/export-ui-evidence.sh "$latest/m3-actor-poc/evidence" "$output"
    ;;
  *)
    echo "usage: $0 [--certified | --from-run <m3-evidence-directory> | --rerun]" >&2
    exit 64
    ;;
esac

chmod 0644 "$output"
jq -e '
  .kind == "m3_btc_ui_evidence"
  and .pair == "Bitcoin"
  and .terminal == {phase:"completed",revision:4}
  and (.effects | length) == 5
  and ([.effects[].transaction_id] | unique | length) == 5
  and .private_material_disclosed == false
' "$output" >/dev/null

docker compose --env-file runtime/runtime.env up -d --no-deps --force-recreate \
  lez-explorer basecamp-ui >/dev/null

run_id="$(jq -r '.run_id' "$output")"
cat <<BANNER

──────────────────────────────────────────────────────────────
 M3 LEZ / Bitcoin evidence is ready

 run:        ${run_id}
 effects:    2 Bitcoin + 3 LEZ
 terminal:   revision 4 · completed
 Basecamp:   vnc://127.0.0.1:5901  (password: lezswap)
 proof UI:   http://127.0.0.1:3003/#/evidence
──────────────────────────────────────────────────────────────
BANNER
