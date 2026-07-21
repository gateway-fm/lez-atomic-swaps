#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

export LC_ALL=C
umask 077

readonly repo_root="$(pwd)"
readonly artifact_runner="scripts/run-m4-lez-artifact-tests.sh"
readonly lez_stack_runner="scripts/run-lez-v02-stack.sh"
readonly deployment_runner="scripts/run-m4-lez-local-deployment.sh"
readonly monero_runner="scripts/run-monero-e2e.sh"
readonly sidecar_manifest="compat/lez-v0_2-sidecar/Cargo.toml"
readonly deployer_manifest="compat/lez-v0.2-provisional/escrow/deployer/Cargo.toml"
readonly expected_rapidsnark_sha256="d4133227f845ff5bfa3672eb5b9c018a6a086bfa164b176bdaf76949c7d1f423"
readonly expected_gmp_sha256="0a910b420c3ad603c83c9dc2818c7ae05394c231ca23135c7b873e8e680ea41b"
readonly expected_fq_sha256="797b5d24bb8e8b088f811bddfff35f33973af9c797fb3812489cd42ba6a957d0"
readonly expected_fr_sha256="40f809394904682cb5517845cd3c2f936a5eb4609712534b573f552f2811fb82"

fail() {
  echo "M4 actual-claim runner failed: $*" >&2
  exit 1
}

emit_contract() {
  jq -n '
    {
      schema_version: 1,
      kind: "m4_actual_claim_poc_contract",
      milestone: "M4",
      source_binding: "clean_expected_commit",
      protocol_run_id_equals_lez_run_id: true,
      monero_child_run_suffix: "-xmr",
      all_effect_outputs_create_new: true,
      automatic_submission_retry: false,
      dynamic_literal_loopback_ports: true,
      public_runtime_resources: [],
      implemented_execute_through: "deployment",
      actor_onboarding_implemented: false,
      monero_launcher_implemented: true,
      monero_launcher_reachable_in_execute: false,
      monero_launcher_executed_in_certifying_replay: false,
      monero_owned_volume_count: 4,
      successful_claim_tail_implemented: false,
      phases: [
        "preflight", "build", "identity", "lez_stack", "deployment",
        "actor_onboarding", "monero_stack", "agreement", "journals", "tag13",
        "monero_funding", "sidecars", "release", "tag14_finality", "tag15",
        "tag15_finality", "extraction", "monero_sweep", "evidence", "cleanup"
      ],
      cleanup: {
        exact_resource_ledger: true,
        pid_start_time_binary_binding: true,
        foreign_sentinel_required: true,
        exact_monero_volume_capture: true,
        broad_cleanup_forbidden: true
      },
      composed_launchers: [
        "run-m4-lez-artifact-tests.sh", "run-lez-v02-stack.sh",
        "run-m4-lez-local-deployment.sh", "run-monero-e2e.sh"
      ],
      required_future_binaries: [
        "lez-v02-xmr-stage-a-compose", "lez-v02-xmr-stage-a-poc",
        "lez-v02-xmr-regtest-fund", "lez-v02-xmr-regtest-verify",
        "lez-v02-xmr-release-prepare", "lez-v0-2-xmr-release-service",
        "lez-v02-xmr-classify-finalized", "xmr-reference-tag15",
        "lez-adaptor-role-runner", "lez-v02-xmr-regtest-sweep",
        "bind-finalized-claim-sweep"
      ]
    }
  '
}

require_command() {
  command -v "$1" >/dev/null || fail "missing required command: $1"
}

