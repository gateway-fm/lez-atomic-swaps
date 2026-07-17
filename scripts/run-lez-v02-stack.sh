#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly expected_source_commit="a58fbce2ff48c58b7bb5001b1a27e64b9596ee3a"
readonly expected_sequencer_sha256="3727e9aa10600d04d0cdfda6eb39df146ef4cc14f5b09ad33bcf076a8f2c412f"
readonly expected_indexer_sha256="6ed54f04ae018f3554898a9f0aef6decd6930c4e8609326d146ca164e48d7442"
readonly expected_r0vm_sha256="36c016a5bb2ded5bd1f8f92cc487e6ffaeb1e95ec05850c983081a0f716b515b"
readonly channel_id="b6adb2d238911395adde0b2f40b880ec03ffd1a3a8d97e7df8cacadf08873748"
readonly genesis_channel_id="0000000000000000000000000000000000000000000000000000000000000000"
readonly bedrock_signing_key_hex="0ab865b8054be13810889714c1f1d82c3d8bb2e4510c26d0edc35cc653f306c2"
readonly expected_bedrock_signing_key_sha256="8fd0d8a6423536c14b5d3979e5135bf37253f5dfbc8485b52202bbf963b8f02e"
readonly upstream_lez_channel_id="0101010101010101010101010101010101010101010101010101010101010101"
readonly upstream_genesis_time_hex="2c04626900000000"
readonly upstream_slot_duration_seconds="1.0"
slot_duration_seconds="${LEZ_V02_SLOT_DURATION_SECONDS:-$upstream_slot_duration_seconds}"
case "$slot_duration_seconds" in
  1.0 | 3.0 | 10.0) ;;
  *)
    echo "LEZ_V02_SLOT_DURATION_SECONDS must be exactly 1.0, 3.0, or 10.0" >&2
    exit 1
    ;;
esac
readonly slot_duration_seconds
readonly default_maker_account_id="B1UN3hPgxacgHKBRoThcAmsPajGcUf6YXUhgB36x4DAd"
readonly default_maker_vault_account_id="7Mzr43PK9VxpcvwdjgL8PeE4nb2aG9FqBKLfkoH8RBmQ"
readonly maker_genesis_allocation="100000"
readonly default_taker_account_id="34Kqgek6R7N1zU5FSJz8ziXwSPEPCuWGcn1T7GCVrfib"
readonly default_taker_vault_account_id="AXLjVw4tKTgieQoGRgXMVLVVaB4c5YnL1YTogZdX1cpH"
readonly taker_genesis_allocation="200000"

if [[ "${LEZ_V02_MAKER_ACCOUNT_ID+x}" != \
      "${LEZ_V02_MAKER_VAULT_ACCOUNT_ID+x}" ]]; then
  echo "maker owner and Vault overrides must be supplied together" >&2
  exit 1
fi
if [[ "${LEZ_V02_TAKER_ACCOUNT_ID+x}" != \
      "${LEZ_V02_TAKER_VAULT_ACCOUNT_ID+x}" ]]; then
  echo "taker owner and Vault overrides must be supplied together" >&2
  exit 1
fi
maker_account_id="${LEZ_V02_MAKER_ACCOUNT_ID:-$default_maker_account_id}"
maker_vault_account_id="${LEZ_V02_MAKER_VAULT_ACCOUNT_ID:-$default_maker_vault_account_id}"
taker_account_id="${LEZ_V02_TAKER_ACCOUNT_ID:-$default_taker_account_id}"
taker_vault_account_id="${LEZ_V02_TAKER_VAULT_ACCOUNT_ID:-$default_taker_vault_account_id}"
readonly maker_account_id maker_vault_account_id taker_account_id taker_vault_account_id

validate_actor_account_id() {
  local account_id="$1"
  local role="$2"
  if [[ ! "$account_id" =~ ^[1-9A-HJ-NP-Za-km-z]{43,44}$ ]]; then
    echo "${role} account ID must be a canonical base58 public AccountId" >&2
    exit 1
  fi
}

validate_actor_account_id "$maker_account_id" "maker"
validate_actor_account_id "$maker_vault_account_id" "maker Vault"
validate_actor_account_id "$taker_account_id" "taker"
validate_actor_account_id "$taker_vault_account_id" "taker Vault"

run_id="${RUN_ID:-local-$$}"
if [[ ! "$run_id" =~ ^[a-z0-9][a-z0-9_-]{0,63}$ ]]; then
  echo "RUN_ID must match ^[a-z0-9][a-z0-9_-]{0,63}$" >&2
  exit 1
fi
if [[ "$maker_account_id" == "$taker_account_id" ||
      "$maker_vault_account_id" == "$taker_vault_account_id" ||
      "$maker_account_id" == "$maker_vault_account_id" ||
      "$maker_account_id" == "$taker_vault_account_id" ||
      "$taker_account_id" == "$maker_vault_account_id" ||
      "$taker_account_id" == "$taker_vault_account_id" ||
      "$maker_genesis_allocation" == "$taker_genesis_allocation" ]]; then
  echo "maker and taker genesis identities, Vaults, and allocations must remain distinct" >&2
  exit 1
fi

