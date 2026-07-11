#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

run_id="${RUN_ID:-local-$$}"
if [[ ! "$run_id" =~ ^[a-z0-9][a-z0-9_-]*$ ]]; then
  echo "RUN_ID must contain only lowercase letters, numbers, underscores, or hyphens" >&2
  exit 1
fi

project="lez-atomic-swaps-${run_id}"
compose_file="tests/e2e/zebra/compose.yml"
dockerfile="tests/e2e/zebra/Dockerfile"
run_dir=".e2e/${run_id}"
manifest="${run_dir}/run.env"

owns_image=0
if [[ -z "${ZEBRA_IMAGE:-}" ]]; then
  ZEBRA_IMAGE="lez-atomic-swaps-zebra:${run_id}"
  owns_image=1
fi
export ZEBRA_IMAGE

mkdir -p "$run_dir"
printf 'RUN_ID=%s\nCOMPOSE_PROJECT_NAME=%s\nZEBRA_IMAGE=%s\n' \
  "$run_id" "$project" "$ZEBRA_IMAGE" >"$manifest"

compose=(docker compose --project-name "$project" --file "$compose_file")

if [[ -n "$("${compose[@]}" ps --quiet)" ]]; then
  echo "refusing to reuse active Docker project: ${project}" >&2
  exit 1
fi

cleanup() {
  status=$?
  if (( status != 0 )); then
    "${compose[@]}" logs --no-color zebra || true
  fi
  "${compose[@]}" down --volumes --remove-orphans || true
  if (( owns_image == 1 )); then
    docker image rm "$ZEBRA_IMAGE" >/dev/null 2>&1 || true
  fi
  return "$status"
}
trap cleanup EXIT INT TERM

if (( owns_image == 1 )); then
  docker build \
    --file "$dockerfile" \
    --label "org.logos-co.atomic-swaps.run=${run_id}" \
    --tag "$ZEBRA_IMAGE" \
    tests/e2e/zebra
fi

export RUN_ID="$run_id"
"${compose[@]}" up --detach --no-build zebra

published_endpoint="$("${compose[@]}" port zebra 18232 | tail -n 1)"
rpc_port="${published_endpoint##*:}"
rpc_url="http://127.0.0.1:${rpc_port}"
printf 'ZEBRA_RPC_URL=%s\n' "$rpc_url" >>"$manifest"

ready=0
for _ in {1..30}; do
  if curl -sf --max-time 2 \
    -H 'content-type: application/json' \
    --data '{"jsonrpc":"2.0","id":1,"method":"getblockcount","params":[]}' \
    "$rpc_url" >/dev/null; then
    ready=1
    break
  fi
  sleep 2
done

if (( ready == 0 )); then
  echo "Zebra RPC did not become ready within 60 seconds" >&2
  exit 1
fi

ZEBRA_RPC_URL="$rpc_url" CARGO_BUILD_JOBS=2 cargo test --locked \
  -p lez-zec-swap-sdk --test zebra_regtest -- --ignored --nocapture