require_owner_file() {
  local path="$1" label="$2" mode uid links
  [[ -f "$path" && ! -L "$path" ]] || fail "${label} is not a regular non-symlink file"
  uid="$(stat -c '%u' "$path")"
  mode="$(stat -c '%a' "$path")"
  links="$(stat -c '%h' "$path")"
  [[ "$uid" == "$(id -u)" && "$links" == 1 && $((8#$mode & 077)) == 0 ]] ||
    fail "${label} is not owner-private and single-link"
}

require_safe_parent() {
  local path="$1" label="$2" mode uid canonical
  [[ -d "$path" && ! -L "$path" ]] || fail "${label} is not a regular directory"
  canonical="$(readlink -f "$path")"
  [[ "$canonical" == "$path" ]] || fail "${label} is not canonical"
  uid="$(stat -c '%u' "$path")"
  mode="$(stat -c '%a' "$path")"
  [[ "$uid" == "$(id -u)" && $((8#$mode & 022)) == 0 ]] ||
    fail "${label} is not owned safely"
}

sha256_file() {
  sha256sum -- "$1" | sed 's/ .*//'
}

verify_native_library() {
  local filename="$1" expected="$2" path
  path="${RAPIDSNARK_LIB_DIR}/${filename}"
  [[ -f "$path" && ! -L "$path" ]] || fail "verified native library is unavailable: ${filename}"
  [[ "$(sha256_file "$path")" == "$expected" ]] || fail "native library identity drift: ${filename}"
}

manifest_value() {
  local key="$1" path="$2" count value
  count="$(rg -c "^${key}=" "$path" || true)"
  [[ "$count" == 1 ]] || fail "manifest key is missing or repeated: ${key}"
  value="$(sed -n "s/^${key}=//p" "$path")"
  [[ -n "$value" ]] || fail "manifest value is empty: ${key}"
  printf '%s\n' "$value"
}

source_preflight() {
  local actual_commit dirty
  [[ "${M4_EXPECTED_COMMIT:-}" =~ ^[0-9a-f]{40}$ ]] ||
    fail "M4_EXPECTED_COMMIT must be one lowercase 40-character Git object ID"
  actual_commit="$(git rev-parse --verify HEAD)"
  [[ "$actual_commit" == "$M4_EXPECTED_COMMIT" ]] || fail "HEAD differs from M4_EXPECTED_COMMIT"
  [[ "$(git rev-parse --show-toplevel)" == "$repo_root" ]] || fail "repository root identity drift"
  dirty="$(git status --porcelain=v1 --untracked-files=normal)"
  [[ -z "$dirty" ]] || fail "exact-commit replay requires a clean worktree"
  git diff --quiet --exit-code || fail "unstaged tracked source differs"
  git diff --cached --quiet --exit-code || fail "staged source differs"
}

configure_run_identity() {
  [[ "${RUN_ID:-}" =~ ^[a-z0-9][a-z0-9_-]{7,47}$ ]] ||
    fail "RUN_ID must be 8..48 lowercase letters, numbers, underscores, or hyphens"
  readonly run_id="$RUN_ID"
  readonly MONERO_RUN_ID="${run_id}-xmr"
  readonly artifact_run_id="${run_id}-artifact"
  [[ "$MONERO_RUN_ID" =~ ^[a-z0-9][a-z0-9_-]{7,63}$ ]] || fail "Monero child run ID is invalid"
  [[ "$artifact_run_id" =~ ^[a-z0-9][a-z0-9_-]{7,63}$ ]] || fail "artifact child run ID is invalid"
  readonly default_run_root="${repo_root}/.e2e/${run_id}/m4-actual-claim"
  readonly run_root="${M4_RUN_ROOT:-$default_run_root}"
  [[ "$run_root" == /* && "$(dirname "$run_root")" != "/" ]] || fail "M4_RUN_ROOT must be absolute"
  [[ ! -e "$run_root" && ! -L "$run_root" ]] || fail "refusing to reuse M4 run root"
  local parent
  parent="$(dirname "$run_root")"
  if [[ "$run_root" == "$default_run_root" ]]; then
    require_safe_parent "${repo_root}/.e2e" "M4 run namespace parent"
    [[ ! -e "${repo_root}/.e2e/${MONERO_RUN_ID}" ]] || fail "refusing reused Monero child state"
    [[ ! -e "${repo_root}/.e2e/${run_id}/lez-v02" ]] || fail "refusing reused LEZ child state"
  else
    require_safe_parent "$parent" "custom M4 run-root parent"
  fi
}

environment_preflight() {
  local command_name
  for command_name in awk bash base64 cargo chmod cut date flock git id jq mkdir \
      mktemp openssl readlink rg sed sha256sum stat tac xxd; do
    require_command "$command_name"
  done
  [[ "${RAPIDSNARK_LIB_DIR:-}" == /* && -d "$RAPIDSNARK_LIB_DIR" ]] ||
    fail "RAPIDSNARK_LIB_DIR must be an absolute verified library directory"
  [[ "${BINDGEN_EXTRA_CLANG_ARGS:-}" == '-I/usr/lib/gcc/x86_64-linux-gnu/13/include' ]] ||
    fail "BINDGEN_EXTRA_CLANG_ARGS differs from the pinned sidecar contract"
  [[ "${LEZ_M4_TOOL_DIR:-}" == /* && -d "$LEZ_M4_TOOL_DIR" ]] ||
    fail "LEZ_M4_TOOL_DIR must be an existing absolute pinned tool directory"
  [[ "${LOGOS_BLOCKCHAIN_CIRCUITS:-}" == /* && -d "$LOGOS_BLOCKCHAIN_CIRCUITS" ]] ||
    fail "LOGOS_BLOCKCHAIN_CIRCUITS must be an existing absolute directory"
  for path in "$artifact_runner" "$lez_stack_runner" "$deployment_runner" "$monero_runner"; do
    [[ -x "$path" && ! -L "$path" ]] || fail "composed launcher is unavailable: ${path}"
  done
  verify_native_library librapidsnark.a "$expected_rapidsnark_sha256"
  verify_native_library libgmp.a "$expected_gmp_sha256"
  verify_native_library libfq.a "$expected_fq_sha256"
  verify_native_library libfr.a "$expected_fr_sha256"
}

run_preflight() {
  source_preflight
  configure_run_identity
  environment_preflight
}

initialize_run_root() {
  if [[ "$run_root" == "$default_run_root" ]]; then
    local namespace="${repo_root}/.e2e/${run_id}"
    if [[ ! -e "$namespace" ]]; then
      mkdir -m 0700 "$namespace"
    fi
    require_safe_parent "$namespace" "M4 run namespace"
  fi
  mkdir -m 0700 "$run_root"
  readonly private_root="${run_root}/private"
  readonly evidence_root="${run_root}/evidence"
  readonly manifest_root="${run_root}/manifests"
  readonly log_root="${run_root}/logs"
  readonly build_root="${run_root}/build"
  mkdir -m 0700 "$private_root" "$evidence_root" "$manifest_root" "$log_root" "$build_root"
  mkdir -m 0700 "${evidence_root}/phases"
  readonly resource_ledger="${manifest_root}/resource-ledger.jsonl"
  readonly phase_ledger="${evidence_root}/phases.jsonl"
  (umask 077; : >"$resource_ledger"; : >"$phase_ledger")
  chmod 0600 "$resource_ledger" "$phase_ledger"
}

phase_index=0
record_phase() {
  local phase="$1" state="$2"
  phase_index=$((phase_index + 1))
  jq -cn --arg phase "$phase" --arg state "$state" --argjson index "$phase_index" \
    '{schema_version:1,index:$index,phase:$phase,state:$state}' >>"$phase_ledger"
}

record_resource() {
  local kind="$1" identity="$2" name="$3" start_ticks="${4:-}" binary_sha256="${5:-}"
  jq -cn --arg kind "$kind" --arg identity "$identity" --arg name "$name" \
    --arg start_ticks "$start_ticks" --arg binary_sha256 "$binary_sha256" \
    '{schema_version:1,kind:$kind,identity:$identity,name:$name,
      start_ticks:(if $start_ticks=="" then null else $start_ticks end),
      binary_sha256:(if $binary_sha256=="" then null else $binary_sha256 end)}' \
    >>"$resource_ledger"
}

process_start_ticks() {
  local pid="$1"
  awk '{print $22}' "/proc/${pid}/stat" 2>/dev/null
}

process_is_owned() {
  local pid="$1" start_ticks="$2" binary_sha256="$3" executable
  [[ "$pid" =~ ^[1-9][0-9]*$ && -r "/proc/${pid}/stat" ]] || return 1
  [[ "$(process_start_ticks "$pid")" == "$start_ticks" ]] || return 1
  executable="$(readlink -f "/proc/${pid}/exe" 2>/dev/null)" || return 1
  [[ -f "$executable" && "$(sha256_file "$executable")" == "$binary_sha256" ]]
}

safe_ephemeral_path() {
  local path="$1"
  case "$path" in
    "${run_root}/build/"* | "${repo_root}/.e2e/${run_id}/lez-v02/image-context") return 0 ;;
    *) return 1 ;;
  esac
}

cleanup_started=0
cleanup() {
  local source_status=$? cleanup_failed=0 sentinel_survived=false resources_absent=true
  local kind identity name start_ticks binary_sha256
  trap - EXIT
  set +e
  if [[ "$cleanup_started" != 1 ]]; then
    exit "$source_status"
  fi
  record_phase cleanup started
  while IFS=$'\t' read -r kind identity name start_ticks binary_sha256; do
    case "$kind" in
      process)
        if process_is_owned "$identity" "$start_ticks" "$binary_sha256"; then
          kill -TERM "$identity" 2>/dev/null
          for _ in {1..100}; do
            process_is_owned "$identity" "$start_ticks" "$binary_sha256" || break
            sleep 0.05
          done
          process_is_owned "$identity" "$start_ticks" "$binary_sha256" && kill -KILL "$identity" 2>/dev/null
        fi
        ;;
      container)
        docker container inspect "$identity" >/dev/null 2>&1 &&
          docker container rm --force "$identity" >/dev/null 2>&1
        ;;
      volume)
        docker volume inspect "$name" >/dev/null 2>&1 && docker volume rm "$name" >/dev/null 2>&1
        ;;
      network)
        docker network inspect "$name" >/dev/null 2>&1 && docker network rm "$name" >/dev/null 2>&1
        ;;
      image)
        docker image inspect "$name" >/dev/null 2>&1 && docker image rm "$name" >/dev/null 2>&1
        ;;
      ephemeral_path)
        if [[ -e "$name" || -L "$name" ]]; then
          if safe_ephemeral_path "$name" && [[ -d "$name" && ! -L "$name" ]]; then
            rm -rf -- "$name"
          else
            cleanup_failed=1
          fi
        fi
        ;;
      sentinel_network) ;;
      *) cleanup_failed=1 ;;
    esac
  done < <(tac "$resource_ledger" | jq -r '[.kind,.identity,.name,(.start_ticks//""),(.binary_sha256//"")] | @tsv')

  while IFS=$'\t' read -r kind identity name start_ticks binary_sha256; do
    case "$kind" in
      process) process_is_owned "$identity" "$start_ticks" "$binary_sha256" && resources_absent=false ;;
      container) docker container inspect "$identity" >/dev/null 2>&1 && resources_absent=false ;;
      volume) docker volume inspect "$name" >/dev/null 2>&1 && resources_absent=false ;;
      network) docker network inspect "$name" >/dev/null 2>&1 && resources_absent=false ;;
      image) docker image inspect "$name" >/dev/null 2>&1 && resources_absent=false ;;
      ephemeral_path) [[ -e "$name" || -L "$name" ]] && resources_absent=false ;;
      sentinel_network)
        if docker network inspect "$name" >/dev/null 2>&1; then sentinel_survived=true; else cleanup_failed=1; fi
        ;;
    esac
  done < <(jq -r '[.kind,.identity,.name,(.start_ticks//""),(.binary_sha256//"")] | @tsv' "$resource_ledger")

  if [[ "$sentinel_survived" == true ]]; then
    local sentinel_name
    sentinel_name="$(jq -r 'select(.kind=="sentinel_network") | .name' "$resource_ledger")"
    [[ -n "$sentinel_name" ]] && docker network rm "$sentinel_name" >/dev/null 2>&1 || cleanup_failed=1
  fi
  [[ "$resources_absent" == true ]] || cleanup_failed=1
  local cleanup_result=passed
  [[ "$cleanup_failed" == 0 ]] || cleanup_result=failed
  jq -n --arg result "$cleanup_result" --argjson source_status "$source_status" \
    --argjson absent "$resources_absent" --argjson sentinel "$sentinel_survived" \
    '{schema_version:1,result:$result,source_exit_status:$source_status,
      exact_run_resources_absent:$absent,sidecar_processes_absent:$absent,
      sidecar_ports_closed:$absent,foreign_sentinel_survived_exact_cleanup:$sentinel,
      foreign_resources_targeted:false,broad_cleanup_used:false}' \
    >"${evidence_root}/cleanup.json"
  chmod 0600 "${evidence_root}/cleanup.json"
  record_phase cleanup "$cleanup_result"
  if [[ "$cleanup_failed" != 0 ]]; then
    exit 1
  fi
  exit "$source_status"
}

trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

acquire_run_lock() {
  readonly lock_path="${repo_root}/.e2e/m4-actual-claim.execute.lock"
  exec {run_lock_fd}>"$lock_path"
  flock -n "$run_lock_fd" || fail "another M4 actual-claim execute path owns this checkout"
  chmod 0600 "$lock_path"
}

create_foreign_sentinel() {
  readonly sentinel_network="lez-atomic-swaps-m4-${run_id}-foreign-sentinel"
  if docker network inspect "$sentinel_network" >/dev/null 2>&1; then
    fail "foreign sentinel name already exists"
  fi
  docker network create --driver bridge \
    --label "org.logos-co.atomic-swaps.sentinel-for=${run_id}" \
    "$sentinel_network" >/dev/null
  record_resource sentinel_network "$sentinel_network" "$sentinel_network"
}

build_identity_and_artifact() {
  record_phase build started
  readonly sidecar_target="${build_root}/sidecar-target"
  record_resource ephemeral_path "$sidecar_target" "$sidecar_target"
  CARGO_TARGET_DIR="$sidecar_target" CARGO_NET_OFFLINE=true \
    cargo +1.96.0 build --locked --offline --manifest-path "$sidecar_manifest" \
      --example lez-v02-local-actor-identity
  readonly identity_binary="${sidecar_target}/debug/examples/lez-v02-local-actor-identity"
  [[ -x "$identity_binary" && ! -L "$identity_binary" ]] || fail "identity binary build is unavailable"

  readonly artifact_root="${build_root}/m4-artifact"
  record_resource ephemeral_path "${artifact_root}/target" "${artifact_root}/target"
  RUN_ID="$artifact_run_id" LEZ_M4_ARTIFACT_ROOT="$artifact_root" LEZ_M4_KEEP_BUILD=1 \
    LEZ_M4_TOOL_DIR="$LEZ_M4_TOOL_DIR" "$artifact_runner" execute
  readonly artifact_evidence="${artifact_root}/evidence/artifact.toml"
  require_owner_file "$artifact_evidence" "checked M4 artifact evidence"

  export RISC0_HOME="${LEZ_M4_TOOL_DIR}/home"
  export RISC0_SERVER_PATH="${RISC0_HOME}/extensions/v3.0.5-cargo-risczero-x86_64-unknown-linux-gnu/r0vm"
  export RISC0_DOCKER_CONTAINER_TAG="r0.1.94.1"
  export PATH="${LEZ_M4_TOOL_DIR}/cargo-home/bin:${LEZ_M4_TOOL_DIR}/bin:${PATH}"
  export CARGO_TARGET_DIR="${artifact_root}/target"
  CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=2 cargo +1.96.0 build --locked --offline \
    --manifest-path "$deployer_manifest" --bin lez-zec-escrow-v02-deployer
  readonly deployer_binary="${CARGO_TARGET_DIR}/debug/lez-zec-escrow-v02-deployer"
  [[ -x "$deployer_binary" && ! -L "$deployer_binary" ]] || fail "checked deployer is unavailable"
  write_build_manifest
  record_phase build completed
}

artifact_value() {
  local key="$1" count value
  count="$(rg -c "^${key} = " "$artifact_evidence" || true)"
  [[ "$count" == 1 ]] || fail "artifact evidence key is missing or repeated: ${key}"
  value="$(sed -n "s/^${key} = \"\(.*\)\"$/\1/p" "$artifact_evidence")"
  [[ -n "$value" ]] || fail "artifact evidence value is invalid: ${key}"
  printf '%s\n' "$value"
}

write_build_manifest() {
  local guest_path guest_expected guest_actual output
  guest_path="$(artifact_value elf_path)"
  guest_expected="$(artifact_value elf_sha256)"
  [[ "$guest_path" == /* && -f "$guest_path" && ! -L "$guest_path" ]] || fail "checked guest path is unsafe"
  guest_actual="$(sha256_file "$guest_path")"
  [[ "$guest_actual" == "$guest_expected" ]] || fail "checked guest digest differs from artifact evidence"
  output="${manifest_root}/build.json"
  [[ ! -e "$output" ]] || fail "build manifest already exists"
  jq -n --arg commit "$M4_EXPECTED_COMMIT" \
    --arg runner "$(sha256_file "${BASH_SOURCE[0]}")" \
    --arg artifact_runner "$(sha256_file "$artifact_runner")" \
    --arg stack_runner "$(sha256_file "$lez_stack_runner")" \
    --arg deployment_runner "$(sha256_file "$deployment_runner")" \
    --arg monero_runner "$(sha256_file "$monero_runner")" \
    --arg identity "$(sha256_file "$identity_binary")" \
    --arg deployer "$(sha256_file "$deployer_binary")" --arg guest "$guest_actual" \
    '{schema_version:1,source_commit:$commit,binary_sha256:{runner:$runner,
      artifact_runner:$artifact_runner,lez_stack_runner:$stack_runner,
      deployment_runner:$deployment_runner,monero_runner:$monero_runner,
      identity_provisioner:$identity,deployer:$deployer,checked_guest:$guest}}' >"$output"
  chmod 0600 "$output"
}

provision_identities() {
  record_phase identity started
  readonly identity_root="${private_root}/lez-identities"
  mkdir -m 0700 "$identity_root"
  local role output
  for role in maker taker; do
    output="${evidence_root}/${role}-lez-identity.json"
    "$identity_binary" --output-directory "${identity_root}/${role}" >"$output"
    chmod 0600 "$output"
    require_owner_file "${identity_root}/${role}/lez-signer.key" "${role} LEZ signer"
    jq -e '.schema=="lez-v0.2-local-actor-identity" and .version==2
      and (.account_id|type=="string") and (.account_id_hex|test("^[0-9a-f]{64}$"))
      and (.vault_account_id|type=="string") and (.vault_account_id_hex|test("^[0-9a-f]{64}$"))' \
      "$output" >/dev/null || fail "${role} identity evidence is invalid"
  done
  [[ "$(jq -r '.account_id_hex' "${evidence_root}/maker-lez-identity.json")" != \
     "$(jq -r '.account_id_hex' "${evidence_root}/taker-lez-identity.json")" ]] ||
    fail "fresh Maker and Taker identities collided"
  record_phase identity completed
}

capture_lez_resources() {
  local image project network container_ids count container_id
  image="$(manifest_value LEZ_V02_IMAGE "$lez_stack_manifest")"
  project="$(manifest_value COMPOSE_PROJECT_NAME "$lez_stack_manifest")"
  network="${project}-private"
  docker image inspect "$image" >/dev/null 2>&1 || fail "LEZ image is absent after keep-running launch"
  docker network inspect "$network" >/dev/null 2>&1 || fail "LEZ network is absent after keep-running launch"
  [[ "$(docker image inspect --format '{{ index .Config.Labels "org.logos-co.atomic-swaps.run" }}' "$image")" == "$run_id" ]] ||
    fail "LEZ image run label drift"
  [[ "$(docker network inspect --format '{{ index .Labels "org.logos-co.atomic-swaps.run" }}' "$network")" == "$run_id" ]] ||
    fail "LEZ network run label drift"
  record_resource image "$image" "$image"
  record_resource network "$network" "$network"
  container_ids="$(docker container ls --all --quiet --filter "label=org.logos-co.atomic-swaps.run=${run_id}")"
  count="$(sed '/^$/d' <<<"$container_ids" | wc -l | tr -d ' ')"
  [[ "$count" == 3 ]] || fail "LEZ resource capture did not find exactly three containers"
  while IFS= read -r container_id; do
    [[ -n "$container_id" ]] || continue
    [[ "$(docker inspect -f '{{ index .Config.Labels "org.logos-co.atomic-swaps.run" }}' "$container_id")" == "$run_id" ]] ||
      fail "LEZ container label drift"
    record_resource container "$container_id" "$container_id"
  done <<<"$container_ids"
  local image_context="${repo_root}/.e2e/${run_id}/lez-v02/image-context"
  [[ -d "$image_context" && ! -L "$image_context" ]] &&
    record_resource ephemeral_path "$image_context" "$image_context"
}

start_lez_stack() {
  record_phase lez_stack started
  local maker_account maker_vault taker_account taker_vault
  maker_account="$(jq -er '.account_id' "${evidence_root}/maker-lez-identity.json")"
  maker_vault="$(jq -er '.vault_account_id' "${evidence_root}/maker-lez-identity.json")"
  taker_account="$(jq -er '.account_id' "${evidence_root}/taker-lez-identity.json")"
  taker_vault="$(jq -er '.vault_account_id' "${evidence_root}/taker-lez-identity.json")"
  RUN_ID="$run_id" LEZ_V02_KEEP_RUNNING=1 LEZ_V02_SLOT_DURATION_SECONDS=1.0 \
    LEZ_V02_MAKER_ACCOUNT_ID="$maker_account" LEZ_V02_MAKER_VAULT_ACCOUNT_ID="$maker_vault" \
    LEZ_V02_TAKER_ACCOUNT_ID="$taker_account" LEZ_V02_TAKER_VAULT_ACCOUNT_ID="$taker_vault" \
    "$lez_stack_runner"
  readonly lez_stack_manifest="${repo_root}/.e2e/${run_id}/lez-v02/run.env"
  require_owner_file "$lez_stack_manifest" "LEZ stack manifest"
  [[ "$(manifest_value RUN_ID "$lez_stack_manifest")" == "$run_id" ]] || fail "LEZ stack run identity drift"
  capture_lez_resources
  record_phase lez_stack completed
}

deploy_m4_program() {
  record_phase deployment started
  readonly deployment_evidence="${evidence_root}/lez-deployment"
  [[ ! -e "$deployment_evidence" ]] || fail "deployment evidence root exists"
  M4_LEZ_RUN_ID="$run_id" M4_LEZ_STACK_MANIFEST="$lez_stack_manifest" \
    M4_LEZ_ARTIFACT_EVIDENCE="$artifact_evidence" M4_LEZ_DEPLOYER="$deployer_binary" \
    M4_LEZ_EXPECTED_DEPLOYER_SHA256="$(sha256_file "$deployer_binary")" \
    M4_LEZ_EVIDENCE_ROOT="$deployment_evidence" "$deployment_runner" execute
  require_owner_file "${deployment_evidence}/finality.json" "LEZ deployment finality evidence"
  jq -e '.result=="passed" and .send_attempts_this_process==1
    and .runtime_external_resources==[] and .public_rpc_used==false and .faucet_used==false' \
    "${deployment_evidence}/finality.json" >/dev/null || fail "LEZ deployment evidence is incomplete"
  record_phase deployment completed
}

actor_onboarding() {
  record_phase actor_onboarding not_implemented
  fail "actor_onboarding phase is not implemented; no Monero or swap effect was started"
}

capture_monero_resources() {
  local protocol_run="$run_id" container_ids count container_id project network image
  local volume_names volume_name
  # shellcheck disable=SC1090
  source "$monero_manifest"
  project="$MONERO_COMPOSE_PROJECT"
  network="$MONERO_NETWORK"
  image="$MONERO_IMAGE"
  export RUN_ID="$protocol_run"
  [[ "$project" == "lez-atomic-swaps-monero-${MONERO_RUN_ID}" ]] || fail "Monero project identity drift"
  [[ "$network" == "${project}-private" ]] || fail "Monero network identity drift"
  [[ "$image" == "lez-atomic-swaps-monero:${MONERO_RUN_ID}" ]] || fail "Monero image identity drift"
  docker image inspect "$image" >/dev/null 2>&1 || fail "Monero image is absent after keep-running launch"
  docker network inspect "$network" >/dev/null 2>&1 || fail "Monero network is absent after keep-running launch"
  [[ "$(docker image inspect --format '{{ index .Config.Labels "org.logos-co.atomic-swaps.run" }}' "$image")" == "$MONERO_RUN_ID" ]] ||
    fail "Monero image run label drift"
  [[ "$(docker network inspect --format '{{ index .Labels "org.logos-co.atomic-swaps.run" }}' "$network")" == "$MONERO_RUN_ID" ]] ||
    fail "Monero network run label drift"
  record_resource image "$image" "$image"
  record_resource network "$network" "$network"
  container_ids="$(docker container ls --all --quiet --filter "label=com.docker.compose.project=${project}")"
  count="$(sed '/^$/d' <<<"$container_ids" | wc -l | tr -d ' ')"
  [[ "$count" == 4 ]] || fail "Monero resource capture did not find exactly four containers"
  volume_names="$(docker volume ls --quiet \
    --filter "label=org.logos-co.atomic-swaps.run=${MONERO_RUN_ID}")"
  count="$(sed '/^$/d' <<<"$volume_names" | wc -l | tr -d ' ')"
  [[ "$count" == 4 ]] || fail "Monero resource capture did not find exactly four run-owned volumes"
  while IFS= read -r volume_name; do
    [[ -n "$volume_name" ]] || continue
    [[ "$(docker volume inspect --format '{{ index .Labels "org.logos-co.atomic-swaps.run" }}' "$volume_name")" == "$MONERO_RUN_ID" ]] ||
      fail "Monero volume run label drift"
    record_resource volume "$volume_name" "$volume_name"
  done <<<"$volume_names"
  while IFS= read -r container_id; do
    [[ -n "$container_id" ]] || continue
    [[ "$(docker container inspect --format '{{ index .Config.Labels "org.logos-co.atomic-swaps.run" }}' "$container_id")" == "$MONERO_RUN_ID" ]] ||
      fail "Monero container run label drift"
    record_resource container "$container_id" "$container_id"
  done <<<"$container_ids"

}
start_monero_child() {
  record_phase monero_stack started
  RUN_ID="$MONERO_RUN_ID" MONERO_E2E_KEEP_RUNNING=1 MONERO_E2E_REQUIRE_CLEAN=1 \
    "$monero_runner"
  readonly monero_manifest="${repo_root}/.e2e/${MONERO_RUN_ID}/monero/run.env"
  require_owner_file "$monero_manifest" "Monero child manifest"
  capture_monero_resources
  record_phase monero_stack completed
}

execute_run() {
  run_preflight
  require_command docker
  docker info >/dev/null || fail "Docker daemon is unavailable"
  initialize_run_root
  cleanup_started=1
  acquire_run_lock
  record_phase preflight completed
  create_foreign_sentinel
  build_identity_and_artifact
  provision_identities
  start_lez_stack
  deploy_m4_program
  actor_onboarding
  start_monero_child
  fail "agreement and later M4 phases are not implemented"
}

mode="${1:-}"
case "$mode" in
  contract)
    [[ "$#" == 1 ]] || fail "contract accepts no arguments"
    require_command jq
    emit_contract
    ;;
  preflight | verify-source)
    [[ "$#" == 1 ]] || fail "${mode} accepts no arguments"
    run_preflight
    jq -n --arg commit "$M4_EXPECTED_COMMIT" --arg run_id "$run_id" \
      --arg monero_run_id "$MONERO_RUN_ID" --arg run_root "$run_root" \
      '{schema_version:1,kind:"m4_actual_claim_preflight",result:"passed",
        source_commit:$commit,run_id:$run_id,monero_child_run_id:$monero_run_id,
        run_root:$run_root,docker_contacted:false}'
    ;;
  execute)
    [[ "$#" == 1 ]] || fail "execute accepts no arguments"
    execute_run
    ;;
  *) fail "expected contract, preflight, verify-source, or execute" ;;
esac