project="lez-atomic-swaps-lez-v02-${run_id}"
network="${project}-private"
run_dir="$(pwd)/.e2e/${run_id}/lez-v02"
source_dir="${LEZ_V02_SOURCE_DIR:-/tmp/lez-v020-native-investigation}"
services_dir="${LEZ_V02_SERVICES_DIR:-/tmp/lez-v02-services-a58fbce2-20260713/release}"
r0vm="${LEZ_V02_R0VM:-/tmp/lez-atomic-swaps-tools/risc0-3.0.5/home/extensions/v3.0.5-cargo-risczero-x86_64-unknown-linux-gnu/r0vm}"
compose_file="tests/e2e/lez-v02/compose.yml"
dockerfile="tests/e2e/lez-v02/Dockerfile"
cryptarchia_policy="tests/e2e/lez-v02/cryptarchia-advanced.jq"
missing_channel_policy="tests/e2e/lez-v02/missing-channel-response.sh"
# shellcheck source=tests/e2e/lez-v02/missing-channel-response.sh
source "$missing_channel_policy"
image_context="${run_dir}/image-context"
manifest="${run_dir}/run.env"
logs_dir="${run_dir}/logs"
evidence_dir="${run_dir}/evidence"
LEZ_V02_IMAGE="lez-atomic-swaps-lez-v02:${run_id}"
LEZ_V02_UID="$(id -u)"
LEZ_V02_GID="$(id -g)"
LEZ_V02_RUN_DIR="$run_dir"
export RUN_ID="$run_id" LEZ_V02_IMAGE LEZ_V02_UID LEZ_V02_GID LEZ_V02_RUN_DIR
export LEZ_V02_SOURCE_DIR="$source_dir"

if [[ "$LEZ_V02_UID" == "0" || "$LEZ_V02_GID" == "0" ]]; then
  echo "LEZ v0.2 Docker services require a numeric non-root host UID and GID" >&2
  exit 1
fi
if [[ -e "$run_dir" ]]; then
  echo "refusing to reuse LEZ v0.2 run state: ${run_dir}" >&2
  exit 1
fi
if ! [[ -d "$source_dir/.git" ]]; then
  echo "LEZ_V02_SOURCE_DIR is not an exact source checkout: ${source_dir}" >&2
  exit 1
fi
if [[ -n "$(git -C "$source_dir" status --porcelain --untracked-files=all)" ]]; then
  echo "LEZ v0.2 source checkout must be clean: ${source_dir}" >&2
  exit 1
fi
if [[ "$(git -C "$source_dir" rev-parse HEAD)" != "$expected_source_commit" ]] ||
   [[ "$(git -C "$source_dir" rev-parse 'refs/tags/v0.2.0^{}')" != "$expected_source_commit" ]]; then
  echo "LEZ v0.2 source/tag identity mismatch" >&2
  exit 1
fi

verify_sha256() {
  local expected="$1"
  local path="$2"
  if [[ ! -f "$path" ]]; then
    echo "missing exact LEZ v0.2 runtime artifact: ${path}" >&2
    exit 1
  fi
  printf '%s  %s\n' "$expected" "$path" | sha256sum --check --strict >/dev/null
}

verify_sha256 "$expected_sequencer_sha256" "${services_dir}/sequencer_service"
verify_sha256 "$expected_indexer_sha256" "${services_dir}/indexer_service"
verify_sha256 "$expected_r0vm_sha256" "$r0vm"

mkdir -p "$image_context" "$logs_dir" "$evidence_dir" \
  "${run_dir}/config" "${run_dir}/bedrock" "${run_dir}/indexer" "${run_dir}/sequencer"
chmod 700 "$run_dir" "${run_dir}/config" "${run_dir}/bedrock" \
  "${run_dir}/indexer" "${run_dir}/sequencer" "$logs_dir" "$evidence_dir"
printf "%s" "$bedrock_signing_key_hex" | xxd -r -p \
  >"${run_dir}/sequencer/bedrock_signing_key"
if [[ "$(wc -c <"${run_dir}/sequencer/bedrock_signing_key")" != "32" ]]; then
  echo "deterministic local Bedrock signing key must contain exactly 32 raw bytes" >&2
  exit 1
fi
verify_sha256 "$expected_bedrock_signing_key_sha256" \
  "${run_dir}/sequencer/bedrock_signing_key"
chmod 0400 "${run_dir}/sequencer/bedrock_signing_key"
cp "${services_dir}/sequencer_service" "${image_context}/sequencer_service"
cp "${services_dir}/indexer_service" "${image_context}/indexer_service"
cp "$r0vm" "${image_context}/r0vm"
chmod 0555 "${image_context}/sequencer_service" "${image_context}/indexer_service" \
  "${image_context}/r0vm"

chain_start_epoch="$(date -u "+%s")"
genesis_time_big_endian="$(printf "%016x" "$chain_start_epoch")"
genesis_time_hex="$(sed -E \
  "s/^(..)(..)(..)(..)(..)(..)(..)(..)$/\8\7\6\5\4\3\2\1/" \
  <<<"$genesis_time_big_endian")"
count_fixed_occurrences() {
  local needle="$1"
  local file="$2"
  local matches
  matches="$(rg -Fo -- "$needle" "$file" || true)"
  if [[ -z "$matches" ]]; then
    printf "0\n"
  else
    printf "%s\n" "$matches" | wc -l | tr -d "[:space:]"
  fi
}

