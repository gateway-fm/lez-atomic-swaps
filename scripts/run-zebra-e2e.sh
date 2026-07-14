#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

run_id="${RUN_ID:-local-$$}"
if [[ ! "$run_id" =~ ^[a-z0-9][a-z0-9_-]*$ ]]; then
  echo "RUN_ID must contain only lowercase letters, numbers, underscores, or hyphens" >&2
  exit 1
fi
primary_only="${ZEBRA_E2E_PRIMARY_ONLY:-0}"
skip_tests="${ZEBRA_E2E_SKIP_TESTS:-0}"
keep_running="${ZEBRA_E2E_KEEP_RUNNING:-0}"
for toggle in "$primary_only" "$skip_tests" "$keep_running"; do
  if [[ "$toggle" != "0" && "$toggle" != "1" ]]; then
    echo "Zebra PoC mode toggles must be 0 or 1" >&2
    exit 1
  fi
done
if (( primary_only == 1 && skip_tests == 0 )); then
  echo "primary-only Zebra mode requires ZEBRA_E2E_SKIP_TESTS=1" >&2
  exit 1
fi

project="lez-atomic-swaps-${run_id}"
compose_file="tests/e2e/zebra/compose.yml"
dockerfile="tests/e2e/zebra/Dockerfile"
run_dir="$(pwd)/.e2e/${run_id}"
manifest="${run_dir}/run.env"
maker_database="${run_dir}/maker-zebra.sqlite3"

owns_image=0
if [[ -z "${ZEBRA_IMAGE:-}" ]]; then
  ZEBRA_IMAGE="lez-atomic-swaps-zebra:${run_id}"
  owns_image=1
fi
export ZEBRA_IMAGE

mkdir -p "$run_dir"
chmod 700 "$run_dir"
if [[ -e "$manifest" ]]; then
  echo "refusing to overwrite Zebra E2E run manifest: ${manifest}" >&2
  exit 1
fi
for database_file in "$maker_database" "${maker_database}-wal" "${maker_database}-shm"; do
  if [[ -e "$database_file" ]]; then
    echo "refusing to reuse maker E2E database state: ${database_file}" >&2
    exit 1
  fi
done
export ZEBRA_E2E_DB="$maker_database"
printf 'RUN_ID=%s\nCOMPOSE_PROJECT_NAME=%s\nZEBRA_IMAGE=%s\nZEBRA_E2E_DB=%s\n' \
  "$run_id" "$project" "$ZEBRA_IMAGE" "$ZEBRA_E2E_DB" >"$manifest"

compose=(docker compose --project-name "$project" --file "$compose_file")

if [[ -n "$("${compose[@]}" ps --quiet)" ]]; then
  echo "refusing to reuse active Docker project: ${project}" >&2
  exit 1
fi

cleanup() {
  status=$?
  if (( keep_running == 1 && status == 0 )); then
    echo "Zebra Regtest remains running for RUN_ID=${run_id}; evidence: ${manifest}"
    echo "Cleanup stack: docker compose --project-name ${project} --file ${compose_file} down --volumes --remove-orphans"
    if (( owns_image == 1 )); then
      echo "Cleanup image: docker image rm ${ZEBRA_IMAGE}"
    fi
    return 0
  fi
  if (( status != 0 )); then
    "${compose[@]}" logs --no-color || true
  fi
  "${compose[@]}" down --volumes --remove-orphans || true
  if (( owns_image == 1 )); then
    docker image rm "$ZEBRA_IMAGE" >/dev/null 2>&1 || true
  fi
  return "$status"
}
trap cleanup EXIT
trap "exit 130" INT
trap "exit 143" TERM

if (( owns_image == 1 )); then
  docker build \
    --file "$dockerfile" \
    --label "org.logos-co.atomic-swaps.run=${run_id}" \
    --tag "$ZEBRA_IMAGE" \
    tests/e2e/zebra
fi

export RUN_ID="$run_id"
services=(zebra)
if (( primary_only == 0 )); then
  services+=(zebra_fork)
fi
"${compose[@]}" up --detach --no-build "${services[@]}"

published_endpoint="$("${compose[@]}" port zebra 18232 | tail -n 1)"
rpc_port="${published_endpoint##*:}"
rpc_url="http://127.0.0.1:${rpc_port}"
export ZEBRA_RPC_URL="$rpc_url"
printf 'ZEBRA_RPC_URL=%s\n' "$ZEBRA_RPC_URL" >>"$manifest"
if (( primary_only == 0 )); then
  fork_published_endpoint="$("${compose[@]}" port zebra_fork 18232 | tail -n 1)"
  fork_rpc_port="${fork_published_endpoint##*:}"
  fork_rpc_url="http://127.0.0.1:${fork_rpc_port}"
  export ZEBRA_FORK_RPC_URL="$fork_rpc_url"
  printf 'ZEBRA_FORK_RPC_URL=%s\n' "$ZEBRA_FORK_RPC_URL" >>"$manifest"
fi
printf 'ZEBRA_E2E_PRIMARY_ONLY=%s\nZEBRA_E2E_SKIP_TESTS=%s\n' \
  "$primary_only" "$skip_tests" >>"$manifest"

ready=0
fork_ready="$primary_only"
for _ in {1..30}; do
  if (( ready == 0 )) && curl -sf --max-time 2 \
    -H 'content-type: application/json' \
    --data '{"jsonrpc":"2.0","id":1,"method":"getblockcount","params":[]}' \
    "$rpc_url" >/dev/null; then
    ready=1
  fi
  if (( primary_only == 0 && fork_ready == 0 )) && curl -sf --max-time 2 \
    -H 'content-type: application/json' \
    --data '{"jsonrpc":"2.0","id":1,"method":"getblockcount","params":[]}' \
    "$fork_rpc_url" >/dev/null; then
    fork_ready=1
  fi
  if (( ready == 1 && fork_ready == 1 )); then
    break
  fi
  sleep 2
done

if (( ready == 0 )); then
  echo "Primary Zebra RPC did not become ready within 60 seconds" >&2
fi
if (( primary_only == 0 && fork_ready == 0 )); then
  echo "Fork Zebra RPC did not become ready within 60 seconds" >&2
fi
if (( ready == 0 || fork_ready == 0 )); then
  exit 1
fi

if (( skip_tests == 0 )); then
  CARGO_BUILD_JOBS=2 cargo test --locked \
    -p lez-maker-node --test zebra_runtime_restart -- --ignored --nocapture

  CARGO_BUILD_JOBS=2 cargo test --locked \
    -p lez-zec-swap-sdk --test zebra_regtest -- --ignored --nocapture
else
  echo "Zebra isolated service-readiness passed: primary=${ZEBRA_RPC_URL}"
fi
