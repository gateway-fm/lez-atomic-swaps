#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

export LC_ALL=C
umask 077

repo_root="$(pwd)"
readonly repo_root
readonly artifact_runner="scripts/run-m4-lez-artifact-tests.sh"
readonly lez_stack_runner="scripts/run-lez-v02-stack.sh"
readonly deployment_runner="scripts/run-m4-lez-local-deployment.sh"
readonly onboarding_runner="scripts/run-m4-lez-actor-onboarding.sh"
readonly monero_runner="scripts/run-monero-e2e.sh"
readonly agreement_runner="scripts/run-m4-xmr-agreement.sh"
readonly sidecar_manifest="compat/lez-v0_2-sidecar/Cargo.toml"
readonly deployer_manifest="compat/lez-v0.2-provisional/escrow/deployer/Cargo.toml"
readonly expected_rapidsnark_sha256="d4133227f845ff5bfa3672eb5b9c018a6a086bfa164b176bdaf76949c7d1f423"
readonly expected_gmp_sha256="0a910b420c3ad603c83c9dc2818c7ae05394c231ca23135c7b873e8e680ea41b"
readonly expected_fq_sha256="797b5d24bb8e8b088f811bddfff35f33973af9c797fb3812489cd42ba6a957d0"
readonly expected_fr_sha256="40f809394904682cb5517845cd3c2f936a5eb4609712534b573f552f2811fb82"
readonly m5_xmr_refund_clock_stall_samples=2
readonly m5_xmr_refund_clock_max_ticks=1
readonly m5_xmr_refund_clock_punish_guard_ms=60000
readonly m5_xmr_refund_clock_progress_timeout_seconds=60
readonly m5_xmr_application_mode="${M5_XMR_APPLICATION_MODE:-0}"
readonly m5_xmr_journey="${M5_XMR_JOURNEY:-claim}"
readonly m5_xmr_refund_delay_ms="${M5_XMR_REFUND_DELAY_MS:-900000}"
readonly m5_xmr_refund_window_ms=600000

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
      implemented_execute_through: "evidence",
      actor_onboarding_implemented: true,
      monero_launcher_implemented: true,
      monero_launcher_reachable_in_execute: true,
      monero_launcher_executed_in_certifying_replay: false,
      monero_funding_implemented: true,
      monero_funding_reachable_in_execute: true,
      monero_funding_executed_in_certifying_replay: false,
      monero_verification_implemented: true,
      monero_verification_reachable_in_execute: true,
      monero_verification_executed_in_certifying_replay: false,
      role_sidecar_launcher_contract_green: true,
      role_sidecar_launcher_reachable_in_execute: true,
      agreement_helper_contract_green: true,
      agreement_helper_implemented_through: "countersigned_stage_b",
      agreement_helper_submission_performed: false,
      agreement_helper_reachable_in_execute: true,
      journal_phase_started_before_agreement_helper: true,
      tag13_runner_implemented: true,
      tag13_runner_reachable_in_execute: true,
      tag13_runner_executed_in_certifying_replay: false,
      tag13_handoff_exporter_implemented: true,
      tag13_handoff_exporter_reachable_in_execute: true,
      tag13_handoff_exporter_executed_in_certifying_replay: false,
      available_unwired_launchers: [
        "run-m4-lez-sidecar.sh"
      ],
      monero_owned_volume_count: 4,
      successful_claim_tail_implemented: false,
      tag14_preparation_implemented: true,
      tag14_preparation_reachable_in_execute: true,
      tag14_preparation_executed_in_certifying_replay: false,
      tag14_publication_implemented: true,
      tag14_publication_reachable_in_execute: true,
      tag14_publication_executed_in_certifying_replay: false,
      tag14_finality_implemented: true,
      tag14_finality_reachable_in_execute: true,
      tag14_finality_executed_in_certifying_replay: false,
      tag15_signature_implemented: true,
      tag15_signature_reachable_in_execute: true,
      tag15_signature_executed_in_certifying_replay: false,
      tag15_publication_implemented: true,
      tag15_publication_reachable_in_execute: true,
      tag15_publication_executed_in_certifying_replay: false,
      tag15_finality_implemented: true,
      tag15_finality_reachable_in_execute: true,
      tag15_finality_executed_in_certifying_replay: false,
      extraction_implemented: true,
      extraction_reachable_in_execute: true,
      extraction_executed_in_certifying_replay: false,
      extraction_scalar_implemented: true,
      extraction_scalar_reachable_in_execute: true,
      extraction_scalar_executed_in_certifying_replay: false,
      monero_sweep_implemented: true,
      monero_sweep_reachable_in_execute: true,
      monero_sweep_executed_in_certifying_replay: false,
      evidence_binding_implemented: true,
      evidence_binding_reachable_in_execute: true,
      evidence_binding_executed_in_certifying_replay: false,
      tag14_preparation_implemented: true,
      tag14_preparation_reachable_in_execute: true,
      tag14_preparation_executed_in_certifying_replay: false,
      tag14_publication_implemented: true,
      tag14_publication_reachable_in_execute: true,
      tag14_publication_executed_in_certifying_replay: false,
      phases: [
        "preflight", "build", "identity", "lez_stack", "deployment",
        "actor_onboarding", "monero_stack", "agreement", "journals", "tag13", "tag13_handoff",
        "monero_funding", "sidecars", "release", "tag14_finality", "tag15",
        "tag15_finality", "extraction", "monero_sweep", "evidence", "cleanup"
      ],
      cleanup: {
        exact_resource_ledger: true,
        pid_start_time_binary_binding: true,
        foreign_sentinel_required: true,
        exact_monero_volume_capture: true,
        monero_child_preregistered: true,
        monero_child_sentinel_fallback: true,
        ledger_validated_before_cleanup: true,
        sentinel_survival_required_for_pass: true,
        docker_labels_revalidated_before_delete: true,
        tag13_no_retry_latch_before_submission: true,
        broad_cleanup_forbidden: true
      },
      composed_launchers: [
        "run-m4-lez-artifact-tests.sh", "run-lez-v02-stack.sh",
        "run-m4-lez-local-deployment.sh", "run-m4-lez-actor-onboarding.sh",
        "run-monero-e2e.sh", "run-m4-xmr-agreement.sh"
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
  for command_name in awk bash base64 cargo chmod cmp cp cut date diff flock git id \
      install jq ln mkdir mktemp openssl readlink rg sed sha256sum sort stat sync tac tr unlink wc xxd; do
    require_command "$command_name"
  done
  if [[ "$m5_xmr_application_mode" == 1 ]]; then
    for command_name in find mv ps setsid; do
      require_command "$command_name"
    done
  elif [[ "$m5_xmr_application_mode" != 0 ]]; then
    fail "M5_XMR_APPLICATION_MODE must be unset, 0, or 1"
  fi
  case "$m5_xmr_journey" in
    claim)
      [[ -z "${M5_XMR_REFUND_DELAY_MS:-}" ]] ||
        fail "M5_XMR_REFUND_DELAY_MS requires M5_XMR_JOURNEY=refund"
      ;;
    refund)
      [[ "$m5_xmr_application_mode" == 1 ]] ||
        fail "M5_XMR_JOURNEY=refund requires M5_XMR_APPLICATION_MODE=1"
      [[ "$m5_xmr_refund_delay_ms" =~ ^[0-9]+$ ]] &&
        (( m5_xmr_refund_delay_ms >= 600000 && m5_xmr_refund_delay_ms <= 3600000 )) ||
        fail "M5_XMR_REFUND_DELAY_MS must be 600000..3600000 milliseconds"
      ;;
    *) fail "M5_XMR_JOURNEY must be claim or refund" ;;
  esac
  [[ "${RAPIDSNARK_LIB_DIR:-}" == /* && -d "$RAPIDSNARK_LIB_DIR" ]] ||
    fail "RAPIDSNARK_LIB_DIR must be an absolute verified library directory"
  [[ "${BINDGEN_EXTRA_CLANG_ARGS:-}" == '-I/usr/lib/gcc/x86_64-linux-gnu/13/include' ]] ||
    fail "BINDGEN_EXTRA_CLANG_ARGS differs from the pinned sidecar contract"
  [[ "${LEZ_M4_TOOL_DIR:-}" == /* && -d "$LEZ_M4_TOOL_DIR" ]] ||
    fail "LEZ_M4_TOOL_DIR must be an existing absolute pinned tool directory"
  [[ "${LOGOS_BLOCKCHAIN_CIRCUITS:-}" == /* && -d "$LOGOS_BLOCKCHAIN_CIRCUITS" ]] ||
    fail "LOGOS_BLOCKCHAIN_CIRCUITS must be an existing absolute directory"
  for path in "$artifact_runner" "$lez_stack_runner" "$deployment_runner" \
      "$onboarding_runner" "$monero_runner" "$agreement_runner"; do
    [[ -x "$path" && ! -L "$path" ]] || fail "composed launcher is unavailable: ${path}"
  done
  verify_native_library librapidsnark.a "$expected_rapidsnark_sha256"
  verify_native_library libgmp.a "$expected_gmp_sha256"
  verify_native_library libfq.a "$expected_fq_sha256"
  verify_native_library libfr.a "$expected_fr_sha256"
  RUN_ID="$artifact_run_id" "$artifact_runner" verify-source
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
  readonly private_namespace="${TMPDIR:-/tmp}/lez-atomic-swaps-m4-${run_id}"
  [[ ! -e "$private_namespace" && ! -L "$private_namespace" ]] || fail "refusing to reuse M4 private namespace"
  mkdir -m 0700 "$private_namespace"
  readonly private_root="${private_namespace}/private"
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
  record_resource ephemeral_path "$private_namespace" "$private_namespace"
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
  local run_label="${6:-}"
  jq -cn --arg kind "$kind" --arg identity "$identity" --arg name "$name" \
    --arg start_ticks "$start_ticks" --arg binary_sha256 "$binary_sha256" --arg run_label "$run_label" \
    '{schema_version:1,kind:$kind,identity:$identity,name:$name,
      start_ticks:(if $start_ticks=="" then null else $start_ticks end),
      binary_sha256:(if $binary_sha256=="" then null else $binary_sha256 end),
      run_label:(if $run_label=="" then null else $run_label end)}' \
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

docker_resource_run_label_matches() {
  local kind="$1" identity="$2" expected="$3" actual
  [[ -n "$expected" ]] || return 1
  case "$kind" in
    container)
      actual="$(docker container inspect --format '{{ index .Config.Labels "org.logos-co.atomic-swaps.run" }}' "$identity" 2>/dev/null)" ||
        return 1
      ;;
    volume)
      actual="$(docker volume inspect --format '{{ index .Labels "org.logos-co.atomic-swaps.run" }}' "$identity" 2>/dev/null)" ||
        return 1
      ;;
    network)
      actual="$(docker network inspect --format '{{ index .Labels "org.logos-co.atomic-swaps.run" }}' "$identity" 2>/dev/null)" ||
        return 1
      ;;
    image)
      actual="$(docker image inspect --format '{{ index .Config.Labels "org.logos-co.atomic-swaps.run" }}' "$identity" 2>/dev/null)" ||
        return 1
      ;;
    *) return 1 ;;
  esac
  [[ "$actual" == "$expected" ]]
}

sentinel_label_matches() {
  local identity="$1" expected="$2" actual
  actual="$(docker network inspect --format '{{ index .Labels "org.logos-co.atomic-swaps.sentinel-for" }}' "$identity" 2>/dev/null)" ||
    return 1
  [[ "$actual" == "$expected" ]]
}

remove_labeled_docker_resource() {
  local kind="$1" identity="$2" expected="$3"
  docker_resource_run_label_matches "$kind" "$identity" "$expected" || return 1
  case "$kind" in
    container) docker container rm --force "$identity" >/dev/null 2>&1 ;;
    volume) docker volume rm "$identity" >/dev/null 2>&1 ;;
    network) docker network rm "$identity" >/dev/null 2>&1 ;;
    image) docker image rm "$identity" >/dev/null 2>&1 ;;
    *) return 1 ;;
  esac
}

cleanup_monero_child_by_label() {
  local run_label="$1" kind identity resources
  for kind in container volume network image; do
    case "$kind" in
      container)
        resources="$(docker container ls --all --quiet \
          --filter "label=org.logos-co.atomic-swaps.run=${run_label}")" || return 1
        ;;
      volume)
        resources="$(docker volume ls --quiet \
          --filter "label=org.logos-co.atomic-swaps.run=${run_label}")" || return 1
        ;;
      network)
        resources="$(docker network ls --quiet \
          --filter "label=org.logos-co.atomic-swaps.run=${run_label}")" || return 1
        ;;
      image)
        resources="$(docker image ls --quiet \
          --filter "label=org.logos-co.atomic-swaps.run=${run_label}")" || return 1
        ;;
    esac
    while IFS= read -r identity; do
      [[ -n "$identity" ]] || continue
      remove_labeled_docker_resource "$kind" "$identity" "$run_label" || return 1
    done <<<"$resources"
  done
  local child_sentinel="lez-atomic-swaps-monero-${run_label}-foreign-sentinel"
  if docker network inspect "$child_sentinel" >/dev/null 2>&1; then
    sentinel_label_matches "$child_sentinel" "$run_label" || return 1
    docker network rm "$child_sentinel" >/dev/null 2>&1 || return 1
  fi
}

docker_label_resources_absent() {
  local run_label="$1" kind resources
  for kind in container volume network image; do
    case "$kind" in
      container)
        resources="$(docker container ls --all --quiet \
          --filter "label=org.logos-co.atomic-swaps.run=${run_label}")" || return 1
        ;;
      volume)
        resources="$(docker volume ls --quiet \
          --filter "label=org.logos-co.atomic-swaps.run=${run_label}")" || return 1
        ;;
      network)
        resources="$(docker network ls --quiet \
          --filter "label=org.logos-co.atomic-swaps.run=${run_label}")" || return 1
        ;;
      image)
        resources="$(docker image ls --quiet \
          --filter "label=org.logos-co.atomic-swaps.run=${run_label}")" || return 1
        ;;
    esac
    [[ -z "$resources" ]] || return 1
  done
  local child_sentinel="lez-atomic-swaps-monero-${run_label}-foreign-sentinel"
  if docker network inspect "$child_sentinel" >/dev/null 2>&1; then
    return 1
  fi
}

safe_ephemeral_path() {
  local path="$1" canonical
  [[ "$path" == /* ]] || return 1
  canonical="$(readlink -f -- "$path")" || return 1
  [[ "$canonical" == "$path" ]] || return 1
  case "$canonical" in
    "${run_root}/build/"* | "${repo_root}/.e2e/${run_id}/lez-v02/image-context" | \
      "${private_namespace}" | "${private_root}/"*) return 0 ;;
    *) return 1 ;;
  esac
}

materialize_validated_resource_ledger() {
  local output="$1"
  local expected_sentinel="lez-atomic-swaps-m4-${run_id}-foreign-sentinel"
  [[ ! -e "$output" && ! -L "$output" ]] || return 1
  jq -s -er --arg run_id "$run_id" --arg monero_run_id "$MONERO_RUN_ID" \
    --arg expected_sentinel "$expected_sentinel" '
      def clean_string:
        type == "string"
        and length > 0
        and (test("[\u0000-\u001f\u007f]") | not);
      def nullable_clean_string:
        . == null or clean_string;
      def base:
        type == "object"
        and keys == [
          "binary_sha256", "identity", "kind", "name", "run_label",
          "schema_version", "start_ticks"
        ]
        and .schema_version == 1
        and (.kind | clean_string)
        and (.identity | clean_string)
        and (.name | clean_string)
        and (.start_ticks | nullable_clean_string)
        and (.binary_sha256 | nullable_clean_string)
        and (.run_label | nullable_clean_string);
      def no_process_metadata:
        .start_ticks == null and .binary_sha256 == null;
      def valid_row:
        base and (
          if .kind == "process" then
            (.identity | test("^[1-9][0-9]*$"))
            and (.start_ticks | type == "string" and test("^[0-9]+$"))
            and (.binary_sha256 | type == "string" and test("^[0-9a-f]{64}$"))
            and .run_label == null
          elif (.kind == "container" or .kind == "volume"
              or .kind == "network" or .kind == "image") then
            no_process_metadata
            and (.run_label == $run_id or .run_label == $monero_run_id)
          elif .kind == "monero_child" then
            no_process_metadata
            and .identity == $monero_run_id and .name == $monero_run_id
            and .run_label == $monero_run_id
          elif .kind == "ephemeral_path" then
            no_process_metadata and .identity == .name and .run_label == null
          elif .kind == "sentinel_network" then
            no_process_metadata
            and .identity == $expected_sentinel and .name == $expected_sentinel
            and .run_label == $run_id
          else false
          end
        );
      if length > 0
        and all(.[]; valid_row)
        and ([.[] | select(.kind == "sentinel_network")] | length == 1)
      then
        .[] |
        [.kind, .identity, .name, (.start_ticks // ""), (.binary_sha256 // ""),
         (.run_label // "")] |
        join("\u001f")
      else
        error("resource ledger violates its exact cleanup contract")
      end
    ' "$resource_ledger" >"$output" || {
      unlink "$output" 2>/dev/null
      return 1
    }
  chmod 0600 "$output" || return 1
  require_owner_file "$output" "validated cleanup resource ledger"
}

cleanup_started=0
cleanup() {
  local source_status=$? cleanup_failed=0 sentinel_survived=false resources_absent=true
  local kind identity name start_ticks binary_sha256 run_label
  local validated_ledger
  local ledger_valid=false index sentinel_name="" sentinel_expected=""
  local -a ledger_rows=() cleanup_failure_reasons=()
  trap - EXIT
  set +e
  if [[ "$m5_xmr_application_mode" == 1 && -n "${m5_application_daemon_pid:-}" ]]; then
    stop_m5_xmr_application_daemon || {
      cleanup_failed=1
      cleanup_failure_reasons+=("m5_application_daemon_stop_failed")
    }
  fi
  if [[ "$cleanup_started" != 1 ]]; then
    exit "$source_status"
  fi
  validated_ledger="${manifest_root}/resource-ledger.validated.usv"
  record_phase cleanup started
  if materialize_validated_resource_ledger "$validated_ledger" &&
     mapfile -t ledger_rows <"$validated_ledger" &&
     (( ${#ledger_rows[@]} > 0 )); then
    ledger_valid=true
  else
    cleanup_failed=1
    cleanup_failure_reasons+=("resource_ledger_validation_failed")
    resources_absent=false
  fi

  if [[ "$ledger_valid" == true ]]; then
    for ((index=${#ledger_rows[@]} - 1; index >= 0; index--)); do
      IFS=$'\x1f' read -r kind identity name start_ticks binary_sha256 run_label \
        <<<"${ledger_rows[index]}"
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
        if docker container inspect "$identity" >/dev/null 2>&1; then
          if docker_resource_run_label_matches container "$identity" "$run_label"; then
            docker container rm --force "$identity" >/dev/null 2>&1 || {
              cleanup_failed=1
              cleanup_failure_reasons+=("container_remove_failed")
            }
          else
            cleanup_failed=1
            cleanup_failure_reasons+=("container_label_mismatch")
          fi
        fi
        ;;
      volume)
        if docker volume inspect "$name" >/dev/null 2>&1; then
          if docker_resource_run_label_matches volume "$name" "$run_label"; then
            docker volume rm "$name" >/dev/null 2>&1 || {
              cleanup_failed=1
              cleanup_failure_reasons+=("volume_remove_failed")
            }
          else
            cleanup_failed=1
            cleanup_failure_reasons+=("volume_label_mismatch")
          fi
        fi
        ;;
      network)
        if docker network inspect "$name" >/dev/null 2>&1; then
          if docker_resource_run_label_matches network "$name" "$run_label"; then
            docker network rm "$name" >/dev/null 2>&1 || {
              cleanup_failed=1
              cleanup_failure_reasons+=("network_remove_failed")
            }
          else
            cleanup_failed=1
            cleanup_failure_reasons+=("network_label_mismatch")
          fi
        fi
        ;;
      image)
        if docker image inspect "$name" >/dev/null 2>&1; then
          if docker_resource_run_label_matches image "$name" "$run_label"; then
            docker image rm "$name" >/dev/null 2>&1 || {
              cleanup_failed=1
              cleanup_failure_reasons+=("image_remove_failed")
            }
          else
            cleanup_failed=1
            cleanup_failure_reasons+=("image_label_mismatch")
          fi
        fi
        ;;
      monero_child)
        cleanup_monero_child_by_label "$run_label" || {
          cleanup_failed=1
          cleanup_failure_reasons+=("monero_child_cleanup_failed")
        }
        ;;
      ephemeral_path)
        if [[ -e "$name" || -L "$name" ]]; then
          if safe_ephemeral_path "$name" && [[ -d "$name" && ! -L "$name" ]]; then
            rm -rf -- "$name"
          else
            cleanup_failed=1
            cleanup_failure_reasons+=("ephemeral_path_boundary_failed")
          fi
        fi
        ;;
      sentinel_network) ;;
      *) cleanup_failed=1; cleanup_failure_reasons+=("unknown_resource_kind") ;;
    esac
    done
  fi

  if [[ "$ledger_valid" == true ]]; then
    for ((index=0; index < ${#ledger_rows[@]}; index++)); do
      IFS=$'\x1f' read -r kind identity name start_ticks binary_sha256 run_label \
        <<<"${ledger_rows[index]}"
    case "$kind" in
      process) process_is_owned "$identity" "$start_ticks" "$binary_sha256" && resources_absent=false ;;
      container) docker container inspect "$identity" >/dev/null 2>&1 && resources_absent=false ;;
      volume) docker volume inspect "$name" >/dev/null 2>&1 && resources_absent=false ;;
      network) docker network inspect "$name" >/dev/null 2>&1 && resources_absent=false ;;
      image) docker image inspect "$name" >/dev/null 2>&1 && resources_absent=false ;;
      monero_child) docker_label_resources_absent "$run_label" || resources_absent=false ;;
      ephemeral_path) [[ -e "$name" || -L "$name" ]] && resources_absent=false ;;
      sentinel_network)
        if sentinel_label_matches "$name" "$run_label"; then
          sentinel_survived=true
          sentinel_name="$name"
          sentinel_expected="$run_label"
        else
          cleanup_failed=1
          cleanup_failure_reasons+=("sentinel_missing_or_label_mismatch")
        fi
        ;;
    esac
    done
  fi

  if [[ "$sentinel_survived" == true ]]; then
    if [[ -n "$sentinel_name" ]] && sentinel_label_matches "$sentinel_name" "$sentinel_expected"; then
      docker network rm "$sentinel_name" >/dev/null 2>&1 || {
        cleanup_failed=1
        cleanup_failure_reasons+=("sentinel_remove_failed")
      }
    else
      cleanup_failed=1
      cleanup_failure_reasons+=("sentinel_revalidation_failed")
    fi
  else
    cleanup_failed=1
    cleanup_failure_reasons+=("sentinel_cleanup_incomplete")
  fi
  # Cleanup requires both successful bounded removal and an absent final state.
  # Never erase an earlier identity, label, or removal failure.
  if [[ "$resources_absent" != true ]]; then
    cleanup_failed=1
    cleanup_failure_reasons+=("exact_run_resources_remain")
  fi
  if [[ -n "${sentinel_name:-}" ]] && docker network inspect "$sentinel_name" >/dev/null 2>&1; then
    cleanup_failed=1
    cleanup_failure_reasons+=("sentinel_remove_incomplete")
  fi
  if [[ "$cleanup_failed" != 0 && ${#cleanup_failure_reasons[@]} == 0 ]]; then
    cleanup_failure_reasons+=("unclassified_cleanup_failure")
  fi
  local cleanup_result=passed
  [[ "$cleanup_failed" == 0 ]] || cleanup_result=failed
  local no_retry_latch_preserved=false
  if [[ -n "${tag13_no_retry_latch:-}" && -f "$tag13_no_retry_latch" && ! -L "$tag13_no_retry_latch" ]]; then
    no_retry_latch_preserved=true
  fi
  local cleanup_failure_reasons_json
  cleanup_failure_reasons_json="$(printf '%s\n' "${cleanup_failure_reasons[@]}" | jq -Rsc 'split("\n") | map(select(length > 0))')"
  jq -n --arg result "$cleanup_result" --argjson source_status "$source_status" \
    --argjson absent "$resources_absent" --argjson sentinel "$sentinel_survived" \
    --argjson no_retry_latch_preserved "$no_retry_latch_preserved" \
    --argjson reasons "$cleanup_failure_reasons_json" \
    '{schema_version:2,result:$result,source_exit_status:$source_status,
      exact_run_resources_absent:$absent,sidecar_processes_absent:$absent,
      sidecar_ports_closed:$absent,foreign_sentinel_survived_exact_cleanup:$sentinel,
      tag13_no_retry_latch_preserved:$no_retry_latch_preserved,
      foreign_resources_targeted:false,broad_cleanup_used:false,
      failure_reasons:$reasons}' \
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
  record_resource sentinel_network "$sentinel_network" "$sentinel_network" "" "" "$run_id"
}

stage_executable() {
  local source="$1" destination="$2" label="$3" canonical uid mode links
  [[ -x "$source" && -f "$source" && ! -L "$source" ]] || fail "${label} build output is unsafe"
  [[ ! -e "$destination" && ! -L "$destination" ]] || fail "${label} staged path exists"
  install -m 0700 -- "$source" "$destination"
  canonical="$(readlink -f "$destination")"
  uid="$(stat -c '%u' "$destination")"
  mode="$(stat -c '%a' "$destination")"
  links="$(stat -c '%h' "$destination")"
  [[ "$canonical" == "$destination" && "$uid" == "$(id -u)" && "$mode" == 700 &&
     "$links" == 1 && -x "$destination" && ! -L "$destination" ]] ||
    fail "${label} staged executable identity is unsafe"
}

build_identity_and_artifact() {
  record_phase build started
  export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
  readonly sidecar_target="${build_root}/sidecar-target"
  record_resource ephemeral_path "$sidecar_target" "$sidecar_target"
  CARGO_TARGET_DIR="$sidecar_target" CARGO_NET_OFFLINE=true \
    cargo +1.96.0 build --locked --offline --manifest-path "$sidecar_manifest" \
      --bin lez-v02-vault-claim-poc --bin lez-v02-xmr-stage-a-compose \
      --bin lez-v02-xmr-stage-a-poc --bin lez-v02-bridge-poc --bin lez-v02-xmr-tag13-export \
      --bin lez-v02-xmr-regtest-fund --bin lez-v02-xmr-regtest-verify --bin lez-v02-xmr-regtest-sweep --example lez-v02-local-actor-identity
  if [[ "$m5_xmr_application_mode" == 1 && "$m5_xmr_journey" == refund ]]; then
    CARGO_TARGET_DIR="$sidecar_target" CARGO_NET_OFFLINE=true \
      cargo +1.96.0 build --locked --offline --manifest-path "$sidecar_manifest" \
        --bin lez-v02-local-clock-driver
  fi
  readonly identity_binary="${sidecar_target}/debug/examples/lez-v02-local-actor-identity"
  [[ -x "$identity_binary" && ! -L "$identity_binary" ]] || fail "identity binary build is unavailable"
  readonly vault_claim_binary="${sidecar_target}/debug/lez-v02-vault-claim-poc"
  [[ -x "$vault_claim_binary" && ! -L "$vault_claim_binary" ]] || fail "Vault Claim binary build is unavailable"

  readonly workspace_target="${build_root}/workspace-target"
  record_resource ephemeral_path "$workspace_target" "$workspace_target"
  CARGO_TARGET_DIR="$workspace_target" CARGO_NET_OFFLINE=true \
    cargo +1.96.0 build --locked --offline -p xmr-reference-actor --features sessions \
      --bin xmr-reference-actor --bin xmr-reference-tag15
  if [[ "$m5_xmr_application_mode" == 1 && "$m5_xmr_journey" == refund ]]; then
    CARGO_TARGET_DIR="$workspace_target" CARGO_NET_OFFLINE=true \
      cargo +1.96.0 build --locked --offline -p xmr-reference-actor --features sessions \
        --bin xmr-reference-tag16
  fi
  if [[ "$m5_xmr_application_mode" == 1 ]]; then
    CARGO_TARGET_DIR="$workspace_target" CARGO_NET_OFFLINE=true \
      cargo +1.96.0 build --locked --offline -p lez-maker-node \
        --bin lez-maker --bin lez-maker-daemon --bin lez-taker --bin xmr-maker-actor
  fi
  CARGO_TARGET_DIR="$workspace_target" CARGO_NET_OFFLINE=true \
    cargo +1.96.0 build --locked --offline -p lez-adaptor-role-runner \
      --bin lez-adaptor-role-runner
  readonly release_target="${build_root}/release-target"
  record_resource ephemeral_path "$release_target" "$release_target"
  CARGO_TARGET_DIR="$release_target" CARGO_NET_OFFLINE=true \
    cargo +1.96.0 build --locked --offline \
      --manifest-path compat/lez-v0_2-xmr-release-service/Cargo.toml \
      --bin lez-v02-xmr-release-prepare --bin lez-v0-2-xmr-release-service --bin lez-v02-xmr-classify-finalized

  readonly staged_binary_root="${build_root}/staged-binaries"
  record_resource ephemeral_path "$staged_binary_root" "$staged_binary_root"
  mkdir -m 0700 "$staged_binary_root"
  readonly agreement_actor_binary="${staged_binary_root}/xmr-reference-actor"
  readonly tag15_binary="${staged_binary_root}/xmr-reference-tag15"
  if [[ "$m5_xmr_application_mode" == 1 && "$m5_xmr_journey" == refund ]]; then
    readonly tag16_binary="${staged_binary_root}/xmr-reference-tag16"
    readonly local_clock_driver_binary="${staged_binary_root}/lez-v02-local-clock-driver"
  fi
  readonly agreement_role_runner_binary="${staged_binary_root}/lez-adaptor-role-runner"
  readonly agreement_composer_binary="${staged_binary_root}/lez-v02-xmr-stage-a-compose"
  readonly tag13_binary="${staged_binary_root}/lez-v02-xmr-stage-a-poc"
  readonly bridge_binary="${staged_binary_root}/lez-v02-bridge-poc"
  readonly tag13_export_binary="${staged_binary_root}/lez-v02-xmr-tag13-export"
  readonly monero_fund_binary="${staged_binary_root}/lez-v02-xmr-regtest-fund"
  readonly monero_verify_binary="${staged_binary_root}/lez-v02-xmr-regtest-verify"
  readonly monero_sweep_binary="${staged_binary_root}/lez-v02-xmr-regtest-sweep"
  readonly release_prepare_binary="${staged_binary_root}/lez-v02-xmr-release-prepare"
  readonly release_service_binary="${staged_binary_root}/lez-v0-2-xmr-release-service"
  readonly classifier_binary="${staged_binary_root}/lez-v02-xmr-classify-finalized"
  readonly vault_claim_staged_binary="${staged_binary_root}/lez-v02-vault-claim-poc"
  if [[ "$m5_xmr_application_mode" == 1 ]]; then
    readonly m5_lez_maker_binary="${staged_binary_root}/lez-maker"
    readonly m5_lez_maker_daemon_binary="${staged_binary_root}/lez-maker-daemon"
    readonly m5_lez_taker_binary="${staged_binary_root}/lez-taker"
    readonly m5_xmr_maker_actor_binary="${staged_binary_root}/xmr-maker-actor"
  fi
  stage_executable "$vault_claim_binary" "$vault_claim_staged_binary" "Vault Claim"
  if [[ "$m5_xmr_application_mode" == 1 ]]; then
    stage_executable "${workspace_target}/debug/lez-maker" "$m5_lez_maker_binary" "M5 Maker CLI"
    stage_executable "${workspace_target}/debug/lez-maker-daemon" \
      "$m5_lez_maker_daemon_binary" "M5 Maker daemon"
    stage_executable "${workspace_target}/debug/lez-taker" "$m5_lez_taker_binary" "M5 Taker CLI"
    stage_executable "${workspace_target}/debug/xmr-maker-actor" \
      "$m5_xmr_maker_actor_binary" "M5 XMR Maker actor"
  fi
  stage_executable "${workspace_target}/debug/xmr-reference-actor" \
    "$agreement_actor_binary" "agreement actor"
  stage_executable "${workspace_target}/debug/xmr-reference-tag15" "$tag15_binary" "Tag15 driver"
  if [[ "$m5_xmr_application_mode" == 1 && "$m5_xmr_journey" == refund ]]; then
    stage_executable "${workspace_target}/debug/xmr-reference-tag16" \
      "$tag16_binary" "Tag16 refund driver"
    stage_executable "${sidecar_target}/debug/lez-v02-local-clock-driver" \
      "$local_clock_driver_binary" "local finalized-clock driver"
  fi
  stage_executable "${workspace_target}/debug/lez-adaptor-role-runner" \
    "$agreement_role_runner_binary" "agreement role runner"
  stage_executable "${sidecar_target}/debug/lez-v02-xmr-stage-a-compose" \
    "$agreement_composer_binary" "stage-a composer"
  stage_executable "${sidecar_target}/debug/lez-v02-xmr-stage-a-poc" \
    "$tag13_binary" "tag13 runner"
  stage_executable "${sidecar_target}/debug/lez-v02-bridge-poc" "$bridge_binary" "LEZ sidecar bridge"
  stage_executable "${sidecar_target}/debug/lez-v02-xmr-tag13-export" "$tag13_export_binary" "Tag13 handoff exporter"
  stage_executable "${sidecar_target}/debug/lez-v02-xmr-regtest-fund" "$monero_fund_binary" "Monero funding"
  stage_executable "${sidecar_target}/debug/lez-v02-xmr-regtest-verify" "$monero_verify_binary" "Monero verification"
  stage_executable "${sidecar_target}/debug/lez-v02-xmr-regtest-sweep" "$monero_sweep_binary" "Monero sweep"
  stage_executable "${release_target}/debug/lez-v02-xmr-release-prepare" "$release_prepare_binary" "Tag14 preparation"
  stage_executable "${release_target}/debug/lez-v0-2-xmr-release-service" "$release_service_binary" "Tag14 release service"
  stage_executable "${release_target}/debug/lez-v02-xmr-classify-finalized" "$classifier_binary" "finalized effect classifier"
  readonly artifact_root="${build_root}/m4-artifact"
  record_resource ephemeral_path "${artifact_root}/target" "${artifact_root}/target"
  RUN_ID="$artifact_run_id" LEZ_M4_ARTIFACT_ROOT="$artifact_root" LEZ_M4_KEEP_BUILD=1 \
    LEZ_M4_TOOL_DIR="$LEZ_M4_TOOL_DIR" "$artifact_runner" execute
  readonly artifact_evidence="${artifact_root}/evidence/artifact.toml"
  require_owner_file "$artifact_evidence" "checked M4 artifact evidence"

  export RISC0_HOME="${LEZ_M4_TOOL_DIR}/home"
  export RISC0_SERVER_PATH="${RISC0_HOME}/extensions/v3.0.5-cargo-risczero-x86_64-unknown-linux-gnu/r0vm"
  export RISC0_DOCKER_CONTAINER_TAG="r0.1.94.1@sha256:c2f63fdd720337c0727e05c5e1733083baba04c00a864a89b0e3f4f8d92617be"
  export PATH="${LEZ_M4_TOOL_DIR}/cargo-home/bin:${LEZ_M4_TOOL_DIR}/bin:${PATH}"
  export CARGO_TARGET_DIR="${artifact_root}/target"
  CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" cargo +1.96.0 build --locked --offline \
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
    --arg onboarding_runner "$(sha256_file "$onboarding_runner")" \
    --arg monero_runner "$(sha256_file "$monero_runner")" \
    --arg agreement_runner "$(sha256_file "$agreement_runner")" \
    --arg identity "$(sha256_file "$identity_binary")" \
    --arg vault_claim "$(sha256_file "$vault_claim_binary")" \
    --arg agreement_actor "$(sha256_file "$agreement_actor_binary")" \
    --arg agreement_role_runner "$(sha256_file "$agreement_role_runner_binary")" \
    --arg agreement_composer "$(sha256_file "$agreement_composer_binary")" \
    --arg tag13 "$(sha256_file "$tag13_binary")" \
    --arg bridge "$(sha256_file "$bridge_binary")" --arg tag13_export "$(sha256_file "$tag13_export_binary")" \
    --arg deployer "$(sha256_file "$deployer_binary")" --arg guest "$guest_actual" \
    '{schema_version:1,source_commit:$commit,binary_sha256:{runner:$runner,
      artifact_runner:$artifact_runner,lez_stack_runner:$stack_runner,
      deployment_runner:$deployment_runner,onboarding_runner:$onboarding_runner,
      monero_runner:$monero_runner,agreement_runner:$agreement_runner,
      identity_provisioner:$identity,vault_claim:$vault_claim,
      agreement_actor:$agreement_actor,agreement_role_runner:$agreement_role_runner,
      agreement_composer:$agreement_composer,tag13_runner:$tag13,bridge:$bridge,tag13_export:$tag13_export,
      deployer:$deployer,checked_guest:$guest}}' >"$output"
  chmod 0600 "$output"
  if [[ "$m5_xmr_application_mode" == 1 ]]; then
    local m5_temporary
    m5_temporary="$(mktemp "${output}.m5.XXXXXX")"
    jq --arg maker "$(sha256_file "$m5_lez_maker_binary")" \
      --arg daemon "$(sha256_file "$m5_lez_maker_daemon_binary")" \
      --arg taker "$(sha256_file "$m5_lez_taker_binary")" \
      --arg xmr_actor "$(sha256_file "$m5_xmr_maker_actor_binary")" '.binary_sha256 += {m5_lez_maker:$maker,m5_lez_maker_daemon:$daemon,m5_lez_taker:$taker,m5_xmr_maker_actor:$xmr_actor}' \
      "$output" >"$m5_temporary"
    chmod 0600 "$m5_temporary"
    mv "$m5_temporary" "$output"
  fi
  require_owner_file "$output" "build identity manifest"
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
  record_resource image "$image" "$image" "" "" "$run_id"
  record_resource network "$network" "$network" "" "" "$run_id"
  container_ids="$(docker container ls --all --quiet --filter "label=org.logos-co.atomic-swaps.run=${run_id}")"
  count="$(sed '/^$/d' <<<"$container_ids" | wc -l | tr -d ' ')"
  [[ "$count" == 3 ]] || fail "LEZ resource capture did not find exactly three containers"
  while IFS= read -r container_id; do
    [[ -n "$container_id" ]] || continue
    [[ "$(docker inspect -f '{{ index .Config.Labels "org.logos-co.atomic-swaps.run" }}' "$container_id")" == "$run_id" ]] ||
      fail "LEZ container label drift"
    record_resource container "$container_id" "$container_id" "" "" "$run_id"
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
    LEZ_V02_R0VM="$RISC0_SERVER_PATH" \
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
  record_phase actor_onboarding started
  readonly actor_onboarding_evidence="${evidence_root}/lez-actor-onboarding"
  M4_ONBOARD_RUN_ID="$run_id" \
    M4_ONBOARD_STACK_MANIFEST="$lez_stack_manifest" \
    M4_ONBOARD_DEPLOYMENT_FINALITY="${deployment_evidence}/finality.json" \
    M4_ONBOARD_EVIDENCE_ROOT="$actor_onboarding_evidence" \
    M4_ONBOARD_PRIVATE_ROOT="$private_root" \
    M4_ONBOARD_MAKER_IDENTITY="${evidence_root}/maker-lez-identity.json" \
    M4_ONBOARD_TAKER_IDENTITY="${evidence_root}/taker-lez-identity.json" \
    M4_ONBOARD_MAKER_PRIVATE_KEY="${private_root}/lez-identities/maker/lez-signer.key" \
    M4_ONBOARD_TAKER_PRIVATE_KEY="${private_root}/lez-identities/taker/lez-signer.key" \
    M4_ONBOARD_VAULT_CLAIM_BIN="$vault_claim_staged_binary" \
    M4_ONBOARD_EXPECTED_VAULT_CLAIM_SHA256="$(sha256_file "$vault_claim_staged_binary")" \
    "$onboarding_runner" execute
  require_owner_file "${actor_onboarding_evidence}/summary.json" "actor-onboarding summary"
  jq -e '.result=="passed" and .total_submission_count==2
    and .actors.maker.submission_count==1 and .actors.taker.submission_count==1
    and .monero_or_swap_effects_started==false and .runtime_external_resources==[]
    and .public_rpc_used==false and .faucet_used==false' \
    "${actor_onboarding_evidence}/summary.json" >/dev/null || fail "actor-onboarding evidence is incomplete"
  record_phase actor_onboarding completed
}

parse_monero_manifest() {
  local line key value expected_key
  local -a expected_keys=(
    RUN_ID MONERO_COMPOSE_PROJECT MONERO_COMPOSE_FILE MONERO_IMAGE MONERO_NETWORK
    MONERO_DAEMON_HOST_PORT MONERO_FUNDING_WALLET_HOST_PORT
    MONERO_MAKER_WALLET_HOST_PORT MONERO_TAKER_WALLET_HOST_PORT
    MONERO_DAEMON_CONFIG MONERO_FUNDING_WALLET_CONFIG MONERO_MAKER_WALLET_CONFIG
    MONERO_TAKER_WALLET_CONFIG MONERO_DAEMON_ENDPOINT MONERO_FUNDING_WALLET_ENDPOINT
    MONERO_MAKER_WALLET_ENDPOINT MONERO_TAKER_WALLET_ENDPOINT
    MONERO_DAEMON_CREDENTIAL_FILE MONERO_DAEMON_USERNAME_FILE MONERO_DAEMON_PASSWORD_FILE
    MONERO_FUNDING_CREDENTIAL_FILE MONERO_FUNDING_RPC_USERNAME_FILE
    MONERO_FUNDING_RPC_PASSWORD_FILE MONERO_FUNDING_WALLET_PASSWORD_FILE
    MONERO_MAKER_CREDENTIAL_FILE MONERO_MAKER_RPC_USERNAME_FILE
    MONERO_MAKER_RPC_PASSWORD_FILE MONERO_MAKER_WALLET_PASSWORD_FILE
    MONERO_TAKER_CREDENTIAL_FILE MONERO_TAKER_RPC_USERNAME_FILE
    MONERO_TAKER_RPC_PASSWORD_FILE MONERO_TAKER_WALLET_PASSWORD_FILE
  )
  declare -gA monero_env=()
  while IFS= read -r line || [[ -n "$line" ]]; do
    [[ "$line" =~ ^export\ ([A-Z0-9_]+)=([-A-Za-z0-9_./:=]+)$ ]] ||
      fail "Monero manifest contains an unsafe or malformed line"
    key="${BASH_REMATCH[1]}"
    value="${BASH_REMATCH[2]}"
    [[ -z "${monero_env[$key]+present}" ]] || fail "Monero manifest repeats key: ${key}"
    monero_env["$key"]="$value"
  done <"$monero_manifest"
  [[ "${#monero_env[@]}" == "${#expected_keys[@]}" ]] ||
    fail "Monero manifest key count differs from its exact contract"
  for expected_key in "${expected_keys[@]}"; do
    [[ -n "${monero_env[$expected_key]+present}" ]] ||
      fail "Monero manifest omits key: ${expected_key}"
  done
  [[ "${monero_env[RUN_ID]}" == "$MONERO_RUN_ID" ]] || fail "Monero manifest run identity drift"
  [[ "${monero_env[MONERO_COMPOSE_PROJECT]}" == "lez-atomic-swaps-monero-${MONERO_RUN_ID}" ]] ||
    fail "Monero project identity drift"
  [[ "${monero_env[MONERO_NETWORK]}" == "${monero_env[MONERO_COMPOSE_PROJECT]}-private" ]] ||
    fail "Monero network identity drift"
  [[ "${monero_env[MONERO_IMAGE]}" == "lez-atomic-swaps-monero:${MONERO_RUN_ID}" ]] ||
    fail "Monero image identity drift"
  for key in MONERO_DAEMON_HOST_PORT MONERO_FUNDING_WALLET_HOST_PORT \
      MONERO_MAKER_WALLET_HOST_PORT MONERO_TAKER_WALLET_HOST_PORT; do
    [[ "${monero_env[$key]}" =~ ^[1-9][0-9]{0,4}$ ]] ||
      fail "Monero manifest port is invalid: ${key}"
  done
  [[ "${monero_env[MONERO_DAEMON_ENDPOINT]}" == \
     "http://127.0.0.1:${monero_env[MONERO_DAEMON_HOST_PORT]}" ]] ||
    fail "Monero daemon endpoint is not bound to its captured loopback port"
  [[ "${monero_env[MONERO_FUNDING_WALLET_ENDPOINT]}" == \
     "http://127.0.0.1:${monero_env[MONERO_FUNDING_WALLET_HOST_PORT]}" ]] ||
    fail "Monero funding-wallet endpoint drift"
  [[ "${monero_env[MONERO_MAKER_WALLET_ENDPOINT]}" == \
     "http://127.0.0.1:${monero_env[MONERO_MAKER_WALLET_HOST_PORT]}" ]] ||
    fail "Monero Maker endpoint drift"
  [[ "${monero_env[MONERO_TAKER_WALLET_ENDPOINT]}" == \
     "http://127.0.0.1:${monero_env[MONERO_TAKER_WALLET_HOST_PORT]}" ]] ||
    fail "Monero Taker endpoint drift"
  for key in MONERO_DAEMON_CREDENTIAL_FILE MONERO_DAEMON_USERNAME_FILE MONERO_DAEMON_PASSWORD_FILE \
      MONERO_FUNDING_CREDENTIAL_FILE MONERO_FUNDING_RPC_USERNAME_FILE \
      MONERO_FUNDING_RPC_PASSWORD_FILE MONERO_FUNDING_WALLET_PASSWORD_FILE \
      MONERO_MAKER_CREDENTIAL_FILE MONERO_MAKER_RPC_USERNAME_FILE \
      MONERO_MAKER_RPC_PASSWORD_FILE MONERO_MAKER_WALLET_PASSWORD_FILE \
      MONERO_TAKER_CREDENTIAL_FILE MONERO_TAKER_RPC_USERNAME_FILE \
      MONERO_TAKER_RPC_PASSWORD_FILE MONERO_TAKER_WALLET_PASSWORD_FILE; do
    [[ "${monero_env[$key]}" == "${repo_root}/.e2e/${MONERO_RUN_ID}/monero/credentials/"* ]] ||
      fail "Monero credential path escaped the child run root: ${key}"
    require_owner_file "${monero_env[$key]}" "Monero credential ${key}"
  done
  readonly monero_daemon_endpoint="${monero_env[MONERO_DAEMON_ENDPOINT]}"
  readonly monero_daemon_username_file="${monero_env[MONERO_DAEMON_USERNAME_FILE]}"
  readonly monero_daemon_password_file="${monero_env[MONERO_DAEMON_PASSWORD_FILE]}"
}

validate_monero_runtime_evidence() {
  readonly monero_runtime_evidence="${repo_root}/.e2e/${MONERO_RUN_ID}/monero/evidence/runtime.json"
  require_owner_file "$monero_runtime_evidence" "Monero runtime evidence"
  jq -e --arg run_id "$MONERO_RUN_ID" --arg daemon "${monero_env[MONERO_DAEMON_ENDPOINT]}" \
    --arg funding "${monero_env[MONERO_FUNDING_WALLET_ENDPOINT]}" \
    --arg maker "${monero_env[MONERO_MAKER_WALLET_ENDPOINT]}" \
    --arg taker "${monero_env[MONERO_TAKER_WALLET_ENDPOINT]}" '
      .schema_version==1 and .result=="passed" and .run_id==$run_id and .milestone=="M4"
      and .release.version=="0.18.5.1" and .chain.nettype=="fakechain"
      and .chain.offline==true and .chain.peers==0
      and .isolation.rpc_bindings_literal_loopback_only==true
      and .isolation.ip_masquerade==false
      and ([.components[] | [.role,.kind,.endpoint]] == [
        ["provisioner","monerod",$daemon],
        ["provisioner","funding-wallet-rpc",$funding],
        ["Maker","wallet-rpc",$maker],["Taker","wallet-rpc",$taker]])
      and (all(.components[]; (.container_id|type)=="string" and (.container_id|length)>0))
      and .local_funding.maker_unlocked==true and .local_funding.taker_unlocked==true
      and .runtime_external_resources==[] and .public_rpc_used==false
      and .faucet_used==false and .public_funds_used==false
    ' "$monero_runtime_evidence" >/dev/null || fail "Monero runtime evidence violates the M4 boundary"
}

capture_monero_resources() {
  local container_ids count container_id project network image
  local volume_names volume_name
  project="${monero_env[MONERO_COMPOSE_PROJECT]}"
  network="${monero_env[MONERO_NETWORK]}"
  image="${monero_env[MONERO_IMAGE]}"
  [[ "$project" == "lez-atomic-swaps-monero-${MONERO_RUN_ID}" ]] || fail "Monero project identity drift"
  [[ "$network" == "${project}-private" ]] || fail "Monero network identity drift"
  [[ "$image" == "lez-atomic-swaps-monero:${MONERO_RUN_ID}" ]] || fail "Monero image identity drift"
  docker image inspect "$image" >/dev/null 2>&1 || fail "Monero image is absent after keep-running launch"
  docker network inspect "$network" >/dev/null 2>&1 || fail "Monero network is absent after keep-running launch"
  [[ "$(docker image inspect --format '{{ index .Config.Labels "org.logos-co.atomic-swaps.run" }}' "$image")" == "$MONERO_RUN_ID" ]] ||
    fail "Monero image run label drift"
  [[ "$(docker network inspect --format '{{ index .Labels "org.logos-co.atomic-swaps.run" }}' "$network")" == "$MONERO_RUN_ID" ]] ||
    fail "Monero network run label drift"
  record_resource image "$image" "$image" "" "" "$MONERO_RUN_ID"
  record_resource network "$network" "$network" "" "" "$MONERO_RUN_ID"
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
    record_resource volume "$volume_name" "$volume_name" "" "" "$MONERO_RUN_ID"
  done <<<"$volume_names"
  while IFS= read -r container_id; do
    [[ -n "$container_id" ]] || continue
    [[ "$(docker container inspect --format '{{ index .Config.Labels "org.logos-co.atomic-swaps.run" }}' "$container_id")" == "$MONERO_RUN_ID" ]] ||
      fail "Monero container run label drift"
    record_resource container "$container_id" "$container_id" "" "" "$MONERO_RUN_ID"
  done <<<"$container_ids"
}
start_monero_child() {
  record_phase monero_stack started
  record_resource monero_child "$MONERO_RUN_ID" "$MONERO_RUN_ID" "" "" "$MONERO_RUN_ID"
  RUN_ID="$MONERO_RUN_ID" MONERO_E2E_KEEP_RUNNING=1 MONERO_E2E_REQUIRE_CLEAN=1 \
    "$monero_runner"
  readonly monero_manifest="${repo_root}/.e2e/${MONERO_RUN_ID}/monero/run.env"
  require_owner_file "$monero_manifest" "Monero child manifest"
  parse_monero_manifest
  validate_monero_runtime_evidence
  capture_monero_resources
  record_phase monero_stack completed
}

m5_application_daemon_pid=""
m5_application_daemon_start_ticks=""
m5_application_daemon_group=""
m5_application_daemon_binary_sha256=""
m5_application_daemon_ready=""
m5_application_daemon_had_chat=0
m5_last_stopped_daemon_pid=""
m5_last_stopped_daemon_group=""

m5_application_process_group_has_members() {
  local process_group="$1" snapshot
  [[ "$process_group" =~ ^[1-9][0-9]*$ ]] || return 0
  snapshot="$(ps -eo pgid=,stat=)" || return 0
  awk -v expected="$process_group" '$1 == expected && $2 !~ /^Z/ {found=1} END {exit !found}' <<<"$snapshot"
}

m5_application_process_group_members() {
  local process_group="$1" snapshot
  snapshot="$(ps -eo pid=,pgid=,stat=)" || return 1
  awk -v expected="$process_group" '$2 == expected && $3 !~ /^Z/ {print $1}' <<<"$snapshot"
}

start_m5_xmr_application_daemon() {
  local instance="$1" with_chat="$2" start="" observed_group="" published=""
  local log_file="${evidence_root}/m5-xmr-daemon-${instance}.log"
  local ready_file="${m5_xmr_runtime_root}/ready-${instance}"
  local -a arguments=(
    --socket "$m5_xmr_maker_socket"
    --database "$m5_xmr_maker_database"
    --ready-file "$ready_file"
    --delivery-directory "$m5_xmr_delivery_root"
    --delivery-signing-key-file "$m5_xmr_delivery_key"
  )
  [[ -z "$m5_application_daemon_pid" ]] || fail "M5 XMR Maker daemon is already tracked"
  [[ ! -e "$ready_file" && ! -L "$ready_file" ]] || fail "M5 XMR daemon ready path exists"
  [[ ! -e "$m5_xmr_maker_socket" && ! -L "$m5_xmr_maker_socket" ]] ||
    fail "M5 XMR owner socket survived a prior daemon"
  if [[ "$with_chat" == 1 ]]; then
    [[ ! -e "$m5_xmr_chat_socket" && ! -L "$m5_xmr_chat_socket" ]] ||
      fail "M5 XMR Chat socket survived a prior daemon"
    arguments+=(
      --chat-socket "$m5_xmr_chat_socket"
      --xmr-maker-agreement-public-key-file "$m5_xmr_maker_agreement_public_key"
      --xmr-private-view-key-file "$m5_xmr_maker_private_view_key"
      --xmr-actor-manifest-registry-file "$m5_xmr_actor_registry"
      --actor-supervisor
      --actor-attempt-timeout-milliseconds 120000
      --actor-poll-milliseconds 20
      --actor-requeue-delay-seconds 3600
      --actor-failure-backoff-seconds 30
      --actor-max-output-bytes 8192
    )
  fi
  setsid "$m5_lez_maker_daemon_binary" "${arguments[@]}" >"$log_file" 2>&1 &
  m5_application_daemon_pid=$!
  m5_application_daemon_binary_sha256="$(sha256_file "$m5_lez_maker_daemon_binary")"
  for _ in {1..100}; do
    start="$(process_start_ticks "$m5_application_daemon_pid")"
    if [[ -n "$start" ]] && process_is_owned "$m5_application_daemon_pid" "$start" \
      "$m5_application_daemon_binary_sha256"; then
      break
    fi
    sleep 0.01
  done
  [[ -n "$start" ]] || fail "M5 XMR Maker daemon did not expose a process identity"
  process_is_owned "$m5_application_daemon_pid" "$start" \
    "$m5_application_daemon_binary_sha256" || fail "M5 XMR Maker daemon identity drifted at launch"
  observed_group="$(ps -o pgid= -p "$m5_application_daemon_pid" | tr -d " ")"
  [[ "$observed_group" == "$m5_application_daemon_pid" ]] ||
    fail "M5 XMR Maker daemon does not own a fresh process group"
  m5_application_daemon_start_ticks="$start"
  m5_application_daemon_group="$observed_group"
  m5_application_daemon_ready="$ready_file"
  m5_application_daemon_had_chat="$with_chat"
  record_resource process "$m5_application_daemon_pid" "m5-xmr-daemon-${instance}" \
    "$m5_application_daemon_start_ticks" "$m5_application_daemon_binary_sha256"
  for _ in {1..1200}; do
    [[ -s "$ready_file" ]] && break
    process_is_owned "$m5_application_daemon_pid" "$m5_application_daemon_start_ticks" \
      "$m5_application_daemon_binary_sha256" ||
      fail "M5 XMR Maker daemon exited before readiness; inspect ${log_file}"
    sleep 0.05
  done
  require_owner_file "$ready_file" "M5 XMR Maker daemon readiness"
  published="$(sed -n "1p" "$ready_file")"
  [[ "$published" == "$m5_xmr_maker_socket" ]] || fail "M5 XMR readiness socket drift"
  [[ -S "$m5_xmr_maker_socket" ]] || fail "M5 XMR owner socket is unavailable"
  if [[ "$with_chat" == 1 ]]; then
    [[ -S "$m5_xmr_chat_socket" ]] || fail "M5 XMR Chat socket is unavailable"
  else
    [[ ! -e "$m5_xmr_chat_socket" && ! -L "$m5_xmr_chat_socket" ]] ||
      fail "Delivery-only daemon exposed Chat"
  fi
}

stop_m5_xmr_application_daemon() {
  local pid="${m5_application_daemon_pid:-}" process_group="${m5_application_daemon_group:-}"
  local start="${m5_application_daemon_start_ticks:-}" binary_sha="${m5_application_daemon_binary_sha256:-}"
  local ready="${m5_application_daemon_ready:-}" had_chat="${m5_application_daemon_had_chat:-0}"
  local status=0
  [[ -n "$pid" ]] || return 0
  process_is_owned "$pid" "$start" "$binary_sha" || return 1
  [[ "$process_group" == "$pid" ]] || return 1
  kill -INT -- "-${process_group}" 2>/dev/null || return 1
  for _ in {1..200}; do
    process_is_owned "$pid" "$start" "$binary_sha" || break
    sleep 0.05
  done
  if process_is_owned "$pid" "$start" "$binary_sha"; then
    kill -TERM -- "-${process_group}" 2>/dev/null || return 1
    for _ in {1..100}; do
      process_is_owned "$pid" "$start" "$binary_sha" || break
      sleep 0.05
    done
  fi
  if process_is_owned "$pid" "$start" "$binary_sha"; then
    kill -KILL -- "-${process_group}" 2>/dev/null || return 1
    for _ in {1..100}; do
      process_is_owned "$pid" "$start" "$binary_sha" || break
      sleep 0.05
    done
  fi
  process_is_owned "$pid" "$start" "$binary_sha" && return 1
  if wait "$pid" 2>/dev/null; then status=0; else status=$?; fi
  [[ "$status" == 0 ]] || return 1
  for _ in {1..100}; do
    m5_application_process_group_has_members "$process_group" || break
    sleep 0.05
  done
  if m5_application_process_group_has_members "$process_group"; then
    kill -TERM -- "-${process_group}" 2>/dev/null || return 1
    for _ in {1..100}; do
      m5_application_process_group_has_members "$process_group" || break
      sleep 0.05
    done
  fi
  if m5_application_process_group_has_members "$process_group"; then
    kill -KILL -- "-${process_group}" 2>/dev/null || return 1
    for _ in {1..100}; do
      m5_application_process_group_has_members "$process_group" || break
      sleep 0.05
    done
  fi
  m5_application_process_group_has_members "$process_group" && return 1
  [[ ! -e "/proc/${pid}" ]] || return 1
  [[ ! -e "$m5_xmr_maker_socket" && ! -L "$m5_xmr_maker_socket" ]] || return 1
  if [[ "$had_chat" == 1 ]]; then
    [[ ! -e "$m5_xmr_chat_socket" && ! -L "$m5_xmr_chat_socket" ]] || return 1
  fi
  [[ -z "$ready" || ( ! -e "$ready" && ! -L "$ready" ) ]] || return 1
  m5_last_stopped_daemon_pid="$pid"
  m5_last_stopped_daemon_group="$process_group"
  m5_application_daemon_pid=""
  m5_application_daemon_start_ticks=""
  m5_application_daemon_group=""
  m5_application_daemon_binary_sha256=""
  m5_application_daemon_ready=""
  m5_application_daemon_had_chat=0
}

prepare_m5_xmr_delivery_plan() {
  record_phase m5_xmr_application_plan started
  readonly m5_xmr_application_root="${private_root}/m5-xmr-application"
  readonly m5_xmr_runtime_root="${m5_xmr_application_root}/runtime"
  readonly m5_xmr_delivery_root="${m5_xmr_application_root}/delivery"
  readonly m5_xmr_removed_delivery_root="${m5_xmr_application_root}/delivery-removed"
  readonly m5_xmr_reconciled_delivery_root="${m5_xmr_application_root}/delivery-reconciled"
  readonly m5_xmr_delivery_key="${m5_xmr_application_root}/delivery.key"
  readonly m5_xmr_delivery_identity="${evidence_root}/m5-xmr-delivery-identity.json"
  readonly m5_xmr_maker_socket="${m5_xmr_runtime_root}/maker.sock"
  readonly m5_xmr_chat_socket="${m5_xmr_runtime_root}/chat.sock"
  readonly m5_xmr_maker_database="${m5_xmr_application_root}/maker.sqlite3"
  readonly m5_xmr_offer_id="m5-xmr-application-offer-001"
  readonly m5_xmr_reservation_id="m5-xmr-application-reservation-001"
  readonly m5_xmr_foreign_units=1000000000000
  readonly m5_xmr_lez_units=700
  readonly m5_xmr_offer_ttl_seconds=14400
  mkdir -m 0700 "$m5_xmr_application_root" "$m5_xmr_runtime_root"
  openssl rand -out "$m5_xmr_delivery_key" 32
  chmod 0600 "$m5_xmr_delivery_key"
  require_owner_file "$m5_xmr_delivery_key" "M5 XMR Delivery signing key"
  "$m5_lez_maker_binary" delivery-identity --signing-key-file "$m5_xmr_delivery_key" \
    >"$m5_xmr_delivery_identity"
  chmod 0600 "$m5_xmr_delivery_identity"
  require_owner_file "$m5_xmr_delivery_identity" "M5 XMR public Delivery identity"
  m5_xmr_delivery_public_key="$(jq -er 'if .schema_version==1 and (.public_key|test("^[0-9a-f]{66}$")) then .public_key else error("invalid Delivery identity") end' \
    "$m5_xmr_delivery_identity")"
  readonly m5_xmr_delivery_public_key

  start_m5_xmr_application_daemon delivery 0
  "$m5_lez_maker_binary" --socket "$m5_xmr_maker_socket" configure-pair \
    --request-id m5-xmr-route-create-001 --pair monero --direction taker-sells-lez \
    --enabled false --price-source local --minimum-foreign-units "$m5_xmr_foreign_units" \
    --maximum-foreign-units "$m5_xmr_foreign_units" --offer-ttl-seconds "$m5_xmr_offer_ttl_seconds" \
    >"${evidence_root}/m5-xmr-route-created.json"
  "$m5_lez_maker_binary" --socket "$m5_xmr_maker_socket" set-local-price \
    --request-id m5-xmr-price-create-001 --pair monero --direction taker-sells-lez \
    --lez-units-per-lot 7 --foreign-units-per-lot 10000000000 \
    >"${evidence_root}/m5-xmr-price-created.json"
  "$m5_lez_maker_binary" --socket "$m5_xmr_maker_socket" configure-pair \
    --request-id m5-xmr-route-enable-001 --expected-revision 1 --pair monero \
    --direction taker-sells-lez --enabled true --price-source local \
    --minimum-foreign-units "$m5_xmr_foreign_units" \
    --maximum-foreign-units "$m5_xmr_foreign_units" --offer-ttl-seconds "$m5_xmr_offer_ttl_seconds" \
    >"${evidence_root}/m5-xmr-route-enabled.json"
  "$m5_lez_maker_binary" --socket "$m5_xmr_maker_socket" publish-offer \
    --request-id m5-xmr-publish-001 --offer-id "$m5_xmr_offer_id" \
    --pair monero --direction taker-sells-lez >"${evidence_root}/m5-xmr-offer-published.json"
  readonly m5_xmr_plan_receipt="${evidence_root}/m5-xmr-plan.json"
  "$m5_lez_taker_binary" --delivery-directory "$m5_xmr_delivery_root" \
    --maker-public-key "$m5_xmr_delivery_public_key" --now-unix-seconds "$(date -u +%s)" \
    --pair monero --direction taker-sells-lez --plan-xmr-offer "$m5_xmr_offer_id" \
    --reservation-id "$m5_xmr_reservation_id" --foreign-units "$m5_xmr_foreign_units" \
    >"$m5_xmr_plan_receipt"
  chmod 0600 "$m5_xmr_plan_receipt"
  require_owner_file "$m5_xmr_plan_receipt" "M5 XMR authenticated plan"
  jq -e --arg offer "$m5_xmr_offer_id" --arg reservation "$m5_xmr_reservation_id" \
    --argjson foreign "$m5_xmr_foreign_units" --argjson lez "$m5_xmr_lez_units" '
      .schema_version==1 and .offer_id==$offer and .reservation_id==$reservation
      and (.signed_envelope_sha256|test("^[0-9a-f]{64}$"))
      and (.swap_id|test("^[0-9a-f]{64}$")) and .foreign_units==$foreign
      and .lez_units==$lez and .private_material_disclosed==false
    ' "$m5_xmr_plan_receipt" >/dev/null || fail "M5 XMR plan changed authenticated terms"
  m5_xmr_planned_swap_id="$(jq -er .swap_id "$m5_xmr_plan_receipt")"
  readonly m5_xmr_planned_swap_id
  stop_m5_xmr_application_daemon || fail "M5 XMR Delivery-only daemon did not stop exactly"
  record_phase m5_xmr_application_plan completed
}

compose_xmr_agreement() {
  record_phase agreement started
  readonly agreement_root="${private_root}/xmr-agreement"
  readonly agreement_stdout="${evidence_root}/xmr-agreement-receipt.json"
  [[ ! -e "$agreement_root" && ! -L "$agreement_root" ]] || fail "agreement output root exists"
  [[ ! -e "$agreement_stdout" && ! -L "$agreement_stdout" ]] || fail "agreement stdout evidence exists"
  local taker_owner maker_owner sequencer_url indexer_url now_seconds
  taker_owner="$(jq -er '.account_id_hex' "${evidence_root}/taker-lez-identity.json")"
  maker_owner="$(jq -er '.account_id_hex' "${evidence_root}/maker-lez-identity.json")"
  sequencer_url="$(manifest_value LEZ_SEQUENCER_RPC_URL "$lez_stack_manifest")"
  indexer_url="$(manifest_value LEZ_INDEXER_RPC_URL "$lez_stack_manifest")"
  now_seconds="$(date -u +%s)"
  readonly agreement_monero_amount_piconero=1000000000000
  readonly agreement_lez_amount=700
  if [[ "$m5_xmr_application_mode" == 1 && "$m5_xmr_journey" == refund ]]; then
    readonly refund_at_ms="$((now_seconds * 1000 + m5_xmr_refund_delay_ms))"
    readonly maker_xmr_funding_cutoff_ms="$((refund_at_ms - 300000))"
    readonly punish_at_ms="$((refund_at_ms + m5_xmr_refund_window_ms))"
  else
    # Keep the original actual-claim timing byte-for-byte equivalent by default.
    readonly maker_xmr_funding_cutoff_ms="$(((now_seconds + 14400) * 1000))"
    readonly refund_at_ms="$((maker_xmr_funding_cutoff_ms + 10000))"
    readonly punish_at_ms="$((refund_at_ms + 10000))"
  fi
  local -a m5_swap_id_argument=()
  if [[ "$m5_xmr_application_mode" == 1 ]]; then
    [[ "$agreement_monero_amount_piconero" == "$m5_xmr_foreign_units" && \
       "$agreement_lez_amount" == "$m5_xmr_lez_units" ]] || fail "M5 XMR plan/agreement terms drift"
    m5_swap_id_argument=(--swap-id "$m5_xmr_planned_swap_id")
  fi

  record_phase journals started
  "$agreement_runner" execute --run-id "$run_id" --output-root "$agreement_root" \
    --taker-lez-owner "$taker_owner" --maker-lez-owner "$maker_owner" \
    --sequencer-url "$sequencer_url" --indexer-url "$indexer_url" \
    --monero-daemon-url "$monero_daemon_endpoint" \
    --monero-rpc-username-file "$monero_daemon_username_file" \
    --monero-rpc-password-file "$monero_daemon_password_file" \
    --monero-amount-piconero "$agreement_monero_amount_piconero" \
    --lez-amount "$agreement_lez_amount" \
    --maker-xmr-funding-cutoff-ms "$maker_xmr_funding_cutoff_ms" \
    --refund-at-ms "$refund_at_ms" --punish-at-ms "$punish_at_ms" \
    "${m5_swap_id_argument[@]}" \
    --actor-bin "$agreement_actor_binary" --role-runner-bin "$agreement_role_runner_binary" \
    --composer-bin "$agreement_composer_binary" >"$agreement_stdout"
  chmod 0600 "$agreement_stdout"

  readonly agreement_receipt="${agreement_root}/agreement-receipt.json"
  readonly agreement_stage_a="${agreement_root}/exchange/agreement-stage-a.bin"
  readonly agreement_stage_b="${agreement_root}/stage-b/stage-b.bin"
  readonly agreement_composer_receipt="${agreement_root}/exchange/stage-a-composer-receipt.json"
  require_owner_file "$agreement_stdout" "agreement stdout receipt"
  require_owner_file "$agreement_receipt" "agreement internal receipt"
  require_owner_file "$agreement_stage_a" "countersigned Stage A"
  require_owner_file "$agreement_stage_b" "countersigned Stage B"
  require_owner_file "$agreement_composer_receipt" "Stage-A composer receipt"
  cmp -- "$agreement_stdout" "$agreement_receipt" || fail "agreement stdout differs from internal receipt"
  jq -e --arg run_id "$run_id" --arg monero "$agreement_monero_amount_piconero" \
    --arg lez "$agreement_lez_amount" --arg cutoff "$maker_xmr_funding_cutoff_ms" \
    --arg refund "$refund_at_ms" --arg punish "$punish_at_ms" \
    --arg stage_a "$(sha256_file "$agreement_stage_a")" \
    --arg stage_b "$(sha256_file "$agreement_stage_b")" '
      .schema_version==1 and .kind=="m4_xmr_agreement_receipt" and .result=="passed"
      and .run_id==$run_id and (.swap_id|test("^[0-9a-f]{64}$"))
      and .stage_a_sha256==$stage_a and .stage_b_sha256==$stage_b
      and .requested_terms=={monero_amount_piconero:$monero,lez_amount:$lez,
        maker_xmr_funding_cutoff_ms:$cutoff,refund_at_ms:$refund,punish_at_ms:$punish}
      and .terms_bound_to_stage_material_by_helper==false
      and .composer_receipt_validation_scope=="schema_shape_and_unsigned_wire_length_only"
      and .composer_receipt_wire_bytes_matched_output==true
      and .submission_performed==false and .stage_a_rpc_scope=="read_only"
      and .sessions_equal_across_roles==true and .taker_claim_material_private==true
      and .refund_presignatures_equal==true and .stage_b_countersigned==true
    ' "$agreement_receipt" >/dev/null || fail "agreement receipt violates the exact M4 boundary"
  if [[ "$m5_xmr_application_mode" == 1 ]]; then
    jq -e --arg swap "$m5_xmr_planned_swap_id" '.swap_id==$swap' "$agreement_receipt" >/dev/null ||
      fail "M5 XMR plan swap ID did not survive agreement composition"
  fi
  jq -e '(.wire_bytes|type)=="number" and .wire_bytes>0
    and (.agreement_commitment|test("^[0-9a-f]{64}$"))
    and (.monero_genesis_hash|test("^[0-9a-f]{64}$"))
    and (.lez_genesis_hash|test("^[0-9a-f]{64}$"))' "$agreement_composer_receipt" >/dev/null ||
    fail "agreement composer receipt is incomplete"

  local role purpose
  for role in maker taker; do
    require_owner_file "${agreement_root}/stage-b/private/${role}.sqlite" "${role} role journal"
    for purpose in claim refund; do
      require_owner_file "${agreement_root}/material/${role}-sessions/${purpose}.json" \
        "${role} ${purpose} session"
    done
  done
  cmp -- "${agreement_root}/material/maker-sessions/claim.json" \
    "${agreement_root}/material/taker-sessions/claim.json" ||
    fail "role-local claim sessions differ"
  cmp -- "${agreement_root}/material/maker-sessions/refund.json" \
    "${agreement_root}/material/taker-sessions/refund.json" ||
    fail "role-local refund sessions differ"
  [[ ! "${agreement_root}/material/taker-sessions/claim.json" -ef \
     "${agreement_root}/material/taker-sessions/refund.json" ]] ||
    fail "Taker claim/refund sessions alias one inode"
  require_owner_file "${agreement_root}/stage-b/private/taker-outbox/claim-partial.json" \
    "private Taker claim partial"
  require_owner_file "${agreement_root}/stage-b/private/taker-outbox/claim-presignature.json" \
    "private Taker claim presignature"
  [[ ! -e "${agreement_root}/stage-b/exchange/claim/taker-partial.json" &&
     ! -L "${agreement_root}/stage-b/exchange/claim/taker-partial.json" ]] ||
    fail "Taker claim partial crossed the exchange boundary"
  [[ ! -e "${agreement_root}/stage-b/exchange/claim/taker-presignature.json" &&
     ! -L "${agreement_root}/stage-b/exchange/claim/taker-presignature.json" ]] ||
    fail "Taker claim presignature crossed the exchange boundary"
  cmp -- "${agreement_root}/stage-b/exchange/refund/maker-presignature.json" \
    "${agreement_root}/stage-b/exchange/refund/taker-presignature.json" ||
    fail "refund presignatures differ"
  record_phase journals completed
  record_phase agreement completed
}

m5_delivery_offer_files_absent() {
  local directory="$1"
  [[ -d "$directory" && ! -L "$directory" ]] || return 1
  [[ -z "$(find "$directory" -mindepth 1 -maxdepth 1 -name "*.offer.json" -print -quit)" ]]
}

require_no_sqlite_sidecars() {
  local database="$1" label="$2" suffix
  for suffix in -wal -shm -journal; do
    [[ ! -e "${database}${suffix}" && ! -L "${database}${suffix}" ]] ||
      fail "${label} has a forbidden SQLite sidecar: ${suffix}"
  done
}

write_m5_journal_snapshot() {
  local output="$1" maker_journal="$2" taker_journal="$3"
  [[ ! -e "$output" && ! -L "$output" ]] || fail "M5 XMR journal snapshot exists"
  require_owner_file "$maker_journal" "M5 XMR Maker role journal"
  require_owner_file "$taker_journal" "M5 XMR Taker role journal"
  require_no_sqlite_sidecars "$maker_journal" "M5 XMR Maker role journal"
  require_no_sqlite_sidecars "$taker_journal" "M5 XMR Taker role journal"
  jq -n --arg maker_path "$maker_journal" --arg taker_path "$taker_journal" \
    --argjson maker_device "$(stat -c %d "$maker_journal")" \
    --argjson maker_inode "$(stat -c %i "$maker_journal")" \
    --argjson maker_size "$(stat -c %s "$maker_journal")" \
    --arg maker_sha256 "$(sha256_file "$maker_journal")" \
    --argjson taker_device "$(stat -c %d "$taker_journal")" \
    --argjson taker_inode "$(stat -c %i "$taker_journal")" \
    --argjson taker_size "$(stat -c %s "$taker_journal")" \
    --arg taker_sha256 "$(sha256_file "$taker_journal")" '
      {schema_version:1,sqlite_sidecars_present:false,
       maker:{path:$maker_path,device:$maker_device,inode:$maker_inode,size:$maker_size,sha256:$maker_sha256},
       taker:{path:$taker_path,device:$taker_device,inode:$taker_inode,size:$taker_size,sha256:$taker_sha256}}
    ' >"$output"
  chmod 0600 "$output"
  require_owner_file "$output" "M5 XMR journal snapshot"
}

write_m5_application_artifact_snapshot() {
  local output="$1" path
  [[ ! -e "$output" && ! -L "$output" ]] || fail "M5 XMR artifact snapshot exists"
  [[ -z "$(find "$m5_xmr_maker_actor_root" "$m5_xmr_taker_actor_root" -type l -print -quit)" ]] ||
    fail "M5 XMR actor tree contains a symlink"
  {
    for path in "$m5_xmr_taker_receipt" "$m5_xmr_maker_role_journal" "$m5_xmr_taker_role_journal"; do
      require_owner_file "$path" "M5 XMR immutable application artifact"
      printf "%s\t%s\t%s\t%s\t%s\n" "$(stat -c %d "$path")" "$(stat -c %i "$path")" \
        "$(stat -c %s "$path")" "$(sha256_file "$path")" "$path"
    done
    while IFS= read -r -d "" path; do
      require_owner_file "$path" "M5 XMR immutable actor artifact"
      printf "%s\t%s\t%s\t%s\t%s\n" "$(stat -c %d "$path")" "$(stat -c %i "$path")" \
        "$(stat -c %s "$path")" "$(sha256_file "$path")" "$path"
    done < <(find "$m5_xmr_maker_actor_root" "$m5_xmr_taker_actor_root" -type f -print0 | sort -z)
  } | sort >"$output"
  chmod 0600 "$output"
  require_owner_file "$output" "M5 XMR application artifact snapshot"
}

run_m5_xmr_taker_acceptance() {
  local output="$1" use_delivery="$2"
  local -a arguments=(
    --maker-public-key "$m5_xmr_delivery_public_key"
    --now-unix-seconds "$(date -u +%s)"
    --pair monero
    --direction taker-sells-lez
    --accept-xmr-offer "$m5_xmr_offer_id"
    --chat-socket "$m5_xmr_chat_socket"
    --reservation-id "$m5_xmr_reservation_id"
    --foreign-units "$m5_xmr_foreign_units"
    --xmr-stage-a-file "$agreement_stage_a"
    --xmr-activation-file "$agreement_stage_b"
    --xmr-source-taker-root "${agreement_root}/material/taker"
    --xmr-taker-public-packet "${agreement_root}/exchange/taker.json"
    --xmr-maker-public-packet "${agreement_root}/exchange/maker.json"
    --xmr-taker-role-journal "$m5_xmr_taker_role_journal"
    --xmr-taker-actor-root "$m5_xmr_taker_actor_root"
    --xmr-acceptance-receipt "$m5_xmr_taker_receipt"
  )
  if [[ "$use_delivery" == 1 ]]; then
    arguments=(--delivery-directory "$m5_xmr_delivery_root" "${arguments[@]}")
  fi
  "$m5_lez_taker_binary" "${arguments[@]}" >"$output"
  chmod 0600 "$output"
  require_owner_file "$output" "M5 XMR Taker acceptance output"
}

wait_m5_xmr_typed_blocked() {
  local monitor_tmp="${evidence_root}/.m5-xmr-monitor.tmp" members=""
  for _ in {1..1200}; do
    if "$m5_lez_maker_binary" --socket "$m5_xmr_maker_socket" monitor \
      --id "$m5_xmr_planned_swap_id" >"$monitor_tmp" 2>/dev/null &&
      jq -e --arg swap "$m5_xmr_planned_swap_id" '
        .schema_version==1 and .swap_id==$swap and .actor_kind=="monero"
        and .schedule_state=="queued" and .lease_generation==1 and .attempt_count==1
        and .progress.source_generation==1 and .progress.observation.state=="active"
        and .progress.observation.phase=="offered" and .progress.observation.revision==0
        and .progress.observation.next_action=="xmr_chain_effects_not_yet_composed"
        and (.progress.observed_at|type)=="number" and .manual_action==null
      ' "$monitor_tmp" >/dev/null 2>&1; then
      break
    fi
    process_is_owned "$m5_application_daemon_pid" "$m5_application_daemon_start_ticks" \
      "$m5_application_daemon_binary_sha256" || fail "M5 XMR supervisor daemon exited before Blocked"
    sleep 0.05
  done
  jq -e --arg swap "$m5_xmr_planned_swap_id" '
    .swap_id==$swap and .schedule_state=="queued" and .attempt_count==1
    and .progress.observation=={state:"active",phase:"offered",revision:0,
      next_action:"xmr_chain_effects_not_yet_composed"} and .manual_action==null
  ' "$monitor_tmp" >/dev/null || fail "M5 XMR supervisor did not commit the exact typed Blocked projection"
  members="$(m5_application_process_group_members "$m5_application_daemon_group")"
  [[ "$members" == "$m5_application_daemon_pid" ]] ||
    fail "M5 XMR supervisor retained an actor child after the bounded observation"
  mv "$monitor_tmp" "$m5_xmr_blocked_monitor"
  chmod 0600 "$m5_xmr_blocked_monitor"
  require_owner_file "$m5_xmr_blocked_monitor" "M5 XMR typed Blocked monitor"
}

complete_m5_xmr_application_handoff() {
  record_phase m5_xmr_application started
  readonly m5_xmr_maker_role_journal="${agreement_root}/stage-b/private/maker.sqlite"
  readonly m5_xmr_taker_role_journal="${agreement_root}/stage-b/private/taker.sqlite"
  readonly m5_xmr_journals_before="${evidence_root}/m5-xmr-journals-before.json"
  readonly m5_xmr_journals_after="${evidence_root}/m5-xmr-journals-after.json"
  write_m5_journal_snapshot "$m5_xmr_journals_before" \
    "$m5_xmr_maker_role_journal" "$m5_xmr_taker_role_journal"

  readonly m5_xmr_maker_actor_root="${m5_xmr_application_root}/maker-actor"
  readonly m5_xmr_maker_provision="${evidence_root}/m5-xmr-maker-provision.json"
  "$agreement_actor_binary" provision-application maker \
    --private-root "${agreement_root}/material/maker" \
    --own-public-packet "${agreement_root}/exchange/maker.json" \
    --peer-public-packet "${agreement_root}/exchange/taker.json" \
    --agreement-stage-a "$agreement_stage_a" --activation-stage-b "$agreement_stage_b" \
    --role-journal "$m5_xmr_maker_role_journal" --output-root "$m5_xmr_maker_actor_root" \
    >"$m5_xmr_maker_provision"
  chmod 0600 "$m5_xmr_maker_provision"
  require_owner_file "$m5_xmr_maker_provision" "M5 XMR Maker application provision"
  jq -e --arg swap "$m5_xmr_planned_swap_id" --arg stage_a "$(sha256_file "$agreement_stage_a")" \
    --arg stage_b "$(sha256_file "$agreement_stage_b")" '
      .schema_version==1 and .role=="maker" and .was_replay==false and .swap_id==$swap
      and .stage_a_sha256==$stage_a and .stage_b_sha256==$stage_b
      and .private_material_disclosed==false
    ' "$m5_xmr_maker_provision" >/dev/null ||
    fail "decoded Stage A did not retain the authenticated M5 XMR swap ID and terms"
  m5_xmr_decoded_stage_a_swap_id="$(jq -er .swap_id "$m5_xmr_maker_provision")"
  m5_xmr_actor_config="$(jq -er .config_path "$m5_xmr_maker_provision")"
  m5_xmr_actor_state="$(jq -er .state_database_path "$m5_xmr_maker_provision")"
  readonly m5_xmr_decoded_stage_a_swap_id m5_xmr_actor_config m5_xmr_actor_state
  require_owner_file "$m5_xmr_actor_config" "M5 XMR Maker actor manifest"
  require_owner_file "$m5_xmr_actor_state" "M5 XMR Maker actor state"

  readonly m5_xmr_maker_agreement_public_key="${m5_xmr_application_root}/maker-agreement.pub"
  readonly m5_xmr_maker_private_view_key="${m5_xmr_application_root}/maker-view.raw"
  readonly m5_xmr_actor_registry="${m5_xmr_application_root}/maker-registry.json"
  jq -er .agreement_public_key "${agreement_root}/exchange/maker.json" | xxd -r -p \
    >"$m5_xmr_maker_agreement_public_key"
  tr -d "\n" <"${agreement_root}/material/maker/monero-view.key" | xxd -r -p \
    >"$m5_xmr_maker_private_view_key"
  chmod 0600 "$m5_xmr_maker_agreement_public_key" "$m5_xmr_maker_private_view_key"
  require_owner_file "$m5_xmr_maker_agreement_public_key" "M5 XMR Maker public agreement key"
  require_owner_file "$m5_xmr_maker_private_view_key" "M5 XMR Maker private view key"
  [[ "$(stat -c %s "$m5_xmr_maker_agreement_public_key")" == 33 ]] ||
    fail "M5 XMR Maker public agreement key width drift"
  [[ "$(stat -c %s "$m5_xmr_maker_private_view_key")" == 32 ]] ||
    fail "M5 XMR Maker private view key width drift"
  jq -n --arg swap "$m5_xmr_planned_swap_id" --arg config "$m5_xmr_actor_config" \
    --arg config_sha "$(sha256_file "$m5_xmr_actor_config")" \
    --arg program "$m5_xmr_maker_actor_binary" \
    --arg program_sha "$(sha256_file "$m5_xmr_maker_actor_binary")" \
    --arg state "$m5_xmr_actor_state" '
      {schema_version:1,actors:[{swap_id:$swap,config_path:$config,
       config_sha256:$config_sha,program_path:$program,program_sha256:$program_sha,
       state_database_path:$state}]}
    ' >"$m5_xmr_actor_registry"
  chmod 0600 "$m5_xmr_actor_registry"
  require_owner_file "$m5_xmr_actor_registry" "M5 XMR strict actor registry"

  readonly m5_xmr_taker_actor_root="${m5_xmr_application_root}/taker-actor"
  readonly m5_xmr_taker_receipt="${m5_xmr_application_root}/taker-receipt.json"
  readonly m5_xmr_initial_acceptance="${evidence_root}/m5-xmr-initial-acceptance.json"
  readonly m5_xmr_replay_acceptance="${evidence_root}/m5-xmr-replay-acceptance.json"
  readonly m5_xmr_blocked_monitor="${evidence_root}/m5-xmr-blocked-monitor.json"
  start_m5_xmr_application_daemon authority 1
  run_m5_xmr_taker_acceptance "$m5_xmr_initial_acceptance" 1
  jq -e --arg swap "$m5_xmr_planned_swap_id" '
    .schema_version==1 and .offer_revision==3 and .swap_id==$swap
    and .replay=={stage_a:false,activation:false} and .private_material_disclosed==false
    and .actor.role=="taker" and .actor.provisioning_replay==false
    and .actor.receipt_replay==false
  ' "$m5_xmr_initial_acceptance" >/dev/null || fail "M5 XMR fresh application acceptance drift"
  jq -e --arg swap "$m5_xmr_planned_swap_id" --arg offer "$m5_xmr_offer_id" \
    --arg reservation "$m5_xmr_reservation_id" '
      .schema_version==1 and .pair=="monero" and .role=="taker"
      and .swap_id==$swap and .offer_id==$offer and .reservation_id==$reservation
    ' "$m5_xmr_taker_receipt" >/dev/null || fail "M5 XMR Taker receipt drift"
  wait_m5_xmr_typed_blocked

  readonly m5_xmr_artifacts_before="${evidence_root}/m5-xmr-artifacts-before.tsv"
  readonly m5_xmr_artifacts_after="${evidence_root}/m5-xmr-artifacts-after.tsv"
  write_m5_application_artifact_snapshot "$m5_xmr_artifacts_before"
  stop_m5_xmr_application_daemon || fail "M5 XMR authority daemon did not stop before replay"
  [[ ! -e "$m5_xmr_removed_delivery_root" && ! -L "$m5_xmr_removed_delivery_root" ]] ||
    fail "M5 XMR removed Delivery destination exists"
  mv "$m5_xmr_delivery_root" "$m5_xmr_removed_delivery_root"
  require_owner_file "${m5_xmr_removed_delivery_root}/${m5_xmr_offer_id}.offer.json" "removed original M5 XMR Delivery offer"
  mkdir -m 0700 "$m5_xmr_delivery_root"
  [[ -z "$(find "$m5_xmr_delivery_root" -mindepth 1 -print -quit)" ]] ||
    fail "replacement M5 XMR Delivery root was not created empty"

  start_m5_xmr_application_daemon replay 1
  require_owner_file "${m5_xmr_delivery_root}/${m5_xmr_offer_id}.offer.json" \
    "reconciled M5 XMR Delivery offer"
  readonly m5_xmr_reconciled_delivery_plan="${evidence_root}/m5-xmr-reconciled-delivery-plan.json"
  "$m5_lez_taker_binary" --delivery-directory "$m5_xmr_delivery_root" \
    --maker-public-key "$m5_xmr_delivery_public_key" --now-unix-seconds "$(date -u +%s)" \
    --pair monero --direction taker-sells-lez --plan-xmr-offer "$m5_xmr_offer_id" \
    --reservation-id "$m5_xmr_reservation_id" --foreign-units "$m5_xmr_foreign_units" \
    >"$m5_xmr_reconciled_delivery_plan"
  chmod 0600 "$m5_xmr_reconciled_delivery_plan"
  require_owner_file "$m5_xmr_reconciled_delivery_plan" \
    "authenticated reconciled M5 XMR Delivery plan"
  jq -e --arg offer "$m5_xmr_offer_id" --arg reservation "$m5_xmr_reservation_id" \
    --arg swap "$m5_xmr_planned_swap_id" --argjson foreign "$m5_xmr_foreign_units" \
    --argjson lez "$m5_xmr_lez_units" '
      .schema_version==1 and .offer_id==$offer and .reservation_id==$reservation
      and .swap_id==$swap and .foreign_units==$foreign and .lez_units==$lez
      and (.signed_envelope_sha256|test("^[0-9a-f]{64}$"))
      and .private_material_disclosed==false
    ' "$m5_xmr_reconciled_delivery_plan" >/dev/null ||
    fail "reconciled M5 XMR Delivery offer changed authenticated terms"
  [[ ! -e "$m5_xmr_reconciled_delivery_root" && ! -L "$m5_xmr_reconciled_delivery_root" ]] ||
    fail "M5 XMR reconciled Delivery archive exists"
  mv "$m5_xmr_delivery_root" "$m5_xmr_reconciled_delivery_root"
  mkdir -m 0700 "$m5_xmr_delivery_root"
  [[ -z "$(find "$m5_xmr_delivery_root" -mindepth 1 -print -quit)" ]] ||
    fail "M5 XMR Delivery outage root was not created empty"
  run_m5_xmr_taker_acceptance "$m5_xmr_replay_acceptance" 0
  m5_delivery_offer_files_absent "$m5_xmr_delivery_root" ||
    fail "Delivery-free M5 XMR replay observed or created an offer file"
  jq -e --arg swap "$m5_xmr_planned_swap_id" '
    .schema_version==1 and .offer_revision==3 and .swap_id==$swap
    and .replay=={stage_a:true,activation:true} and .private_material_disclosed==false
    and .actor.role=="taker" and .actor.provisioning_replay==true
    and .actor.receipt_replay==true
  ' "$m5_xmr_replay_acceptance" >/dev/null || fail "M5 XMR Delivery-free exact replay drift"
  write_m5_application_artifact_snapshot "$m5_xmr_artifacts_after"
  cmp -- "$m5_xmr_artifacts_before" "$m5_xmr_artifacts_after" ||
    fail "M5 XMR receipt, actor, or journal bytes/inodes changed through replay"
  write_m5_journal_snapshot "$m5_xmr_journals_after" \
    "$m5_xmr_maker_role_journal" "$m5_xmr_taker_role_journal"
  cmp -- "$m5_xmr_journals_before" "$m5_xmr_journals_after" ||
    fail "M5 XMR role journal device/inode/size/hash changed through application replay"
  "$m5_lez_maker_binary" --socket "$m5_xmr_maker_socket" monitor \
    --id "$m5_xmr_planned_swap_id" >"${evidence_root}/m5-xmr-replay-monitor.json"
  jq -e --arg swap "$m5_xmr_planned_swap_id" '
    .swap_id==$swap and .actor_kind=="monero" and .schedule_state=="queued"
    and .attempt_count==1 and .progress.observation.state=="active"
    and .progress.observation.phase=="offered" and .progress.observation.revision==0
    and .progress.observation.next_action=="xmr_chain_effects_not_yet_composed"
    and .manual_action==null
  ' "${evidence_root}/m5-xmr-replay-monitor.json" >/dev/null ||
    fail "M5 XMR replay lost the typed Blocked owner projection"
  stop_m5_xmr_application_daemon || fail "M5 XMR replay daemon did not stop before legacy Tag 13"

  readonly m5_xmr_cutoff_evidence="${evidence_root}/m5-xmr-application-cutoff.json"
  jq -n --arg swap "$m5_xmr_planned_swap_id" --arg decoded "$m5_xmr_decoded_stage_a_swap_id" \
    --arg receipt_swap "$(jq -er .swap_id "$agreement_receipt")" \
    --arg initial_swap "$(jq -er .swap_id "$m5_xmr_initial_acceptance")" \
    --arg replay_swap "$(jq -er .swap_id "$m5_xmr_replay_acceptance")" \
    --arg reconciled_swap "$(jq -er .swap_id "$m5_xmr_reconciled_delivery_plan")" \
    --arg reconciled_envelope "$(jq -er .signed_envelope_sha256 "$m5_xmr_reconciled_delivery_plan")" \
    --argjson daemon_pid "$m5_last_stopped_daemon_pid" \
    --argjson process_group "$m5_last_stopped_daemon_group" '
      {schema_version:1,kind:"m5_xmr_application_pre_tag13_cutoff",result:"passed",
       plan_swap_id:$swap,agreement_receipt_swap_id:$receipt_swap,
       decoded_stage_a_swap_id:$decoded,initial_acceptance_swap_id:$initial_swap,
       replay_acceptance_swap_id:$replay_swap,
       exact_terms:{monero_amount_piconero:1000000000000,lez_amount:700},
       supervisor:{resolution:"blocked",schedule_state:"queued",attempt_count:1,
         child_process:null,manual_action:null,chain_effect_executed:false,
         phase:"offered",revision:0,next_action:"xmr_chain_effects_not_yet_composed",
         minimum_reobservation_seconds:60,configured_reobservation_seconds:3600},
       replay:{original_delivery_root_removed:true,
         publisher_reconciled_consumed_offer_before_outage:true,
         reconciled_offer_authenticated:true,reconciled_offer_swap_id:$reconciled_swap,
         reconciled_offer_envelope_sha256:$reconciled_envelope,reconciled_delivery_root_archived:true,
         replacement_offer_files_present:false,taker_delivery_argument_present:false,artifact_bytes_and_inodes_unchanged:true,
         journal_device_inode_size_hash_unchanged:true,sqlite_sidecars_present:false},
       cutoff:{daemon_pid:$daemon_pid,process_group:$process_group,pid_absent:true,
         process_group_absent:true,owner_socket_absent:true,chat_socket_absent:true,
         ready_files_absent:true},legacy_tag13_started:false}
    ' >"$m5_xmr_cutoff_evidence"
  chmod 0600 "$m5_xmr_cutoff_evidence"
  require_owner_file "$m5_xmr_cutoff_evidence" "M5 XMR pre-Tag13 cutoff evidence"
  record_phase m5_xmr_application completed
}

verify_m5_xmr_application_cutoff() {
  [[ -z "$m5_application_daemon_pid" ]] || fail "M5 XMR daemon remains tracked at Tag 13 cutoff"
  [[ ! -e "/proc/${m5_last_stopped_daemon_pid}" ]] || fail "M5 XMR daemon PID exists at Tag 13 cutoff"
  m5_application_process_group_has_members "$m5_last_stopped_daemon_group" &&
    fail "M5 XMR daemon process group exists at Tag 13 cutoff"
  [[ ! -e "$m5_xmr_maker_socket" && ! -L "$m5_xmr_maker_socket" ]] ||
    fail "M5 XMR owner socket exists at Tag 13 cutoff"
  [[ ! -e "$m5_xmr_chat_socket" && ! -L "$m5_xmr_chat_socket" ]] ||
    fail "M5 XMR Chat socket exists at Tag 13 cutoff"
  [[ -z "$(find "$m5_xmr_runtime_root" -mindepth 1 -maxdepth 1 -name "ready-*" -print -quit)" ]] ||
    fail "M5 XMR readiness evidence survived daemon cutoff"
  require_no_sqlite_sidecars "$m5_xmr_maker_role_journal" "M5 XMR Maker role journal at cutoff"
  require_no_sqlite_sidecars "$m5_xmr_taker_role_journal" "M5 XMR Taker role journal at cutoff"
  require_owner_file "${m5_xmr_reconciled_delivery_root}/${m5_xmr_offer_id}.offer.json" \
    "archived authenticated M5 XMR reconciled Delivery offer"
  m5_delivery_offer_files_absent "$m5_xmr_delivery_root" ||
    fail "M5 XMR replacement Delivery root gained an offer before Tag 13"
  jq -e --arg swap "$m5_xmr_planned_swap_id" '
    .result=="passed" and .plan_swap_id==$swap and .agreement_receipt_swap_id==$swap
    and .decoded_stage_a_swap_id==$swap and .initial_acceptance_swap_id==$swap
    and .replay_acceptance_swap_id==$swap and .supervisor.resolution=="blocked"
    and .supervisor.chain_effect_executed==false and .supervisor.child_process==null
    and .supervisor.minimum_reobservation_seconds==60
    and .supervisor.configured_reobservation_seconds==3600
    and .replay.original_delivery_root_removed==true
    and .replay.publisher_reconciled_consumed_offer_before_outage==true
    and .replay.reconciled_offer_authenticated==true
    and .replay.reconciled_offer_swap_id==$swap
    and (.replay.reconciled_offer_envelope_sha256|test("^[0-9a-f]{64}$"))
    and .replay.reconciled_delivery_root_archived==true
    and .replay.replacement_offer_files_present==false
    and .replay.taker_delivery_argument_present==false
    and .cutoff.pid_absent==true and .cutoff.process_group_absent==true
    and .cutoff.owner_socket_absent==true and .cutoff.chat_socket_absent==true
    and .cutoff.ready_files_absent==true and .legacy_tag13_started==false
  ' "$m5_xmr_cutoff_evidence" >/dev/null || fail "M5 XMR pre-Tag13 cutoff evidence drift"
}

submit_tag13() {
  record_phase tag13 started
  readonly tag13_state="${private_root}/tag13-state"
  mkdir -m 0700 "$tag13_state"
  readonly tag13_stdout="${evidence_root}/tag13-stdout.json"
  readonly tag13_internal="${tag13_state}/m4-xmr-stage-a-tag13-evidence.v2.json"
  readonly tag13_no_retry_latch="${manifest_root}/tag13-no-retry.latch"
  readonly tag13_prepare_request_id="${run_id}-tag13-prepare-001"
  [[ ! -e "$tag13_stdout" && ! -L "$tag13_stdout" ]] || fail "tag13 stdout evidence exists"
  [[ ! -e "$tag13_internal" && ! -L "$tag13_internal" ]] || fail "tag13 internal evidence exists"
  [[ ! -e "$tag13_no_retry_latch" && ! -L "$tag13_no_retry_latch" ]] ||
    fail "tag13 no-retry latch already exists; do not retry this run"

  local temporary
  temporary="$(mktemp "${manifest_root}/.tag13-no-retry.XXXXXX")"
  printf '%s\n' 'tag13_submission_may_have_occurred' >"$temporary"
  chmod 0600 "$temporary"
  sync -f "$temporary"
  ln -- "$temporary" "$tag13_no_retry_latch"
  unlink "$temporary"
  sync -f "$tag13_no_retry_latch"
  sync -d "$manifest_root"
  require_owner_file "$tag13_no_retry_latch" "tag13 durable no-retry latch"
  [[ "$(sed -n '1p' "$tag13_no_retry_latch")" == tag13_submission_may_have_occurred ]] ||
    fail "tag13 no-retry latch marker drift"

  "$tag13_binary" --state-directory "$tag13_state" \
    --private-key-file "${private_root}/lez-identities/taker/lez-signer.key" \
    --sequencer-url "$(manifest_value LEZ_SEQUENCER_RPC_URL "$lez_stack_manifest")" \
    --indexer-url "$(manifest_value LEZ_INDEXER_RPC_URL "$lez_stack_manifest")" \
    --agreement-wire-file "$agreement_stage_a" --activation-wire-file "$agreement_stage_b" \
    --monero-view-key-file "${agreement_root}/material/taker/monero-view.key" \
    --run-id "$run_id" --prepare-request-id "$tag13_prepare_request_id" >"$tag13_stdout"
  chmod 0600 "$tag13_stdout"
  require_owner_file "$tag13_stdout" "tag13 stdout evidence"
  require_owner_file "$tag13_internal" "tag13 internal evidence"
  [[ "$(jq -S -c . "$tag13_stdout")" == "$(jq -S -c . "$tag13_internal")" ]] ||
    fail "tag13 stdout and durable evidence differ semantically"
  jq -e --arg run_id "$run_id" --arg request "$tag13_prepare_request_id" \
    --arg stage_a "$(sha256_file "$agreement_stage_a")" \
    --arg stage_b "$(sha256_file "$agreement_stage_b")" \
    --arg lez_amount "$agreement_lez_amount" \
    --argjson cutoff "$maker_xmr_funding_cutoff_ms" '
      .schema=="lez_v02_m4_xmr_stage_a_tag13_poc_v2" and .role=="taker"
      and .run_id==$run_id and .prepare_request_id==$request
      and .stage_a_agreement_wire_sha256==$stage_a
      and .stage_b_activation_wire_sha256==$stage_b
      and .terms.amount==$lez_amount and .maker_xmr_funding_cutoff_ms==$cutoff
      and .initialization.effect=="initialize" and .funding.effect=="fund"
      and .initialization.finalized_clock.height < .funding.finalized_clock.height
      and .initialization.finalized_clock.timestamp_ms <= $cutoff
      and .funding.finalized_clock.timestamp_ms <= $cutoff
      and .public_rpc_used==false and .automatic_submission_retry==false
      and .send_attempt_ceiling_per_effect_per_process==1
      and .finality_polling_is_submission_retry==false
      and .crash_atomic_submission==false
      and .monero_lock_observed==false and .swap_completed==false
      and .atomic_swap_proven==false
      and .atomicity_claim=="none_tag13_only_proves_ordered_finalized_lez_escrow_funding"
    ' "$tag13_internal" >/dev/null || fail "tag13 evidence violates the exact v2 boundary"
  record_phase tag13 completed
}

export_tag13_handoff() {
  record_phase tag13_handoff started
  readonly tag13_handoff_root="${private_root}/tag13-handoff"
  mkdir -m 0700 "$tag13_handoff_root"
  "$tag13_export_binary" --state-directory "$tag13_state" --output-directory "$tag13_handoff_root" \
    --run-id "$run_id" --stage-a-agreement-wire-sha256 "$(sha256_file "$agreement_stage_a")" \
    --stage-b-activation-wire-sha256 "$(sha256_file "$agreement_stage_b")" \
    --authenticated-transfer-program-id "dcbbfebcd59399961ed9973b8307dc475fd4c5ca5779aacfe7588f7dbc3f4a71"
  for artifact in taker-runtime.json maker-runtime.json terms.json tag13-handoff-receipt.json; do
    require_owner_file "$tag13_handoff_root/$artifact" "Tag13 handoff $artifact"
  done
  record_resource ephemeral_path "$tag13_handoff_root" "$tag13_handoff_root"
  record_phase tag13_handoff completed
}

start_role_sidecars() {
  record_phase sidecars started
  readonly sidecar_parent="${private_root}/role-sidecars"
  readonly taker_sidecar_root="${sidecar_parent}/taker"
  readonly maker_sidecar_root="${sidecar_parent}/maker"
  mkdir -m 0700 "$sidecar_parent"
  record_resource ephemeral_path "$sidecar_parent" "$sidecar_parent"
  local sequencer indexer
  sequencer="$(manifest_value LEZ_SEQUENCER_RPC_URL "$lez_stack_manifest")"
  indexer="$(manifest_value LEZ_INDEXER_RPC_URL "$lez_stack_manifest")"
  "$repo_root/scripts/run-m4-lez-sidecar.sh" start --root "$taker_sidecar_root" --role taker --run-id "$run_id" \
    --sidecar-bin "$bridge_binary" --sequencer-url "$sequencer" --indexer-url "$indexer" \
    --runtime-file "$tag13_handoff_root/taker-runtime.json" --terms-file "$tag13_handoff_root/terms.json" \
    --private-key-file "${private_root}/lez-identities/taker/lez-signer.key" \
    --authenticated-transfer-program-id "dcbbfebcd59399961ed9973b8307dc475fd4c5ca5779aacfe7588f7dbc3f4a71" \
    --adopt-state-directory "$tag13_state" --tag13-handoff-receipt "$tag13_handoff_root/tag13-handoff-receipt.json" >/dev/null
  "$repo_root/scripts/run-m4-lez-sidecar.sh" start --root "$maker_sidecar_root" --role maker --run-id "$run_id" \
    --sidecar-bin "$bridge_binary" --sequencer-url "$sequencer" --indexer-url "$indexer" \
    --runtime-file "$tag13_handoff_root/maker-runtime.json" --terms-file "$tag13_handoff_root/terms.json" \
    --private-key-file "${private_root}/lez-identities/maker/lez-signer.key" \
    --authenticated-transfer-program-id "dcbbfebcd59399961ed9973b8307dc475fd4c5ca5779aacfe7588f7dbc3f4a71" >/dev/null
  local sidecar_root manifest pid start binary_sha
  for sidecar_root in "$taker_sidecar_root" "$maker_sidecar_root"; do
    manifest="$sidecar_root/pid-manifest.json"
    require_owner_file "$manifest" "sidecar PID manifest"
    pid="$(jq -er .pid "$manifest")"
    start="$(jq -er .start_ticks "$manifest")"
    binary_sha="$(jq -er .binary_sha256 "$manifest")"
    record_resource process "$pid" "$sidecar_root" "$start" "$binary_sha"
  done
  record_phase sidecars completed
}
fund_and_verify_monero() {
  record_phase monero_funding started
  readonly monero_funding_evidence="${evidence_root}/monero-funding.json"
  readonly monero_verification_evidence="${evidence_root}/monero-verification.json"
  [[ ! -e "$monero_funding_evidence" && ! -L "$monero_funding_evidence" ]] || fail "Monero funding evidence exists"
  [[ ! -e "$monero_verification_evidence" && ! -L "$monero_verification_evidence" ]] || fail "Monero verification evidence exists"
  "$monero_fund_binary" --agreement-wire-file "$agreement_stage_a" --monero-view-key-file "${agreement_root}/material/taker/monero-view.key" --daemon-url "${monero_env[MONERO_DAEMON_ENDPOINT]}" --daemon-username-file "${monero_env[MONERO_DAEMON_USERNAME_FILE]}" --daemon-password-file "${monero_env[MONERO_DAEMON_PASSWORD_FILE]}" --funding-wallet-url "${monero_env[MONERO_MAKER_WALLET_ENDPOINT]}" --funding-wallet-username-file "${monero_env[MONERO_MAKER_RPC_USERNAME_FILE]}" --funding-wallet-password-file "${monero_env[MONERO_MAKER_RPC_PASSWORD_FILE]}" --shared-wallet-url "${monero_env[MONERO_FUNDING_WALLET_ENDPOINT]}" --shared-wallet-username-file "${monero_env[MONERO_FUNDING_RPC_USERNAME_FILE]}" --shared-wallet-password-file "${monero_env[MONERO_FUNDING_RPC_PASSWORD_FILE]}" --shared-wallet-file-password-file "${monero_env[MONERO_FUNDING_WALLET_PASSWORD_FILE]}" --shared-wallet-filename "m4-${MONERO_RUN_ID}-shared" --output-evidence "$monero_funding_evidence" >/dev/null
  require_owner_file "$monero_funding_evidence" "Monero funding evidence"
  local tx_id
  tx_id="$(jq -er 'if (.schema=="lez_v02_m4_actual_local_monero_funding_v2" and .attempt_state=="confirmed" and .public_rpc_used==false and .faucet_used==false and .automatic_submission_retry==false) then .transaction_id else error end' "$monero_funding_evidence")" || fail "Monero funding evidence violates the local one-shot boundary"
  [[ "$tx_id" =~ ^[0-9a-f]{64}$ ]] || fail "Monero funding transaction ID is invalid"
  record_phase monero_funding completed
  record_phase monero_verification started
  "$monero_verify_binary" --agreement-wire-file "$agreement_stage_a" --monero-transaction-id "$tx_id" --run-id "$MONERO_RUN_ID" --daemon-url "${monero_env[MONERO_DAEMON_ENDPOINT]}" --daemon-username-file "${monero_env[MONERO_DAEMON_USERNAME_FILE]}" --daemon-password-file "${monero_env[MONERO_DAEMON_PASSWORD_FILE]}" --target-wallet-url "${monero_env[MONERO_FUNDING_WALLET_ENDPOINT]}" --target-wallet-username-file "${monero_env[MONERO_FUNDING_RPC_USERNAME_FILE]}" --target-wallet-password-file "${monero_env[MONERO_FUNDING_RPC_PASSWORD_FILE]}" --foreign-wallet-url "${monero_env[MONERO_TAKER_WALLET_ENDPOINT]}" --foreign-wallet-username-file "${monero_env[MONERO_TAKER_RPC_USERNAME_FILE]}" --foreign-wallet-password-file "${monero_env[MONERO_TAKER_RPC_PASSWORD_FILE]}" --output-evidence "$monero_verification_evidence" >/dev/null
  require_owner_file "$monero_verification_evidence" "Monero verification evidence"
  local required_confirmations
  required_confirmations=$(jq -er '.required_confirmations | select(type == "number" and . >= 1)' "$monero_funding_evidence") || fail "Monero funding evidence lacks required confirmations"
  jq -e --arg run_id "$MONERO_RUN_ID" --arg tx "$tx_id" --argjson required "$required_confirmations" '.schema=="lez_v02_m4_actual_local_monero_verification_v2" and .run_id==$run_id and .transaction_id==$tx and .confirmations >= $required and .public_rpc_used==false and .faucet_used==false and .network_scope=="isolated_official_monero_regtest"' "$monero_verification_evidence" >/dev/null || fail "Monero verification evidence is incomplete"
  record_phase monero_verification completed
}

drive_m5_xmr_local_finality_clock() {
  [[ "$m5_xmr_application_mode" == 1 && "$m5_xmr_journey" == refund ]] ||
    fail "local finalized-clock driving is restricted to the M5 refund journey"
  local tick_number="$1" taker_endpoint maker_owner taker_owner tick_evidence
  [[ "$tick_number" == 1 ]] ||
    fail "local finalized-clock tick exceeded its fixed bound"
  taker_endpoint="$(jq -er '.endpoint' "$taker_sidecar_root/pid-manifest.json")"
  maker_owner="$(jq -er '.account_id_hex' "${evidence_root}/maker-lez-identity.json")"
  taker_owner="$(jq -er '.account_id_hex' "${evidence_root}/taker-lez-identity.json")"
  tick_evidence="${tag16_refund_clock_tick_root}/tick-${tick_number}.json"
  [[ ! -e "$tick_evidence" && ! -L "$tick_evidence" ]] ||
    fail "local finalized-clock tick evidence already exists"
  "$local_clock_driver_binary" \
    --sidecar-endpoint "$taker_endpoint" \
    --capability-file "$taker_sidecar_root/capability" \
    --runtime-file "$tag13_handoff_root/taker-runtime.json" \
    --terms-file "$tag13_handoff_root/terms.json" \
    --run-id "$run_id" \
    --recipient-account-id "$maker_owner" \
    --exclusive-punish-at-ms "$punish_at_ms" \
    --finality-timeout-seconds "$m5_xmr_refund_clock_progress_timeout_seconds" \
    --output-evidence "$tick_evidence"
  require_owner_file "$tick_evidence" "local finalized-clock tick evidence"
  jq -e --arg run "$run_id" --arg sender "$taker_owner" --arg recipient "$maker_owner" \
    --argjson refund "$refund_at_ms" --argjson punish "$punish_at_ms" '
    .schema=="lez_v02_m5_local_clock_driver_v1"
    and .context.run_id==$run
    and .runtime.sidecar_role=="taker"
    and .recipient_account_id==$recipient
    and .exclusive_punish_at_ms==$punish
    and (.transaction_id|test("^[0-9a-f]{64}$"))
    and .submission_request_id==.transaction_id
    and .submission_outcome != null
    and .node_submission_attempts==1
    and .transfer_amount==1
    and .sender_before.account_id==$sender and .sender_after.account_id==$sender
    and .recipient_before.account_id==$recipient and .recipient_after.account_id==$recipient
    and .sender_before.program_owner==.terms.authenticated_transfer_program_id
    and .sender_after.program_owner==.terms.authenticated_transfer_program_id
    and .recipient_before.program_owner==.terms.authenticated_transfer_program_id
    and .recipient_after.program_owner==.terms.authenticated_transfer_program_id
    and .clock_before.timestamp_ms < $punish
    and .clock_after.timestamp_ms < $punish
    and .finalized_clock_before.height < .finalized_clock_after.height
    and .finalized_clock_after.height >= .clock_after.height
    and .finalized_clock_after.timestamp_ms >= $refund
    and .finalized_clock_after.timestamp_ms < $punish
    and .finalized_observation_attempts_before >= 1
    and .finalized_observation_attempts_after >= 1
    and .finality_wait_timeout_seconds==60
    and .finality_source=="authenticated_genesis_bound_official_indexer"
    and .sender_after.balance == (.sender_before.balance - 1)
    and .sender_after.nonce == (.sender_before.nonce + 1)
    and .recipient_after.balance == (.recipient_before.balance + 1)
    and .recipient_after.nonce == .recipient_before.nonce
    and .metadata_account_sha256_after==.metadata_account_sha256_before
    and .custody_account_sha256_after==.custody_account_sha256_before
    and .escrow_accounts_byte_identical==true
    and .accounting_verified==true
    and .local_only==true
    and .retry_policy=="one_node_submission_attempt_no_retry_poll_only"
  ' "$tick_evidence" >/dev/null ||
    fail "local finalized-clock tick violated its one-shot accounting boundary"
  jq -er '.finalized_clock_after.height' "$tick_evidence"
}

wait_for_m5_xmr_refund_window() {
  record_phase tag16_refund_window started
  readonly tag16_refund_window_result="${evidence_root}/tag16-refund-window.json"
  readonly tag16_refund_clock_tick_root="${evidence_root}/tag16-refund-clock-ticks"
  mkdir -m 0700 "$tag16_refund_clock_tick_root"
  local maker_endpoint probe_height attempt result_tmp finalized_timestamp_ms finalized_height
  local finalized_hash finalized_identity="" previous_finalized_identity=""
  local identical_clock_samples=0 clock_tick_count=0 host_timestamp_ms
  maker_endpoint="$(jq -er '.endpoint' "$maker_sidecar_root/pid-manifest.json")"
  probe_height="$(jq -er '.funding.containing_block_id' "$tag13_internal")"
  result_tmp="${tag16_refund_window_result}.attempt"
  for attempt in {1..18000}; do
    rm -f -- "$result_tmp"
    "$classifier_binary" --sidecar-endpoint "$maker_endpoint" --capability-file "$maker_sidecar_root/capability" --runtime-file "$tag13_handoff_root/maker-runtime.json" --terms-file "$tag13_handoff_root/terms.json" --run-id "$run_id" --request-id "${run_id}-tag16-clock-${attempt}" --role maker --effect refund --start-height "$probe_height" --max-blocks 1 --output-result "$result_tmp" >/dev/null 2>&1 || true
    if jq -e '(.outcome.status=="absent" or .outcome.status=="uncertain")
      and (.outcome.finalized_clock.block_hash|test("^[0-9a-f]{64}$"))
      and (.outcome.finalized_clock.height|type)=="number"
      and (.outcome.finalized_clock.timestamp_ms|type)=="number"' "$result_tmp" >/dev/null 2>&1; then
      require_owner_file "$result_tmp" "Tag16 finalized-clock observation"
      finalized_timestamp_ms="$(jq -er '.outcome.finalized_clock.timestamp_ms' "$result_tmp")"
      finalized_height="$(jq -er '.outcome.finalized_clock.height' "$result_tmp")"
      finalized_hash="$(jq -er '.outcome.finalized_clock.block_hash' "$result_tmp")"
      finalized_identity="${finalized_height}:${finalized_hash}:${finalized_timestamp_ms}"
      if [[ "$finalized_identity" == "$previous_finalized_identity" ]]; then
        ((identical_clock_samples += 1))
      else
        previous_finalized_identity="$finalized_identity"
        identical_clock_samples=1
      fi
      (( finalized_timestamp_ms < punish_at_ms )) || fail "finalized LEZ clock reached punish_at before Tag16 submission"
      if (( finalized_timestamp_ms >= refund_at_ms && finalized_timestamp_ms < punish_at_ms )); then
        tag16_scan_start_height="$((finalized_height + 1))"
        mv "$result_tmp" "$tag16_refund_window_result"
        break
      fi
      host_timestamp_ms="$(date -u +%s%3N)"
      if (( host_timestamp_ms >= refund_at_ms &&
            identical_clock_samples >= m5_xmr_refund_clock_stall_samples )); then
        (( host_timestamp_ms < punish_at_ms - m5_xmr_refund_clock_punish_guard_ms )) ||
          fail "local finalized-clock drive reached its punish_at safety guard"
        (( clock_tick_count < m5_xmr_refund_clock_max_ticks )) ||
          fail "local finalized-clock drive exhausted its fixed tick bound"
        ((clock_tick_count += 1))
        probe_height="$(drive_m5_xmr_local_finality_clock "$clock_tick_count")"
        [[ "$probe_height" =~ ^[1-9][0-9]*$ ]] ||
          fail "local clock driver returned an invalid finalized height"
        previous_finalized_identity=""
        identical_clock_samples=0
      fi
    fi
    sleep .25
  done
  require_owner_file "$tag16_refund_window_result" "Tag16 refund-window evidence"
  jq -e --argjson refund_at "$refund_at_ms" --argjson punish_at "$punish_at_ms" '(.outcome.status=="absent" or .outcome.status=="uncertain") and .outcome.finalized_clock.timestamp_ms >= $refund_at and .outcome.finalized_clock.timestamp_ms < $punish_at' "$tag16_refund_window_result" >/dev/null || fail "Tag16 refund-window evidence is outside the signed interval"
  [[ "$tag16_scan_start_height" =~ ^[1-9][0-9]*$ ]] || fail "Tag16 scan start height is invalid"
  record_phase tag16_refund_window completed
}

prepare_tag16_refund_signature() {
  record_phase tag16_prepare started
  readonly tag16_refund_root="${private_root}/tag16-refund"
  readonly taker_refund_adaptor_scalar="${tag16_refund_root}/taker-refund-adaptor.key"
  readonly taker_refund_final_signature="${tag16_refund_root}/taker-final-signature.json"
  mkdir -m 0700 "$tag16_refund_root"
  record_resource ephemeral_path "$tag16_refund_root" "$tag16_refund_root"
  require_owner_file "${agreement_root}/material/taker/xmr-share.key" "Taker Monero spend-key share"
  require_owner_file "${agreement_root}/stage-b/exchange/refund/taker-presignature.json" "Taker refund presignature"
  [[ "$(wc -c <"${agreement_root}/material/taker/xmr-share.key")" == 65 ]] &&
    rg -q '^[0-9a-f]{64}$' "${agreement_root}/material/taker/xmr-share.key" ||
    fail "Taker Monero spend-key share is not canonical little-endian hex"
  xxd -r -p "${agreement_root}/material/taker/xmr-share.key" | xxd -p -c1 | tac | tr -d '\n' >"$taker_refund_adaptor_scalar"
  printf '\n' >>"$taker_refund_adaptor_scalar"
  chmod 600 "$taker_refund_adaptor_scalar"
  sync "$taker_refund_adaptor_scalar"
  require_owner_file "$taker_refund_adaptor_scalar" "temporary Taker refund adaptor scalar"
  [[ "$(wc -c <"$taker_refund_adaptor_scalar")" == 65 ]] &&
    rg -q '^[0-9a-f]{64}$' "$taker_refund_adaptor_scalar" ||
    fail "temporary Taker refund adaptor scalar is invalid"
  if ! "$agreement_role_runner_binary" taker --journal "${agreement_root}/stage-b/private/taker.sqlite" --session "${agreement_root}/material/taker-sessions/refund.json" adapt-presignature --input "${agreement_root}/stage-b/exchange/refund/taker-presignature.json" --adaptor-secret-file "$taker_refund_adaptor_scalar" --output "$taker_refund_final_signature"; then
    unlink -- "$taker_refund_adaptor_scalar"
    fail "Taker refund presignature adaptation failed"
  fi
  unlink -- "$taker_refund_adaptor_scalar"
  require_owner_file "$taker_refund_final_signature" "Taker refund final-signature packet"
  record_phase tag16_prepare completed
}

publish_tag16_refund() {
  record_phase tag16 started
  readonly tag16_submission="${evidence_root}/tag16-refund-submission.json"
  local taker_endpoint
  taker_endpoint="$(jq -er '.endpoint' "$taker_sidecar_root/pid-manifest.json")"
  "$tag16_binary" --sidecar-endpoint "$taker_endpoint" --capability-file "$taker_sidecar_root/capability" --runtime-file "$tag13_handoff_root/taker-runtime.json" --agreement-wire-file "$agreement_stage_a" --activation-wire-file "$agreement_stage_b" --monero-view-key-file "${agreement_root}/material/taker/monero-view.key" --final-signature-file "$taker_refund_final_signature" --run-id "$run_id" --prepare-request-id "${run_id}-tag16-prepare-001" --complete-request-id "${run_id}-tag16-complete-001" --output-evidence "$tag16_submission"
  require_owner_file "$tag16_submission" "Tag16 submission evidence"
  jq -e '.schema=="lez_v02_m5_actual_local_tag16_v1" and .role=="taker" and .submission_outcome != null and .automatic_submission_retry==false and .public_rpc_used==false' "$tag16_submission" >/dev/null || fail "Tag16 submission evidence is incomplete"
  record_phase tag16 completed
}

classify_tag16_refund_finality() {
  record_phase tag16_finality started
  readonly tag16_refund_finality_result="${evidence_root}/tag16-refund-finalized.json"
  local maker_endpoint attempt result_tmp finalized_timestamp_ms
  maker_endpoint="$(jq -er '.endpoint' "$maker_sidecar_root/pid-manifest.json")"
  result_tmp="${tag16_refund_finality_result}.attempt"
  for attempt in {1..600}; do
    rm -f -- "$result_tmp"
    "$classifier_binary" --sidecar-endpoint "$maker_endpoint" --capability-file "$maker_sidecar_root/capability" --runtime-file "$tag13_handoff_root/maker-runtime.json" --terms-file "$tag13_handoff_root/terms.json" --run-id "$run_id" --request-id "${run_id}-tag16-finality-${attempt}" --role maker --effect refund --start-height "$tag16_scan_start_height" --max-blocks 16 --output-result "$result_tmp" >/dev/null 2>&1 || true
    if jq -e --argjson refund_at "$refund_at_ms" --argjson punish_at "$punish_at_ms" '.outcome.status=="found" and .outcome.facts.instruction.effect=="refund" and .outcome.facts.metadata.state=="refunded" and .outcome.facts.custody.balance=="0" and .outcome.facts.containing_block.timestamp_ms >= $refund_at and .outcome.facts.containing_block.timestamp_ms < $punish_at' "$result_tmp" >/dev/null 2>&1; then
      require_owner_file "$result_tmp" "Tag16 finalized refund observation"
      mv "$result_tmp" "$tag16_refund_finality_result"
      break
    fi
    if jq -e '(.outcome.status=="absent" or .outcome.status=="uncertain") and (.outcome.finalized_clock.timestamp_ms|type)=="number"' "$result_tmp" >/dev/null 2>&1; then
      finalized_timestamp_ms="$(jq -er '.outcome.finalized_clock.timestamp_ms' "$result_tmp")"
      (( finalized_timestamp_ms < punish_at_ms )) || fail "finalized LEZ clock reached punish_at before Tag16 finalized"
    fi
    sleep .25
  done
  require_owner_file "$tag16_refund_finality_result" "Tag16 finalized refund evidence"
  jq -e --argjson refund_at "$refund_at_ms" --argjson punish_at "$punish_at_ms" '.outcome.status=="found" and .outcome.facts.instruction.effect=="refund" and .outcome.facts.metadata.state=="refunded" and .outcome.facts.custody.balance=="0" and .outcome.facts.containing_block.timestamp_ms >= $refund_at and .outcome.facts.containing_block.timestamp_ms < $punish_at' "$tag16_refund_finality_result" >/dev/null || fail "Tag16 finalized refund evidence is incomplete"
  record_phase tag16_finality completed
}

ingest_refund_signature() {
  record_phase refund_ingestion started
  readonly maker_observed_refund_signature="${tag16_refund_root}/maker-observed-final-signature.json"
  "$agreement_actor_binary" ingest-finalized-refund-signature --private-root "${agreement_root}/material/maker" --own-public-packet "${agreement_root}/exchange/maker.json" --peer-public-packet "${agreement_root}/exchange/taker.json" --agreement-stage-a "$agreement_stage_a" --activation-stage-b "$agreement_stage_b" --journal "${agreement_root}/stage-b/private/maker.sqlite" --run-id "$run_id" --finalized-refund "$tag16_refund_finality_result" --output-final-signature "$maker_observed_refund_signature"
  require_owner_file "$maker_observed_refund_signature" "Maker observed refund final-signature packet"
  record_phase refund_ingestion completed
}

extract_refund_adaptor_scalar() {
  record_phase refund_extraction started
  readonly extracted_taker_scalar="${tag16_refund_root}/extracted-taker-adaptor.key"
  "$agreement_role_runner_binary" maker --journal "${agreement_root}/stage-b/private/maker.sqlite" --session "${agreement_root}/material/maker-sessions/refund.json" extract-adaptor-secret --presignature "${agreement_root}/stage-b/exchange/refund/maker-presignature.json" --final-signature "$maker_observed_refund_signature" --output "$extracted_taker_scalar"
  require_owner_file "$extracted_taker_scalar" "extracted Taker adaptor scalar"
  record_phase refund_extraction completed
}

sweep_monero_refund() {
  record_phase monero_refund_sweep started
  readonly monero_refund_sweep_evidence="${evidence_root}/monero-refund-sweep.json"
  "$monero_sweep_binary" --journey refund --run-id "$MONERO_RUN_ID" --agreement-wire-file "$agreement_stage_a" --maker-share-file "${agreement_root}/material/maker/xmr-share.key" --extracted-taker-adaptor-scalar-file "$extracted_taker_scalar" --monero-view-key-file "${agreement_root}/material/maker/monero-view.key" --daemon-url "${monero_env[MONERO_DAEMON_ENDPOINT]}" --daemon-username-file "${monero_env[MONERO_DAEMON_USERNAME_FILE]}" --daemon-password-file "${monero_env[MONERO_DAEMON_PASSWORD_FILE]}" --shared-wallet-url "${monero_env[MONERO_FUNDING_WALLET_ENDPOINT]}" --shared-wallet-username-file "${monero_env[MONERO_FUNDING_RPC_USERNAME_FILE]}" --shared-wallet-password-file "${monero_env[MONERO_FUNDING_RPC_PASSWORD_FILE]}" --shared-wallet-file-password-file "${monero_env[MONERO_FUNDING_WALLET_PASSWORD_FILE]}" --taker-wallet-url "${monero_env[MONERO_TAKER_WALLET_ENDPOINT]}" --taker-wallet-username-file "${monero_env[MONERO_TAKER_RPC_USERNAME_FILE]}" --taker-wallet-password-file "${monero_env[MONERO_TAKER_RPC_PASSWORD_FILE]}" --funding-wallet-url "${monero_env[MONERO_MAKER_WALLET_ENDPOINT]}" --funding-wallet-username-file "${monero_env[MONERO_MAKER_RPC_USERNAME_FILE]}" --funding-wallet-password-file "${monero_env[MONERO_MAKER_RPC_PASSWORD_FILE]}" --reconstructed-wallet-filename "m5-${MONERO_RUN_ID}-refund-reconstructed" --restore-height 0 --output-evidence "$monero_refund_sweep_evidence"
  require_owner_file "$monero_refund_sweep_evidence" "Monero refund sweep evidence"
  jq -e '.schema=="lez_v02_m5_actual_local_monero_refund_sweep_v3" and .journey=="refund" and .revealed_role=="taker_refund_signature" and .sweeping_role=="maker" and .public_rpc_used==false and .faucet_used==false and .automatic_submission_retry==false and .confirmations >= .required_confirmations and .peer_count==0' "$monero_refund_sweep_evidence" >/dev/null || fail "Monero refund sweep evidence is incomplete"
  rm -f -- "$monero_verification_evidence"
  "$monero_verify_binary" --agreement-wire-file "$agreement_stage_a" --monero-transaction-id "$(jq -er .transaction_id "$monero_refund_sweep_evidence")" --destination-address "$(jq -er .destination_address "$monero_refund_sweep_evidence")" --amount-piconero "$(jq -er .received_amount_piconero "$monero_refund_sweep_evidence")" --run-id "$MONERO_RUN_ID" --daemon-url "${monero_env[MONERO_DAEMON_ENDPOINT]}" --daemon-username-file "${monero_env[MONERO_DAEMON_USERNAME_FILE]}" --daemon-password-file "${monero_env[MONERO_DAEMON_PASSWORD_FILE]}" --target-wallet-url "${monero_env[MONERO_MAKER_WALLET_ENDPOINT]}" --target-wallet-username-file "${monero_env[MONERO_MAKER_RPC_USERNAME_FILE]}" --target-wallet-password-file "${monero_env[MONERO_MAKER_RPC_PASSWORD_FILE]}" --foreign-wallet-url "${monero_env[MONERO_TAKER_WALLET_ENDPOINT]}" --foreign-wallet-username-file "${monero_env[MONERO_TAKER_RPC_USERNAME_FILE]}" --foreign-wallet-password-file "${monero_env[MONERO_TAKER_RPC_PASSWORD_FILE]}" --output-evidence "$monero_verification_evidence" >/dev/null
  require_owner_file "$monero_verification_evidence" "post-refund Monero verification evidence"
  jq -e --arg tx "$(jq -er .transaction_id "$monero_refund_sweep_evidence")" --argjson amount "$(jq -er .received_amount_piconero "$monero_refund_sweep_evidence")" '.schema=="lez_v02_m4_actual_local_monero_verification_v2" and .transaction_id==$tx and .amount_piconero==$amount and .public_rpc_used==false and .faucet_used==false' "$monero_verification_evidence" >/dev/null || fail "post-refund Monero receipt is incomplete"
  record_phase monero_refund_sweep completed
}

bind_refund_sweep() {
  record_phase refund_evidence started
  readonly refund_sweep_binding="${evidence_root}/refund-sweep-binding.json"
  "$agreement_actor_binary" bind-finalized-refund-sweep --private-root "${agreement_root}/material/maker" --own-public-packet "${agreement_root}/exchange/maker.json" --peer-public-packet "${agreement_root}/exchange/taker.json" --agreement-stage-a "$agreement_stage_a" --activation-stage-b "$agreement_stage_b" --journal "${agreement_root}/stage-b/private/maker.sqlite" --run-id "$MONERO_RUN_ID" --refund-run-id "$run_id" --finalized-refund "$tag16_refund_finality_result" --observed-final-signature "$maker_observed_refund_signature" --extracted-taker-adaptor-scalar "$extracted_taker_scalar" --monero-sweep-evidence "$monero_refund_sweep_evidence" --monero-receipt-evidence "$monero_verification_evidence" --output-binding-evidence "$refund_sweep_binding"
  require_owner_file "$refund_sweep_binding" "refund/sweep binding evidence"
  jq -e '.schema=="lez_v02_m5_refund_cross_chain_binding_v1" and .atomicity_scope=="successful_refund_path_conditional_atomicity" and .distributed_cross_chain_transaction_claimed==false and .lez_effect=="refund" and .lez_sidecar_role=="maker" and .classifier_target=="discover_by_terms" and .classifier_outcome=="found" and .destination_ownership_binding=="owner_private_maker_wallet_boundary_not_stage_a_committed"' "$refund_sweep_binding" >/dev/null || fail "refund/sweep binding evidence is incomplete"
  record_phase refund_evidence completed
}

prepare_tag14_release() {
  record_phase release started
  readonly release_root="${private_root}/tag14-release"
  readonly release_config_root="${release_root}/config"
  readonly release_state_root="${release_root}/state"
  readonly release_public_config="${release_config_root}/release.json"
  readonly release_preparation_config="${release_config_root}/preparation.json"
  readonly release_protection_key="${release_root}/protection.key"
  mkdir -m 0700 "$release_root" "$release_config_root" "$release_state_root"
  record_resource ephemeral_path "$release_root" "$release_root"
  openssl rand -hex 32 >"$release_protection_key"
  chmod 600 "$release_protection_key"
  local taker_endpoint
  taker_endpoint="$(jq -er ' .endpoint ' "$taker_sidecar_root/pid-manifest.json")"
  jq -n --slurpfile evidence "$tag13_internal" --arg sidecar "$taker_endpoint" --arg indexer "$(manifest_value LEZ_INDEXER_RPC_URL "$lez_stack_manifest")" '{schema_version:1,sidecar_endpoint:$sidecar,indexer_endpoint:$indexer,node_profile:"local",run_id:$evidence[0].run_id,runtime:$evidence[0].runtime,terms:$evidence[0].terms,protection_key_id:"m4-local-release-key-001"}' >"$release_public_config"
  jq -n --slurpfile evidence "$tag13_internal" --arg fund_id "m4-${run_id}-fund-finality" --arg authorization_id "m4-${run_id}-authorization-prepare" --arg txid "$(jq -er '.transaction_id' "$monero_funding_evidence")" --arg daemon "${monero_env[MONERO_DAEMON_ENDPOINT]}" --arg target "${monero_env[MONERO_FUNDING_WALLET_ENDPOINT]}" --arg foreign "${monero_env[MONERO_TAKER_WALLET_ENDPOINT]}" '{schema_version:1,escrow_prepare_request_id:$evidence[0].prepare_request_id,fund_finality_request_id:$fund_id,authorization_prepare_request_id:$authorization_id,fund_finality_window:$evidence[0].funding.scanned_window,monero_funding_transaction_id:$txid,monero_daemon_endpoint:$daemon,monero_target_wallet_endpoint:$target,monero_foreign_wallet_endpoint:$foreign}' >"$release_preparation_config"
  chmod 600 "$release_public_config" "$release_preparation_config"
  "$release_prepare_binary" --public-config-file "$release_public_config" --preparation-config-file "$release_preparation_config" --agreement-wire-file "$agreement_stage_a" --activation-wire-file "$agreement_stage_b" --monero-view-key-file "${agreement_root}/material/taker/monero-view.key" --taker-claim-journal "${agreement_root}/stage-b/private/taker.sqlite" --bridge-capability-file "$taker_sidecar_root/capability" --protection-key-file "$release_protection_key" --state-directory "$release_state_root" --daemon-username-file "${monero_env[MONERO_DAEMON_USERNAME_FILE]}" --daemon-password-file "${monero_env[MONERO_DAEMON_PASSWORD_FILE]}" --target-wallet-username-file "${monero_env[MONERO_FUNDING_RPC_USERNAME_FILE]}" --target-wallet-password-file "${monero_env[MONERO_FUNDING_RPC_PASSWORD_FILE]}" --foreign-wallet-username-file "${monero_env[MONERO_TAKER_RPC_USERNAME_FILE]}" --foreign-wallet-password-file "${monero_env[MONERO_TAKER_RPC_PASSWORD_FILE]}" >"${release_root}/preparation-result.json"
  require_owner_file "${release_root}/preparation-result.json" "Tag14 preparation result"
  jq -e '.schema_version==1 and .event=="xmr_claim_authorization_preparation" and .durable_state=="prepared" and .node_profile=="local"' "${release_root}/preparation-result.json" >/dev/null || fail "Tag14 preparation result is incomplete"
  record_phase release completed
}

publish_tag14_release() {
  record_phase tag14_publication started
  readonly tag14_publication_result="${release_root}/publication-result.json"
  "$release_service_binary" --public-config-file "$release_public_config" --state-directory "$release_state_root" --sidecar-capability-file "$taker_sidecar_root/capability" --protection-key-file "$release_protection_key" >"$tag14_publication_result"
  require_owner_file "$tag14_publication_result" "Tag14 publication result"
  jq -e '.schema_version==1 and .event=="xmr_claim_authorization_publication" and (.durable_state=="admitted" or .durable_state=="already_known")' "$tag14_publication_result" >/dev/null || fail "Tag14 publication result is not durably admitted"
  record_phase tag14_publication completed
}

classify_tag14_finality() {
  record_phase tag14_finality started
  readonly tag14_finality_result="${evidence_root}/tag14-finalized.json"
  local maker_endpoint start_height attempt result_tmp
  maker_endpoint="$(jq -er '.endpoint' "$maker_sidecar_root/pid-manifest.json")"
  start_height="$(jq -er '.funding.containing_block_id + 1' "$tag13_internal")"
  result_tmp="${tag14_finality_result}.attempt"
  for attempt in {1..600}; do
    rm -f "$result_tmp"
    "$classifier_binary" --sidecar-endpoint "$maker_endpoint" --capability-file "$maker_sidecar_root/capability" --runtime-file "$tag13_handoff_root/maker-runtime.json" --terms-file "$tag13_handoff_root/terms.json" --run-id "$run_id" --request-id "${run_id}-tag14-finality-${attempt}" --role maker --effect authorize-claim --start-height "$start_height" --max-blocks 16 --output-result "$result_tmp" || true
    if jq -e '.outcome.status=="found" and .outcome.facts.instruction.effect=="authorize_claim"' "$result_tmp" >/dev/null 2>&1; then
      mv "$result_tmp" "$tag14_finality_result"
      break
    fi
    sleep .25
  done
  require_owner_file "$tag14_finality_result" "Tag14 finality result"
  jq -e '.outcome.status=="found" and .outcome.facts.instruction.effect=="authorize_claim"' "$tag14_finality_result" >/dev/null || fail "Tag14 finality result is incomplete"
  record_phase tag14_finality completed
}

prepare_tag15_signature() {
  record_phase tag15_prepare started
  readonly maker_final_signature="${private_root}/tag14-release/maker-final-signature.json"
  "$agreement_actor_binary" complete-claim-from-finalized-authorization --private-root "${agreement_root}/material/maker" --own-public-packet "${agreement_root}/exchange/maker.json" --peer-public-packet "${agreement_root}/exchange/taker.json" --agreement-stage-a "$agreement_stage_a" --activation-stage-b "$agreement_stage_b" --journal "${agreement_root}/stage-b/private/maker.sqlite" --run-id "$run_id" --finalized-authorization "$tag14_finality_result" --output-final-signature "$maker_final_signature"
  require_owner_file "$maker_final_signature" "Maker final-signature packet"
  record_phase tag15_prepare completed
}

publish_tag15() {
  record_phase tag15 started
  readonly tag15_submission="${evidence_root}/tag15-submission.json"
  local maker_endpoint
  maker_endpoint="$(jq -er '.endpoint' "$maker_sidecar_root/pid-manifest.json")"
  "$tag15_binary" --sidecar-endpoint "$maker_endpoint" --capability-file "$maker_sidecar_root/capability" --runtime-file "$tag13_handoff_root/maker-runtime.json" --agreement-wire-file "$agreement_stage_a" --activation-wire-file "$agreement_stage_b" --monero-view-key-file "${agreement_root}/material/taker/monero-view.key" --final-signature-file "$maker_final_signature" --run-id "$run_id" --prepare-request-id "${run_id}-tag15-prepare-001" --complete-request-id "${run_id}-tag15-complete-001" --output-evidence "$tag15_submission"
  require_owner_file "$tag15_submission" "Tag15 submission evidence"
  jq -e '(.schema|type=="string") and .submission_outcome != null and .automatic_submission_retry == false and .public_rpc_used == false' "$tag15_submission" >/dev/null || fail "Tag15 submission evidence is incomplete"
  record_phase tag15 completed
}

classify_tag15_finality() {
  record_phase tag15_finality started
  readonly tag15_finality_result="${evidence_root}/tag15-finalized.json"
  local taker_endpoint start_height attempt result_tmp
  taker_endpoint="$(jq -er '.endpoint' "$taker_sidecar_root/pid-manifest.json")"
  start_height="$(jq -er '.outcome.facts.containing_block.block_id + 1' "$tag14_finality_result")"
  result_tmp="${tag15_finality_result}.attempt"
  for attempt in {1..600}; do
    rm -f "$result_tmp"
    "$classifier_binary" --sidecar-endpoint "$taker_endpoint" --capability-file "$taker_sidecar_root/capability" --runtime-file "$tag13_handoff_root/taker-runtime.json" --terms-file "$tag13_handoff_root/terms.json" --run-id "$run_id" --request-id "${run_id}-tag15-finality-${attempt}" --role taker --effect claim --start-height "$start_height" --max-blocks 16 --output-result "$result_tmp" || true
    if jq -e '.outcome.status=="found" and .outcome.facts.instruction.effect=="claim" and .outcome.facts.metadata.state=="claimed" and .outcome.facts.custody.balance=="0"' "$result_tmp" >/dev/null 2>&1; then
      mv "$result_tmp" "$tag15_finality_result"
      break
    fi
    sleep .25
  done
  require_owner_file "$tag15_finality_result" "Tag15 finality result"
  jq -e '.outcome.status=="found" and .outcome.facts.instruction.effect=="claim" and .outcome.facts.metadata.state=="claimed" and .outcome.facts.custody.balance=="0"' "$tag15_finality_result" >/dev/null || fail "Tag15 finality result is incomplete"
  record_phase tag15_finality completed
}

extract_claim_signature() {
  record_phase extraction started
  readonly observed_final_signature="${private_root}/tag14-release/taker-observed-final-signature.json"
  "$agreement_actor_binary" ingest-finalized-claim-signature --private-root "${agreement_root}/material/taker" --own-public-packet "${agreement_root}/exchange/taker.json" --peer-public-packet "${agreement_root}/exchange/maker.json" --agreement-stage-a "$agreement_stage_a" --activation-stage-b "$agreement_stage_b" --journal "${agreement_root}/stage-b/private/taker.sqlite" --run-id "$run_id" --finalized-claim "$tag15_finality_result" --output-final-signature "$observed_final_signature"
  require_owner_file "$observed_final_signature" "Taker observed final-signature packet"
  record_phase extraction completed
}

extract_adaptor_scalar() {
  record_phase extraction_scalar started
  readonly extracted_maker_scalar="${private_root}/tag14-release/extracted-maker-adaptor.key"
  "$agreement_role_runner_binary" taker --journal "${agreement_root}/stage-b/private/taker.sqlite" --session "${agreement_root}/material/taker-sessions/claim.json" extract-adaptor-secret --presignature "${agreement_root}/stage-b/private/taker-outbox/claim-presignature.json" --final-signature "$observed_final_signature" --output "$extracted_maker_scalar"
  require_owner_file "$extracted_maker_scalar" "extracted Maker adaptor scalar"
  record_phase extraction_scalar completed
}

sweep_monero_claim() {
  record_phase monero_sweep started
  readonly monero_sweep_evidence="${evidence_root}/monero-sweep.json"
  "$monero_sweep_binary" --run-id "$MONERO_RUN_ID" --agreement-wire-file "$agreement_stage_a" --taker-share-file "${agreement_root}/material/taker/xmr-share.key" --extracted-maker-adaptor-scalar-file "$extracted_maker_scalar" --monero-view-key-file "${agreement_root}/material/taker/monero-view.key" --daemon-url "${monero_env[MONERO_DAEMON_ENDPOINT]}" --daemon-username-file "${monero_env[MONERO_DAEMON_USERNAME_FILE]}" --daemon-password-file "${monero_env[MONERO_DAEMON_PASSWORD_FILE]}" --shared-wallet-url "${monero_env[MONERO_FUNDING_WALLET_ENDPOINT]}" --shared-wallet-username-file "${monero_env[MONERO_FUNDING_RPC_USERNAME_FILE]}" --shared-wallet-password-file "${monero_env[MONERO_FUNDING_RPC_PASSWORD_FILE]}" --shared-wallet-file-password-file "${monero_env[MONERO_FUNDING_WALLET_PASSWORD_FILE]}" --taker-wallet-url "${monero_env[MONERO_TAKER_WALLET_ENDPOINT]}" --taker-wallet-username-file "${monero_env[MONERO_TAKER_RPC_USERNAME_FILE]}" --taker-wallet-password-file "${monero_env[MONERO_TAKER_RPC_PASSWORD_FILE]}" --funding-wallet-url "${monero_env[MONERO_MAKER_WALLET_ENDPOINT]}" --funding-wallet-username-file "${monero_env[MONERO_MAKER_RPC_USERNAME_FILE]}" --funding-wallet-password-file "${monero_env[MONERO_MAKER_RPC_PASSWORD_FILE]}" --reconstructed-wallet-filename "m4-${MONERO_RUN_ID}-reconstructed" --restore-height 0 --output-evidence "$monero_sweep_evidence"
  require_owner_file "$monero_sweep_evidence" "Monero sweep evidence"
  jq -e '.schema=="lez_v02_m4_actual_local_monero_claim_sweep_v2" and .public_rpc_used==false and .faucet_used==false and .automatic_submission_retry==false and .confirmations >= .required_confirmations and .peer_count==0' "$monero_sweep_evidence" >/dev/null || fail "Monero sweep evidence is incomplete"
  # The pre-sweep receipt proves the funding output; bind a second, exact receipt
  # to the post-fee sweep before the actor validates the pair.
  # The verifier uses create-new semantics; replace the funding receipt only after the sweep evidence is validated.
  rm -f -- "$monero_verification_evidence"
  "$monero_verify_binary" --agreement-wire-file "$agreement_stage_a" --monero-transaction-id "$(jq -er .transaction_id "$monero_sweep_evidence")" --destination-address "$(jq -er .destination_address "$monero_sweep_evidence")" --amount-piconero "$(jq -er .received_amount_piconero "$monero_sweep_evidence")" --run-id "$MONERO_RUN_ID" --daemon-url "${monero_env[MONERO_DAEMON_ENDPOINT]}" --daemon-username-file "${monero_env[MONERO_DAEMON_USERNAME_FILE]}" --daemon-password-file "${monero_env[MONERO_DAEMON_PASSWORD_FILE]}" --target-wallet-url "${monero_env[MONERO_TAKER_WALLET_ENDPOINT]}" --target-wallet-username-file "${monero_env[MONERO_TAKER_RPC_USERNAME_FILE]}" --target-wallet-password-file "${monero_env[MONERO_TAKER_RPC_PASSWORD_FILE]}" --foreign-wallet-url "${monero_env[MONERO_MAKER_WALLET_ENDPOINT]}" --foreign-wallet-username-file "${monero_env[MONERO_MAKER_RPC_USERNAME_FILE]}" --foreign-wallet-password-file "${monero_env[MONERO_MAKER_RPC_PASSWORD_FILE]}" --output-evidence "$monero_verification_evidence" >/dev/null
  require_owner_file "$monero_verification_evidence" "post-sweep Monero verification evidence"
  jq -e --arg tx "$(jq -er .transaction_id "$monero_sweep_evidence")" --argjson amount "$(jq -er .received_amount_piconero "$monero_sweep_evidence")" '.schema=="lez_v02_m4_actual_local_monero_verification_v2" and .transaction_id==$tx and .amount_piconero==$amount and .public_rpc_used==false and .faucet_used==false' "$monero_verification_evidence" >/dev/null || fail "post-sweep Monero receipt is incomplete"
  record_phase monero_sweep completed
}

bind_claim_sweep() {
  record_phase evidence started
  readonly claim_sweep_binding="${evidence_root}/claim-sweep-binding.json"
  "$agreement_actor_binary" bind-finalized-claim-sweep --private-root "${agreement_root}/material/taker" --own-public-packet "${agreement_root}/exchange/taker.json" --peer-public-packet "${agreement_root}/exchange/maker.json" --agreement-stage-a "$agreement_stage_a" --activation-stage-b "$agreement_stage_b" --journal "${agreement_root}/stage-b/private/taker.sqlite" --run-id "$MONERO_RUN_ID" --claim-run-id "$run_id" --finalized-claim "$tag15_finality_result" --observed-final-signature "$observed_final_signature" --extracted-maker-adaptor-scalar "$extracted_maker_scalar" --monero-sweep-evidence "$monero_sweep_evidence" --monero-receipt-evidence "$monero_verification_evidence" --output-binding-evidence "$claim_sweep_binding"
  require_owner_file "$claim_sweep_binding" "claim/sweep binding evidence"
  jq -e '.schema=="lez_v02_m4_claim_cross_chain_binding_v1" and .distributed_cross_chain_transaction_claimed==false and .lez_effect=="claim" and .lez_sidecar_role=="taker"' "$claim_sweep_binding" >/dev/null || fail "claim/sweep binding evidence is incomplete"
  record_phase evidence completed
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
  if [[ "$m5_xmr_application_mode" == 1 ]]; then
    prepare_m5_xmr_delivery_plan
  fi
  compose_xmr_agreement
  if [[ "$m5_xmr_application_mode" == 1 ]]; then
    complete_m5_xmr_application_handoff
    verify_m5_xmr_application_cutoff
  fi
  submit_tag13
  export_tag13_handoff
  start_role_sidecars
  fund_and_verify_monero
  if [[ "$m5_xmr_application_mode" == 1 && "$m5_xmr_journey" == refund ]]; then
    wait_for_m5_xmr_refund_window
    prepare_tag16_refund_signature
    publish_tag16_refund
    classify_tag16_refund_finality
    ingest_refund_signature
    extract_refund_adaptor_scalar
    sweep_monero_refund
    bind_refund_sweep
  else
    prepare_tag14_release
    publish_tag14_release
    classify_tag14_finality
    prepare_tag15_signature
    publish_tag15
    classify_tag15_finality
    extract_claim_signature
    extract_adaptor_scalar
    sweep_monero_claim
    bind_claim_sweep
  fi
  return 0
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