render_bedrock_deployment_settings() {
  local source="$1" output="$2" genesis_hex="$3" slot_duration="$4"
  local source_genesis_count source_slot_count generated_genesis_count
  local generated_stale_count generated_slot_count generated_stale_slot_count temporary

  [[ "$source" == /* && -f "$source" && ! -L "$source" ]] || {
    echo "Bedrock deployment-settings source must be one absolute regular non-symlink" >&2
    return 1
  }
  [[ "$output" == /* && ! -e "$output" && ! -L "$output" ]] || {
    echo "Bedrock deployment-settings output must be one absent absolute path" >&2
    return 1
  }
  [[ "$genesis_hex" =~ ^[0-9a-f]{16}$ &&
     "$genesis_hex" != "$upstream_genesis_time_hex" ]] || {
    echo "Bedrock genesis time must be one fresh lowercase 64-bit hex value" >&2
    return 1
  }
  case "$slot_duration" in
    1.0 | 3.0 | 10.0) ;;
    *)
      echo "Bedrock slot duration must be exactly 1.0, 3.0, or 10.0 seconds" >&2
      return 1
      ;;
  esac

  source_genesis_count="$(count_fixed_occurrences "$upstream_genesis_time_hex" "$source")"
  source_slot_count="$(count_fixed_occurrences "  slot_duration: '1.0'" "$source")"
  [[ "$source_genesis_count" == 1 && "$source_slot_count" == 1 ]] || {
    echo "locked Bedrock fixture must contain one audited genesis and slot duration" >&2
    return 1
  }

  temporary="$(mktemp "${output}.partial.XXXXXX")" || return 1
  if ! sed \
      -e "s/${upstream_genesis_time_hex}/${genesis_hex}/" \
      -e "s/  slot_duration: '1.0'/  slot_duration: '${slot_duration}'/" \
      "$source" >"$temporary"; then
    rm -f -- "$temporary"
    return 1
  fi
  generated_genesis_count="$(count_fixed_occurrences "$genesis_hex" "$temporary")"
  generated_stale_count="$(count_fixed_occurrences "$upstream_genesis_time_hex" "$temporary")"
  generated_slot_count="$(count_fixed_occurrences "  slot_duration: '${slot_duration}'" "$temporary")"
  generated_stale_slot_count="$(count_fixed_occurrences "  slot_duration: '1.0'" "$temporary")"
  if [[ "$generated_genesis_count" != 1 || "$generated_stale_count" != 0 ||
        "$generated_slot_count" != 1 ||
        ( "$slot_duration" != 1.0 && "$generated_stale_slot_count" != 0 ) ]]; then
    rm -f -- "$temporary"
    echo "generated Bedrock fixture failed exact audited replacement" >&2
    return 1
  fi
  if ! ln "$temporary" "$output"; then
    rm -f -- "$temporary"
    echo "refusing to replace an existing Bedrock deployment-settings output" >&2
    return 1
  fi
  rm -f -- "$temporary"
}

render_bedrock_deployment_settings \
  "${source_dir}/bedrock/deployment-settings.yaml" \
  "${run_dir}/config/deployment-settings.yaml" \
  "$genesis_time_hex" "$slot_duration_seconds"
jq --arg channel "$channel_id" \
  '.bedrock_config.addr = "http://bedrock:18080" | .channel_id = $channel' \
  "${source_dir}/lez/indexer/service/configs/docker/indexer_config.json" \
  >"${run_dir}/config/indexer_config.json"
jq --arg channel "$channel_id" \
  --arg maker "$maker_account_id" \
  --arg maker_amount "$maker_genesis_allocation" \
  --arg taker "$taker_account_id" \
  --arg taker_amount "$taker_genesis_allocation" \
  '.home = "/var/lib/sequencer_service"
    | .bedrock_config.node_url = "http://bedrock:18080"
    | .bedrock_config.channel_id = $channel
    | del(.bedrock_config.backoff)
    | .genesis = [
        {"supply_account": {"account_id": $maker, "balance": ($maker_amount | tonumber)}},
        {"supply_account": {"account_id": $taker, "balance": ($taker_amount | tonumber)}}
      ]' \
  "${source_dir}/lez/sequencer/service/configs/docker/sequencer_config.json" \
  >"${run_dir}/config/sequencer_config.json"
jq -e --arg channel "$channel_id" '.channel_id == $channel' \
  "${run_dir}/config/indexer_config.json" >/dev/null
jq -e --arg channel "$channel_id" '.bedrock_config.channel_id == $channel' \
  "${run_dir}/config/sequencer_config.json" >/dev/null
generated_actor_genesis_entries="$(jq -r '.genesis | length' \
  "${run_dir}/config/sequencer_config.json")"
if [[ "$generated_actor_genesis_entries" != "2" ]] ||
   ! jq -e \
     --arg maker "$maker_account_id" \
     --argjson maker_amount "$maker_genesis_allocation" \
     --arg taker "$taker_account_id" \
     --argjson taker_amount "$taker_genesis_allocation" \
     '(.genesis | length) == 2
      and ([.genesis[] | select(
        .supply_account.account_id == $maker
        and .supply_account.balance == $maker_amount
      )] | length) == 1
      and ([.genesis[] | select(
        .supply_account.account_id == $taker
        and .supply_account.balance == $taker_amount
      )] | length) == 1
      and $maker != $taker
      and $maker_amount != $taker_amount' \
     "${run_dir}/config/sequencer_config.json" >/dev/null; then
  echo "generated sequencer genesis must contain exactly the two distinct actor allocations" >&2
  exit 1
fi
chmod 0400 "${run_dir}/config/"*

printf '%s\n' \
  "RUN_ID=${run_id}" \
  "COMPOSE_PROJECT_NAME=${project}" \
  "LEZ_V02_IMAGE=${LEZ_V02_IMAGE}" \
  "LEZ_V02_SOURCE_DIR=${source_dir}" \
  "LEZ_V02_RUN_DIR=${run_dir}" \
  "LEZ_V02_SOURCE_COMMIT=${expected_source_commit}" \
  "LEZ_V02_SEQUENCER_SHA256=${expected_sequencer_sha256}" \
  "LEZ_V02_INDEXER_SHA256=${expected_indexer_sha256}" \
  "LEZ_V02_R0VM_SHA256=${expected_r0vm_sha256}" >"$manifest"
printf "LEZ_V02_UPSTREAM_CHANNEL_MISMATCH=%s\n" "$upstream_lez_channel_id" >>"$manifest"
printf "LEZ_V02_CHANNEL_PUBLIC_KEY=%s\n" "$channel_id" >>"$manifest"
printf "LEZ_V02_BEDROCK_SIGNING_KEY_SHA256=%s\n" "$expected_bedrock_signing_key_sha256" >>"$manifest"
printf "LEZ_V02_GENESIS_CHANNEL=%s\n" "$genesis_channel_id" >>"$manifest"
printf "LEZ_V02_BEDROCK_GENESIS_TIME_EPOCH=%s\n" "$chain_start_epoch" >>"$manifest"
printf "LEZ_V02_SLOT_DURATION_SECONDS=%s\n" "$slot_duration_seconds" >>"$manifest"
printf "LEZ_V02_MAKER_ACCOUNT_ID=%s\n" "$maker_account_id" >>"$manifest"
printf "LEZ_V02_MAKER_VAULT_ACCOUNT_ID=%s\n" "$maker_vault_account_id" >>"$manifest"
printf "LEZ_V02_MAKER_GENESIS_ALLOCATION=%s\n" "$maker_genesis_allocation" >>"$manifest"
printf "LEZ_V02_TAKER_ACCOUNT_ID=%s\n" "$taker_account_id" >>"$manifest"
printf "LEZ_V02_TAKER_VAULT_ACCOUNT_ID=%s\n" "$taker_vault_account_id" >>"$manifest"
printf "LEZ_V02_TAKER_GENESIS_ALLOCATION=%s\n" "$taker_genesis_allocation" >>"$manifest"
chmod 0600 "$manifest"

docker compose --project-name "$project" --file "$compose_file" config --quiet
if docker network inspect "$network" >/dev/null 2>&1; then
  echo "refusing to reuse LEZ v0.2 Docker network: ${network}" >&2
  exit 1
fi
if [[ -n "$(docker container ls --all --quiet \
    --filter "label=org.logos-co.atomic-swaps.run=${run_id}")" ]]; then
  echo "refusing to reuse active LEZ v0.2 Docker run: ${run_id}" >&2
  exit 1
fi
if docker image inspect "$LEZ_V02_IMAGE" >/dev/null 2>&1; then
  echo "refusing to reuse LEZ v0.2 runtime image: ${LEZ_V02_IMAGE}" >&2
  exit 1
fi

declare -A containers=()

assert_run_resources_absent() {
  local cleanup_failed=0
  local remaining_containers=""

  if ! docker info >/dev/null 2>&1; then
    echo "cannot prove LEZ v0.2 cleanup because the Docker daemon is unavailable" >&2
    return 1
  fi
  if ! remaining_containers="$(docker container ls --all --quiet \
      --filter "label=org.logos-co.atomic-swaps.run=${run_id}")"; then
    echo "failed to query run-scoped LEZ v0.2 containers during cleanup" >&2
    cleanup_failed=1
  elif [[ -n "$remaining_containers" ]]; then
    echo "run-scoped LEZ v0.2 containers remain after cleanup: ${remaining_containers}" >&2
    cleanup_failed=1
  fi
  if docker network inspect "$network" >/dev/null 2>&1; then
    echo "run-scoped LEZ v0.2 network remains after cleanup: ${network}" >&2
    cleanup_failed=1
  fi
  if docker image inspect "$LEZ_V02_IMAGE" >/dev/null 2>&1; then
    echo "run-scoped LEZ v0.2 image remains after cleanup: ${LEZ_V02_IMAGE}" >&2
    cleanup_failed=1
  fi

  return "$cleanup_failed"
}

cleanup() {
  local run_status=$?
  local cleanup_failed=0
  local final_status
  local service
  local container_id

  trap - EXIT
  set +e
  for service in bedrock indexer sequencer; do
    container_id="${containers[$service]:-}"
    if [[ -n "$container_id" ]]; then
      docker logs "$container_id" >"${logs_dir}/${service}.log" 2>&1
    fi
  done

  if [[ "${LEZ_V02_KEEP_RUNNING:-0}" == "1" && "$run_status" == "0" ]]; then
    echo "LEZ v0.2 stack remains running for RUN_ID=${run_id}; evidence: ${manifest}"
    printf "Cleanup containers: docker container rm --force"
    for service in sequencer indexer bedrock; do
      container_id="${containers[$service]:-}"
      if [[ -n "$container_id" ]]; then
        printf " %q" "$container_id"
      fi
    done
    printf "\nCleanup network: docker network rm %q\n" "$network"
    printf "Cleanup image: docker image rm %q\n" "$LEZ_V02_IMAGE"
  else
    for service in sequencer indexer bedrock; do
      container_id="${containers[$service]:-}"
      if [[ -n "$container_id" ]] &&
         ! docker container rm --force "$container_id" >/dev/null 2>&1; then
        echo "failed to remove run-scoped LEZ v0.2 container: ${container_id}" >&2
        cleanup_failed=1
      fi
    done
    if docker network inspect "$network" >/dev/null 2>&1 &&
       ! docker network rm "$network" >/dev/null 2>&1; then
      echo "failed to remove run-scoped LEZ v0.2 network: ${network}" >&2
      cleanup_failed=1
    fi
    if docker image inspect "$LEZ_V02_IMAGE" >/dev/null 2>&1 &&
       ! docker image rm "$LEZ_V02_IMAGE" >/dev/null 2>&1; then
      echo "failed to remove run-scoped LEZ v0.2 image: ${LEZ_V02_IMAGE}" >&2
      cleanup_failed=1
    fi
    if ! assert_run_resources_absent; then
      cleanup_failed=1
    fi
  fi

  final_status="$run_status"
  if [[ "$run_status" == "0" && "$cleanup_failed" != "0" ]]; then
    final_status=1
  fi
  exit "$final_status"
}
trap cleanup EXIT
trap "exit 130" INT
trap "exit 143" TERM

docker build \
  --file "$dockerfile" \
  --label "org.logos-co.atomic-swaps.run=${run_id}" \
  --tag "$LEZ_V02_IMAGE" \
  "$image_context"

docker network create \
  --driver bridge \
  --opt com.docker.network.bridge.enable_ip_masquerade=false \
  --label "org.logos-co.atomic-swaps.run=${run_id}" \
  --label "org.logos-co.atomic-swaps.scope=lez-v0.2-local-devnet" \
  "$network" >/dev/null

containers[bedrock]="$(docker create \
  --name "${project}-bedrock" \
  --label "org.logos-co.atomic-swaps.run=${run_id}" \
  --label "org.logos-co.atomic-swaps.scope=lez-v0.2-local-devnet" \
  --label "org.logos-co.atomic-swaps.component=bedrock" \
  --network "$network" \
  --network-alias bedrock \
  --publish "127.0.0.1::18080" \
  --user "${LEZ_V02_UID}:${LEZ_V02_GID}" \
  --read-only \
  --cap-drop ALL \
  --security-opt no-new-privileges=true \
  --pids-limit 512 \
  --cpus 2 \
  --memory 4g \
  --stop-timeout 20 \
  --workdir /work \
  --env HOME=/tmp \
  --env POL_PROOF_DEV_MODE=true \
  --tmpfs /tmp:rw,noexec,nosuid,size=268435456,mode=1777 \
  --mount "type=bind,src=${source_dir}/bedrock,dst=/opt/lez-v0.2-source/bedrock,readonly" \
  --mount "type=bind,src=${source_dir}/bedrock/kzgrs_test_params,dst=/kzgrs_test_params,readonly" \
  --mount "type=bind,src=${run_dir}/config/deployment-settings.yaml,dst=/run-config/deployment-settings.yaml,readonly" \
  --mount "type=bind,src=${run_dir}/bedrock,dst=/work/state" \
  --entrypoint /usr/bin/logos-blockchain-node \
  ghcr.io/logos-blockchain/logos-blockchain@sha256:91d6c5bf07e07fcfba5e7cf07d21ee686a6bc4b9f6210f2d28bffbcad9a3729f \
  /opt/lez-v0.2-source/bedrock/node-config.yaml \
  --deployment /run-config/deployment-settings.yaml)"

containers[indexer]="$(docker create \
  --name "${project}-indexer" \
  --label "org.logos-co.atomic-swaps.run=${run_id}" \
  --label "org.logos-co.atomic-swaps.scope=lez-v0.2-local-devnet" \
  --label "org.logos-co.atomic-swaps.component=indexer" \
  --network "$network" \
  --network-alias indexer \
  --publish "127.0.0.1::8779" \
  --user "${LEZ_V02_UID}:${LEZ_V02_GID}" \
  --read-only \
  --cap-drop ALL \
  --security-opt no-new-privileges=true \
  --pids-limit 512 \
  --cpus 2 \
  --memory 2g \
  --stop-timeout 20 \
  --env HOME=/tmp \
  --env RUST_LOG=info \
  --env RISC0_SERVER_PATH=/usr/local/bin/r0vm \
  --tmpfs /tmp:rw,noexec,nosuid,size=268435456,mode=1777 \
  --mount "type=bind,src=${run_dir}/config/indexer_config.json,dst=/run-config/indexer_config.json,readonly" \
  --mount "type=bind,src=${run_dir}/indexer,dst=/var/lib/indexer_service" \
  --entrypoint /usr/local/bin/indexer_service \
  "$LEZ_V02_IMAGE" \
  /run-config/indexer_config.json --port 8779 --data-dir /var/lib/indexer_service)"

containers[sequencer]="$(docker create \
  --name "${project}-sequencer" \
  --label "org.logos-co.atomic-swaps.run=${run_id}" \
  --label "org.logos-co.atomic-swaps.scope=lez-v0.2-local-devnet" \
  --label "org.logos-co.atomic-swaps.component=sequencer" \
  --network "$network" \
  --network-alias sequencer \
  --publish "127.0.0.1::3040" \
  --user "${LEZ_V02_UID}:${LEZ_V02_GID}" \
  --read-only \
  --cap-drop ALL \
  --security-opt no-new-privileges=true \
  --pids-limit 1024 \
  --cpus 4 \
  --memory 8g \
  --stop-timeout 30 \
  --env HOME=/tmp \
  --env RUST_LOG=info \
  --env RISC0_SERVER_PATH=/usr/local/bin/r0vm \
  --tmpfs /tmp:rw,nosuid,size=2147483648,mode=1777 \
  --mount "type=bind,src=${run_dir}/config/sequencer_config.json,dst=/run-config/sequencer_config.json,readonly" \
  --mount "type=bind,src=${run_dir}/sequencer,dst=/var/lib/sequencer_service" \
  --entrypoint /usr/local/bin/sequencer_service \
  "$LEZ_V02_IMAGE" \
  /run-config/sequencer_config.json --port 3040)"

printf "%s\n" \
  "LEZ_V02_DOCKER_NETWORK=${network}" \
  "LEZ_V02_BEDROCK_CONTAINER=${containers[bedrock]}" \
  "LEZ_V02_INDEXER_CONTAINER=${containers[indexer]}" \
  "LEZ_V02_SEQUENCER_CONTAINER=${containers[sequencer]}" >>"$manifest"

rpc_call() {
  local url="$1"
  local payload="$2"
  local output="$3"
  curl -fsS --max-time 5 \
    -H 'content-type: application/json' \
    --data "$payload" "$url" >"$output" &&
    jq -e 'has("result") and (has("error") | not)' "$output" >/dev/null
}

assert_actor_preclaim_state() {
  local rpc_url="$1"
  local rpc_name="$2"
  local role="$3"
  local owner_id="$4"
  local vault_id="$5"
  local allocation="$6"
  local finalized_block_id="${7:-}"
  local owner_output
  local vault_output
  local nonces_output="${evidence_dir}/${rpc_name}-${role}-nonces-preclaim.json"
  local owner_payload
  local vault_payload
  local nonces_payload

  if [[ "$rpc_name" == "indexer" ]]; then
    if [[ ! "$finalized_block_id" =~ ^[1-9][0-9]*$ ]]; then
      echo "indexer actor readiness requires an exact finalized block ID" >&2
      return 1
    fi
    owner_output="${evidence_dir}/${rpc_name}-${role}-owner-preclaim-at-block-${finalized_block_id}.json"
    vault_output="${evidence_dir}/${rpc_name}-${role}-vault-preclaim-at-block-${finalized_block_id}.json"
    owner_payload="$(printf \
      '{"jsonrpc":"2.0","id":1,"method":"getAccountAtBlock","params":["%s",%s]}' \
      "$owner_id" "$finalized_block_id")"
    vault_payload="$(printf \
      '{"jsonrpc":"2.0","id":1,"method":"getAccountAtBlock","params":["%s",%s]}' \
      "$vault_id" "$finalized_block_id")"
  else
    owner_output="${evidence_dir}/${rpc_name}-${role}-owner-preclaim.json"
    vault_output="${evidence_dir}/${rpc_name}-${role}-vault-preclaim.json"
    owner_payload="$(printf \
      '{"jsonrpc":"2.0","id":1,"method":"getAccount","params":["%s"]}' \
      "$owner_id")"
    vault_payload="$(printf \
      '{"jsonrpc":"2.0","id":1,"method":"getAccount","params":["%s"]}' \
      "$vault_id")"
  fi
  nonces_payload="$(printf \
    '{"jsonrpc":"2.0","id":1,"method":"getAccountsNonces","params":[["%s","%s"]]}' \
    "$owner_id" "$vault_id")"

  rpc_call "$rpc_url" "$owner_payload" "$owner_output"
  rpc_call "$rpc_url" "$vault_payload" "$vault_output"
  jq -e '.result.balance == 0 and .result.nonce == 0' "$owner_output" >/dev/null
  jq -e --argjson allocation "$allocation" \
    '.result.balance == $allocation and .result.nonce == 0' "$vault_output" >/dev/null

  if [[ "$rpc_name" == "sequencer" ]]; then
    rpc_call "$rpc_url" "$nonces_payload" "$nonces_output"
    jq -e '.result == [0, 0]' "$nonces_output" >/dev/null
  fi
}

wait_for_bedrock() {
  local url="$1"
  local sample="$2"
  local height
  for _ in {1..90}; do
    if curl -fsS --max-time 5 "${url}/cryptarchia/info" >"$sample" &&
       jq -e '.cryptarchia_info.height >= 1' "$sample" >/dev/null; then
      height="$(jq -r '.cryptarchia_info.height' "$sample")"
      printf 'Bedrock ready at height %s\n' "$height"
      return 0
    fi
    sleep 2
  done
  echo "Bedrock did not produce a cryptarchia sample within 180 seconds" >&2
  return 1
}

wait_for_bedrock_advance() {
  local url="$1"
  local before="$2"
  local after="$3"
  for _ in {1..30}; do
    if curl -fsS --max-time 5 "${url}/cryptarchia/info" >"$after" &&
       jq -e --slurp -f "$cryptarchia_policy" "$before" "$after" >/dev/null; then
      return 0
    fi
    sleep 2
  done
  echo "Bedrock cryptarchia did not advance after readiness within 60 seconds" >&2
  return 1
}

wait_for_rpc() {
  local url="$1"
  local payload="$2"
  local output="$3"
  local description="$4"
  for _ in {1..90}; do
    if rpc_call "$url" "$payload" "$output"; then
      return 0
    fi
    sleep 2
  done
  echo "${description} did not become ready within 180 seconds" >&2
  return 1
}

wait_for_bootstrap_channel() {
  local url="$1"
  local output="$2"
  local status
  for _ in {1..120}; do
    status="$(curl -sS --max-time 5 -o "$output" -w "%{http_code}" "${url}/channel/${channel_id}")"
    if [[ "$status" == "200" ]] &&
       jq -e --arg public_key "$channel_id" --arg root "$genesis_channel_id" \
         ".accredited_keys == [\$public_key]
          and .configuration_threshold == 1
          and .withdraw_threshold == 1
          and .posting_timeframe == 0
          and .posting_timeout == 0
          and .balance == 0
          and (.tip_message | (type == \"string\" and length == 64 and . != \$root))" \
         "$output" >/dev/null; then
      return 0
    fi
    sleep 2
  done
  echo "LEZ sequencer did not create its accredited Bedrock channel within 240 seconds" >&2
  return 1
}

wait_for_channel_advance() {
  local url="$1"
  local before="$2"
  local after="$3"
  for _ in {1..60}; do
    if curl -fsS --max-time 5 "${url}/channel/${channel_id}" >"$after" &&
       jq -e --slurp --arg public_key "$channel_id" --arg root "$genesis_channel_id" \
         ".[0].accredited_keys == [\$public_key]
          and .[1].accredited_keys == [\$public_key]
          and .[1].configuration_threshold == 1
          and .[1].withdraw_threshold == 1
          and .[1].tip_slot >= .[0].tip_slot
          and (.[1].tip_slot > .[0].tip_slot or .[1].tip_message != .[0].tip_message)
          and (.[1].tip_message | (type == \"string\" and length == 64 and . != \$root))" \
         "$before" "$after" >/dev/null; then
      return 0
    fi
    sleep 2
  done
  echo "Bedrock channel did not advance after LEZ finality within 120 seconds" >&2
  return 1
}

published_url() {
  local service="$1"
  local container_port="$2"
  local container_id
  local endpoint
  local host_port
  for _ in {1..30}; do
    container_id="${containers[$service]}"
    endpoint="$(docker port "$container_id" "${container_port}/tcp" 2>/dev/null | tail -n 1)"
    host_port="${endpoint##*:}"
    if [[ "$host_port" =~ ^[1-9][0-9]*$ ]]; then
      docker inspect "$container_id" --format '{{json .HostConfig.PortBindings}}' \
        >"${evidence_dir}/${service}-host-port-bindings.json"
      docker inspect "$container_id" --format '{{json .NetworkSettings.Ports}}' \
        >"${evidence_dir}/${service}-effective-ports.json"
      printf "http://127.0.0.1:%s\n" "$host_port"
      return 0
    fi
    sleep 1
  done
  echo "Docker did not assign a dynamic loopback port for ${service}:${container_port}" >&2
  return 1
}

docker start "${containers[bedrock]}" >/dev/null
bedrock_url="$(published_url bedrock 18080)"
wait_for_bedrock "$bedrock_url" "${evidence_dir}/bedrock-cryptarchia-1.json"
wait_for_bedrock_advance "$bedrock_url" \
  "${evidence_dir}/bedrock-cryptarchia-1.json" \
  "${evidence_dir}/bedrock-cryptarchia-2.json"
missing_channel_body="${evidence_dir}/bedrock-channel-before-bootstrap.txt"
channel_status="$(curl -sS --max-time 5 -o "$missing_channel_body" \
  -w "%{http_code}" "${bedrock_url}/channel/${channel_id}")"
if ! lez_v02_is_missing_channel_response "$channel_status" "$missing_channel_body"; then
  echo "fresh Bedrock channel did not return the exact audited missing-channel response (HTTP ${channel_status})" >&2
  exit 1
fi

docker start "${containers[sequencer]}" >/dev/null
sequencer_url="$(published_url sequencer 3040)"
wait_for_rpc "$sequencer_url" \
  '{"jsonrpc":"2.0","id":1,"method":"checkHealth","params":[]}' \
  "${evidence_dir}/sequencer-health.json" "Sequencer RPC"
wait_for_bootstrap_channel "$bedrock_url" \
  "${evidence_dir}/bedrock-channel-after-bootstrap.json"
assert_actor_preclaim_state "$sequencer_url" "sequencer" "maker" \
  "$maker_account_id" "$maker_vault_account_id" "$maker_genesis_allocation"
assert_actor_preclaim_state "$sequencer_url" "sequencer" "taker" \
  "$taker_account_id" "$taker_vault_account_id" "$taker_genesis_allocation"

docker start "${containers[indexer]}" >/dev/null
indexer_url="$(published_url indexer 8779)"
wait_for_rpc "$indexer_url" \
  "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"checkHealth\",\"params\":[]}" \
  "${evidence_dir}/indexer-health.json" "Indexer RPC"
wait_for_rpc "$sequencer_url" \
  '{"jsonrpc":"2.0","id":1,"method":"getBlock","params":[1]}' \
  "${evidence_dir}/sequencer-block-1.json" "Sequencer block 1"
jq -e '.result != null' "${evidence_dir}/sequencer-block-1.json" >/dev/null
rpc_call "$sequencer_url" \
  '{"jsonrpc":"2.0","id":1,"method":"getChannelId","params":[]}' \
  "${evidence_dir}/sequencer-channel.json"
jq -e --arg channel "$channel_id" '.result == $channel' \
  "${evidence_dir}/sequencer-channel.json" >/dev/null
rpc_call "$sequencer_url" \
  '{"jsonrpc":"2.0","id":1,"method":"getProgramIds","params":[]}' \
  "${evidence_dir}/sequencer-programs.json"
jq -e '.result | type == "object" and length > 0' \
  "${evidence_dir}/sequencer-programs.json" >/dev/null
rpc_call "$sequencer_url" \
  '{"jsonrpc":"2.0","id":1,"method":"getLastBlockId","params":[]}' \
  "${evidence_dir}/sequencer-tip.json"
jq -e '.result >= 1' "${evidence_dir}/sequencer-tip.json" >/dev/null

finalized_id=""
for _ in {1..120}; do
  if rpc_call "$indexer_url" \
      '{"jsonrpc":"2.0","id":1,"method":"getLastFinalizedBlockId","params":[]}' \
      "${evidence_dir}/indexer-finalized-tip.json"; then
    finalized_id="$(jq -r '.result // empty' "${evidence_dir}/indexer-finalized-tip.json")"
    if [[ "$finalized_id" =~ ^[1-9][0-9]*$ ]] && (( finalized_id >= 2 )); then
      break
    fi
  fi
  sleep 2
done
if [[ ! "$finalized_id" =~ ^[1-9][0-9]*$ ]] || (( finalized_id < 2 )); then
  echo "Indexer did not expose a finalized non-genesis LEZ block within 240 seconds" >&2
  exit 1
fi

assert_actor_preclaim_state "$indexer_url" "indexer" "maker" \
  "$maker_account_id" "$maker_vault_account_id" "$maker_genesis_allocation" "$finalized_id"
assert_actor_preclaim_state "$indexer_url" "indexer" "taker" \
  "$taker_account_id" "$taker_vault_account_id" "$taker_genesis_allocation" "$finalized_id"

indexer_block_payload="$(printf '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getBlockById\",\"params\":[%s]}' "$finalized_id")"
sequencer_block_payload="$(printf '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getBlock\",\"params\":[%s]}' "$finalized_id")"
rpc_call "$indexer_url" "$indexer_block_payload" "${evidence_dir}/indexer-finalized-block.json"
rpc_call "$sequencer_url" "$sequencer_block_payload" "${evidence_dir}/sequencer-finalized-block.json"
jq -e '.result != null' "${evidence_dir}/indexer-finalized-block.json" >/dev/null
jq -e '.result != null' "${evidence_dir}/sequencer-finalized-block.json" >/dev/null
sequencer_block_binary="${evidence_dir}/sequencer-finalized-block.borsh"
jq -er ".result | select(type == \"string\")" \
  "${evidence_dir}/sequencer-finalized-block.json" | base64 --decode \
  >"$sequencer_block_binary"

sequencer_block_id="$(od -An -tu8 -N8 "$sequencer_block_binary" | tr -d " ")"
sequencer_prev_hash="$(xxd -p -s 8 -l 32 -c 32 "$sequencer_block_binary")"
sequencer_block_hash="$(xxd -p -s 40 -l 32 -c 32 "$sequencer_block_binary")"
sequencer_signature="$(xxd -p -s 80 -l 64 -c 64 "$sequencer_block_binary")"
indexer_block_id="$(jq -r ".result.header.block_id" \
  "${evidence_dir}/indexer-finalized-block.json")"
indexer_prev_hash="$(jq -r ".result.header.prev_block_hash" \
  "${evidence_dir}/indexer-finalized-block.json")"
indexer_block_hash="$(jq -r ".result.header.hash" \
  "${evidence_dir}/indexer-finalized-block.json")"
indexer_signature="$(jq -r ".result.header.signature" \
  "${evidence_dir}/indexer-finalized-block.json")"
if [[ "$sequencer_block_id" != "$finalized_id" || "$indexer_block_id" != "$finalized_id" ||
      "$sequencer_prev_hash" != "$indexer_prev_hash" ||
      "$sequencer_block_hash" != "$indexer_block_hash" ||
      "$sequencer_signature" != "$indexer_signature" ]]; then
  echo "Indexer decoded block and sequencer Borsh block identity mismatch at ${finalized_id}" >&2
  exit 1
fi

indexer_hash_payload="$(printf \
  "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getBlockByHash\",\"params\":[\"%s\"]}" \
  "$indexer_block_hash")"
rpc_call "$indexer_url" "$indexer_hash_payload" \
  "${evidence_dir}/indexer-finalized-block-by-hash.json"
if [[ "$(jq -S -c ".result" "${evidence_dir}/indexer-finalized-block.json")" != \
      "$(jq -S -c ".result" "${evidence_dir}/indexer-finalized-block-by-hash.json")" ]]; then
  echo "Indexer finalized block ID/hash lookup mismatch at ${finalized_id}" >&2
  exit 1
fi

wait_for_channel_advance "$bedrock_url" \
  "${evidence_dir}/bedrock-channel-after-bootstrap.json" \
  "${evidence_dir}/bedrock-channel-after-finality.json"

printf '%s\n' \
  "BEDROCK_RPC_URL=${bedrock_url}" \
  "LEZ_INDEXER_RPC_URL=${indexer_url}" \
  "LEZ_SEQUENCER_RPC_URL=${sequencer_url}" \
  "LEZ_FINALIZED_BLOCK_ID=${finalized_id}" \
  "LEZ_V02_ACTOR_GENESIS_FINALIZED_BLOCK_ID=${finalized_id}" \
  "LEZ_V02_ACTOR_GENESIS_READINESS=sequencer-and-exact-finalized-indexer-preclaim-state" \
  "LEZ_V02_READINESS_SCOPE=service-onboarding-finality-non-genesis-and-exact-finalized-actor-preclaim-state" >>"$manifest"

printf 'LEZ v0.2 isolated service-readiness passed: finalized_block_id=%s\n' "$finalized_id"
