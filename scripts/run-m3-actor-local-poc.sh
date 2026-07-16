#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

export LC_ALL=C
umask 077

readonly mode="${M3_ACTOR_POC_MODE:-execute}"
run_id="${RUN_ID:-m3local-$(date -u +%Y%m%d%H%M%S)-$$}"
if [[ ! "$run_id" =~ ^[a-z0-9][a-z0-9_-]{7,47}$ ]]; then
  echo "RUN_ID must be 8..48 lowercase letters, numbers, underscores, or hyphens" >&2
  exit 2
fi
if [[ "$mode" != "execute" && "$mode" != "contract" ]]; then
  echo "M3_ACTOR_POC_MODE must be execute or contract" >&2
  exit 2
fi
journey="${M3_ACTOR_POC_JOURNEY:-claim}"
if [[ "$journey" != "claim" && "$journey" != "refund" &&
      "$journey" != "first_lock_refund" ]]; then
  echo "M3_ACTOR_POC_JOURNEY must be claim, refund, or first_lock_refund" >&2
  exit 2
fi
case "$journey" in
  claim)
    terminal_revision=4
    terminal_phase="completed"
    replay_command="drive"
    packet_kind="m3_actor_two_direction_local_poc"
    actor_owned_effect_semantics="claim"
    success_label="M3 actor two-direction local PoC"
    lez_slot_duration_seconds="1.0"
    ;;
  refund)
    terminal_revision=4
    terminal_phase="refunded"
    replay_command="recover"
    packet_kind="m3_actor_two_direction_refund_local_poc"
    actor_owned_effect_semantics="refund"
    success_label="M3 actor two-direction local refund PoC"
    lez_slot_duration_seconds="3.0"
    ;;
  first_lock_refund)
    terminal_revision=2
    terminal_phase="refunded"
    replay_command="recover"
    packet_kind="m3_actor_two_direction_first_lock_refund_local_poc"
    actor_owned_effect_semantics="first_lock_refund"
    success_label="M3 actor two-direction first-lock refund local PoC"
    lez_slot_duration_seconds="3.0"
    ;;
esac

readonly journey terminal_revision terminal_phase replay_command packet_kind
readonly actor_owned_effect_semantics success_label
readonly lez_slot_duration_seconds
readonly run_id
repo_root="$(pwd)"
readonly repo_root
readonly bitcoin_run_id="${run_id}-btc"
readonly lez_run_id="${run_id}-lez"
readonly relative_run_root=".e2e/${run_id}/m3-actor-poc"
readonly run_root="${repo_root}/${relative_run_root}"
readonly evidence_dir="${run_root}/evidence"
readonly private_dir="${run_root}/private"
readonly identities_dir="${private_dir}/lez-identities"
readonly directions_dir="${private_dir}/directions"
readonly process_registry="${private_dir}/owned-processes.ndjson"
readonly secure_state_root="/tmp/lez-atomic-swaps-m3-${run_id}-secure-state"
readonly lez_bootstrap_root="${secure_state_root}/bootstrap"
readonly lez_bootstrap_manifest="${private_dir}/lez-bootstrap.env"
readonly bitcoin_manifest="${repo_root}/.e2e/${bitcoin_run_id}/bitcoin-core/run.env"
readonly lez_manifest="${repo_root}/.e2e/${lez_run_id}/lez-v02/run.env"
readonly bitcoin_container_ids="${private_dir}/bitcoin-containers.ids"
readonly lez_container_ids="${private_dir}/lez-containers.ids"
readonly network_resources="${private_dir}/owned-networks.tsv"
readonly volume_resources="${private_dir}/owned-volumes.tsv"
readonly image_resources="${private_dir}/owned-images.tsv"
readonly cleanup_attestation="${evidence_dir}/cleanup-attestation.json"
readonly run_evidence="${evidence_dir}/m3-actor-local-poc.json"
readonly toolchain="${M3_RUST_TOOLCHAIN:-1.96.0}"
readonly direction_driver="${repo_root}/scripts/run-m3-actor-direction.sh"
readonly lez_bootstrap_driver="${repo_root}/scripts/run-m3-lez-bootstrap.sh"
readonly -a directions=(taker_sells_foreign taker_sells_lez)
readonly rapidsnark_sha="d4133227f845ff5bfa3672eb5b9c018a6a086bfa164b176bdaf76949c7d1f423"
readonly gmp_sha="0a910b420c3ad603c83c9dc2818c7ae05394c231ca23135c7b873e8e680ea41b"
readonly fq_sha="797b5d24bb8e8b088f811bddfff35f33973af9c797fb3812489cd42ba6a957d0"
readonly fr_sha="40f809394904682cb5517845cd3c2f936a5eb4609712534b573f552f2811fb82"

emit_contract() {
  jq -n \
    --arg run_id "$run_id" \
    --arg run_root "$relative_run_root" \
    --arg bitcoin_run "$bitcoin_run_id" \
    --arg lez_run "$lez_run_id" \
    --arg lez_slot_duration_seconds "$lez_slot_duration_seconds" \
    --arg journey "$journey" \
    --argjson terminal_revision "$terminal_revision" \
    --arg terminal_phase "$terminal_phase" \
    --arg replay_command "$replay_command" \
    --arg packet_kind "$packet_kind" \
    --arg actor_owned_effect_semantics "$actor_owned_effect_semantics" '
    {
      schema_version: 1,
      kind: "m3_actor_local_poc_contract",
      execution_performed: false,
      journey: $journey,
      evidence_packet_kind: $packet_kind,
      run_id: $run_id,
      run_root: $run_root,
      service_runs: {
        bitcoin_core: $bitcoin_run,
        lez_v0_2: $lez_run
      },
      service_configuration: {
        lez_v0_2: {slot_duration_seconds: $lez_slot_duration_seconds}
      },
      directions: ["taker_sells_foreign", "taker_sells_lez"],
      process_model: {
        actor: "fresh_process_for_every_command_and_revision",
        roles: ["maker", "taker"],
        state: "separate_role_configs_state_dbs_and_signing_journals"
      },
      ordering: {
        stage_one_before_node_facts: true,
        official_nssa_before_stage_two: true,
        stage_two_after_actual_node_facts: true,
        taker_first_effects: true,
        dual_locks_before_scalar_use: ($journey != "first_lock_refund"),
        first_lock_refund_has_no_second_lock_or_dual_lock_gate:
          ($journey == "first_lock_refund"),
        directions_are_sequential: true
      },
      finality: {
        bitcoin: "exact_signed_confirmation_depth",
        lez: "exact_finalized_indexer_ancestry"
      },
      effect_semantics: {
        actor_owned: $actor_owned_effect_semantics,
        expected_unique_effects_by_direction:
          (if $journey == "first_lock_refund" then
             {taker_sells_foreign:{bitcoin:2,lez:0},
              taker_sells_lez:{bitcoin:0,lez:3}}
           else
             {taker_sells_foreign:{bitcoin:2,lez:3},
              taker_sells_lez:{bitcoin:2,lez:3}}
           end),
        maker_second_lock_effect_count:
          (if $journey == "first_lock_refund" then 0 else 1 end),
        accepted_submission_alone_projects: false
      },
      first_lock_refund:
        (if $journey == "first_lock_refund" then {
          signed_maker_second_lock_cutoff_required:true,
          two_fresh_absence_and_first_lock_unspent_reads_required:true,
          lez_absence_window_reaches_current_finalized_tip:true,
          bitcoin_cutoff_clock:"stable_core_median_time",
          actor_internal_admission_is_authoritative:true,
          owner_restart_without_resubmission:true,
          fresh_maker_observer_terminal:true,
          maker_offline_after_activation_until_refund_finality:true,
          taker_only_revision_one_and_refund_projection:true
        } else null end),
      terminal: {
        required_revision: $terminal_revision,
        required_phase: $terminal_phase,
        required_next_action: "complete"
      },
      replay: {
        command: $replay_command,
        restart_both_roles: true,
        resubmission_count: 0
      },
      isolation: {
        compose_projects: "run_scoped",
        dynamic_literal_loopback_ports: true,
        private_e2e_roots: true,
        secure_reservation_state: "exact_run_owned_tmp_root",
        foreign_resource_mutation: false
      },
      cleanup: {
        captured_exact_ids_only: true,
        secure_reservation_state_root_removed: true,
        process_exit_race_silent: true,
        runs_on_success_and_failure: true,
        broad_cleanup_used: false
      },
      evidence: {
        secret_safe_json: true,
        cleanup_attestation: true,
        executable_script_sha256s: ["outer_runner", "direction_driver", "lez_bootstrap"]
      },
      build_prerequisites: {
        rapidsnark_lib_dir: "explicit_absolute_canonical_verified_v0_0_8",
        rapidsnark_files: ["librapidsnark.a","libgmp.a","libfq.a","libfr.a"],
        bindgen_extra_clang_args: "explicit_nonempty",
        inherited_by_offline_sidecar_build: true
      },
      external_resources: {
        public_rpc: false,
        faucet: false,
        public_funds: false,
        test_funds: "deterministic_local_genesis_and_regtest_outputs"
      }
    }'
}

if [[ "$mode" == "contract" ]]; then
  command -v jq >/dev/null || {
    echo "jq is required to emit the M3 actor contract" >&2
    exit 2
  }
  emit_contract
  exit 0
fi

fail() {
  echo "M3 actor local PoC failed: $*" >&2
  exit 2
}

for command_name in cargo chmod curl date docker git id jq kill mkdir mv readlink rg rm sed sha256sum sleep stat; do
  command -v "$command_name" >/dev/null || fail "missing required tool: ${command_name}"
done

[[ -z "${M3_ACTOR_DIRECTION_DRIVER+x}" ]] ||
  fail "M3_ACTOR_DIRECTION_DRIVER overrides are non-certifying and forbidden"
[[ -x "$direction_driver" && ! -L "$direction_driver" ]] ||
  fail "M3 actor direction driver is missing or unsafe: ${direction_driver}"
[[ "$(readlink -f "$direction_driver")" == "$direction_driver" ]] ||
  fail "M3 actor direction driver path is not canonical"
[[ -x "$lez_bootstrap_driver" && ! -L "$lez_bootstrap_driver" ]] ||
  fail "M3 LEZ bootstrap driver is missing or unsafe"
[[ -n "${LEZ_V02_ARTIFACT_TARGET_DIR:-}" && "$LEZ_V02_ARTIFACT_TARGET_DIR" == /* &&
   -d "$LEZ_V02_ARTIFACT_TARGET_DIR" && ! -L "$LEZ_V02_ARTIFACT_TARGET_DIR" ]] ||
  fail "set LEZ_V02_ARTIFACT_TARGET_DIR to one verified absolute artifact target"
readonly lez_deployer="${LEZ_V02_ARTIFACT_TARGET_DIR}/debug/lez-zec-escrow-v02-deployer"
[[ -x "$lez_deployer" && -f "$lez_deployer" && ! -L "$lez_deployer" ]] ||
  fail "verified LEZ deployer is unavailable in the artifact target"

validate_native_build_prerequisites() {
  local directory="${RAPIDSNARK_LIB_DIR:-}" bindgen="${BINDGEN_EXTRA_CLANG_ARGS:-}"
  local entry file expected actual
  [[ -n "$directory" && "$directory" == /* && -d "$directory" && ! -L "$directory" &&
     "$(readlink -f "$directory")" == "$directory" ]] ||
    fail "set RAPIDSNARK_LIB_DIR to the canonical verified absolute v0.0.8 library directory"
  [[ -n "$bindgen" && "${#bindgen}" -le 1024 && "$bindgen" != *$'\n'* &&
     "$bindgen" != *$'\r'* ]] ||
    fail "set BINDGEN_EXTRA_CLANG_ARGS to one explicit nonempty bounded argument string"
  for entry in \
    "librapidsnark.a:$rapidsnark_sha" "libgmp.a:$gmp_sha" \
    "libfq.a:$fq_sha" "libfr.a:$fr_sha"; do
    file="${entry%%:*}"
    expected="${entry#*:}"
    [[ -f "${directory}/${file}" && ! -L "${directory}/${file}" ]] ||
      fail "verified RAPIDSNARK prerequisite is missing or unsafe: ${file}"
    actual="$(sha256sum "${directory}/${file}" | sed 's/ .*//')"
    [[ "$actual" == "$expected" ]] ||
      fail "verified RAPIDSNARK prerequisite hash mismatch: ${file}"
  done
  export RAPIDSNARK_LIB_DIR="$directory"
  export BINDGEN_EXTRA_CLANG_ARGS="$bindgen"
}

validate_native_build_prerequisites

for path in "$run_root" ".e2e/${bitcoin_run_id}" ".e2e/${lez_run_id}" \
  "$secure_state_root"; do
  [[ ! -e "$path" && ! -L "$path" ]] || fail "refusing to reuse run state: ${path}"
done

for child_run in "$bitcoin_run_id" "$lez_run_id"; do
  if [[ -n "$(docker container ls --all --quiet \
      --filter "label=org.logos-co.atomic-swaps.run=${child_run}")" ]] ||
     [[ -n "$(docker network ls --quiet \
      --filter "label=org.logos-co.atomic-swaps.run=${child_run}")" ]] ||
     [[ -n "$(docker volume ls --quiet \
      --filter "label=org.logos-co.atomic-swaps.run=${child_run}")" ]] ||
     [[ -n "$(docker image ls --quiet \
      --filter "label=org.logos-co.atomic-swaps.run=${child_run}")" ]]; then
    fail "refusing to reuse Docker resources for child run ${child_run}"
  fi
done

mkdir -p "$evidence_dir" "$identities_dir" "$directions_dir"
chmod 0700 "$run_root" "$evidence_dir" "$private_dir" "$identities_dir" "$directions_dir"
for registry in "$process_registry" "$network_resources" "$volume_resources" "$image_resources"; do
  : >"$registry"
  chmod 0600 "$registry"
done

process_matches_registry() {
  local pid="$1"
  local expected_start="$2"
  local expected_executable="$3"
  local actual_start actual_executable
  [[ "$pid" =~ ^[1-9][0-9]*$ && -r "/proc/${pid}/stat" ]] || return 1
  actual_start="$(sed -E 's/^[^(]*\([^)]*\) [^ ]+( [^ ]+){18} ([^ ]+).*/\2/' \
    "/proc/${pid}/stat" 2>/dev/null || true)"
  actual_executable="$(readlink -f "/proc/${pid}/exe" 2>/dev/null || true)"
  [[ "$actual_start" == "$expected_start" && "$actual_executable" == "$expected_executable" ]]
}

stop_owned_processes() {
  local record pid start executable
  [[ -f "$process_registry" ]] || return 0
  while IFS= read -r record; do
    [[ -n "$record" ]] || continue
    pid="$(jq -er '.pid | numbers' <<<"$record" 2>/dev/null || true)"
    start="$(jq -er '.start_ticks | strings' <<<"$record" 2>/dev/null || true)"
    executable="$(jq -er '.executable | strings' <<<"$record" 2>/dev/null || true)"
    [[ -n "$pid" && -n "$start" && "$executable" == /* ]] || continue
    if process_matches_registry "$pid" "$start" "$executable"; then
      kill -TERM "$pid" 2>/dev/null || true
      for _ in {1..100}; do
        process_matches_registry "$pid" "$start" "$executable" || break
        sleep 0.05
      done
    fi
    if process_matches_registry "$pid" "$start" "$executable"; then
      kill -KILL "$pid" 2>/dev/null || true
    fi
  done <"$process_registry"
}

assert_exact_owned_resource() {
  local kind="$1"
  local resource="$2"
  local expected_run="$3"
  local actual_run
  case "$kind" in
    container)
      actual_run="$(docker container inspect --format \
        '{{ index .Config.Labels "org.logos-co.atomic-swaps.run" }}' "$resource")"
      ;;
    image)
      actual_run="$(docker image inspect --format \
        '{{ index .Config.Labels "org.logos-co.atomic-swaps.run" }}' "$resource")"
      ;;
    network)
      actual_run="$(docker network inspect --format \
        '{{ index .Labels "org.logos-co.atomic-swaps.run" }}' "$resource")"
      ;;
    volume)
      actual_run="$(docker volume inspect --format \
        '{{ index .Labels "org.logos-co.atomic-swaps.run" }}' "$resource")"
      ;;
    *) return 1 ;;
  esac
  [[ "$actual_run" == "$expected_run" ]]
}

remove_exact_container_file() {
  local ids_file="$1"
  local expected_run="$2"
  local container_id
  [[ -f "$ids_file" ]] || return 0
  while IFS= read -r container_id; do
    [[ -n "$container_id" ]] || continue
    if docker container inspect "$container_id" >/dev/null 2>&1; then
      assert_exact_owned_resource container "$container_id" "$expected_run" || return 1
      docker container rm --force "$container_id" >/dev/null || return 1
    fi
  done <"$ids_file"
}

remove_exact_resource() {
  local kind="$1"
  local resource="$2"
  local expected_run="$3"
  if ! docker "$kind" inspect "$resource" >/dev/null 2>&1; then
    return 0
  fi
  assert_exact_owned_resource "$kind" "$resource" "$expected_run" || return 1
  docker "$kind" rm "$resource" >/dev/null
}

capture_owned_resources() {
  local kind="$1"
  local child_run="$2"
  local expected_count="$3"
  local output="$4"
  local format
  local -a resources=()
  local resource
  case "$kind" in
    image) format='{{.Repository}}:{{.Tag}}' ;;
    network) format='{{.ID}}' ;;
    volume) format='{{.Name}}' ;;
    *) fail "unsupported resource inventory kind: ${kind}" ;;
  esac
  mapfile -t resources < <(docker "$kind" ls --format "$format" \
    --filter "label=org.logos-co.atomic-swaps.run=${child_run}")
  [[ "${#resources[@]}" == "$expected_count" ]] ||
    fail "child run ${child_run} has ${#resources[@]} ${kind} resources; expected ${expected_count}"
  for resource in "${resources[@]}"; do
    [[ -n "$resource" && "$resource" != '<none>:<none>' ]] ||
      fail "child run ${child_run} has an unnamed ${kind} resource"
    assert_exact_owned_resource "$kind" "$resource" "$child_run" ||
      fail "${kind} ${resource} is not owned by ${child_run}"
    printf '%s\t%s\n' "$child_run" "$resource" >>"$output"
  done
}

remove_exact_resource_file() {
  local kind="$1"
  local resources_file="$2"
  local expected_run resource extra
  [[ -f "$resources_file" ]] || return 0
  while IFS=$'\t' read -r expected_run resource extra; do
    [[ -n "$expected_run" && -n "$resource" && -z "$extra" ]] || return 1
    remove_exact_resource "$kind" "$resource" "$expected_run" || return 1
  done <"$resources_file"
}

remove_secure_state_root() {
  local expected="/tmp/lez-atomic-swaps-m3-${run_id}-secure-state"
  [[ "$secure_state_root" == "$expected" ]] || return 1
  if [[ ! -e "$secure_state_root" && ! -L "$secure_state_root" ]]; then
    return 0
  fi
  [[ -d "$secure_state_root" && ! -L "$secure_state_root" ]] || return 1
  [[ "$(stat -c '%u' "$secure_state_root")" == "$(id -u)" ]] || return 1
  [[ "$(stat -c '%a' "$secure_state_root")" == 700 ]] || return 1
  rm -rf --one-file-system -- "$secure_state_root" || return 1
  [[ ! -e "$secure_state_root" && ! -L "$secure_state_root" ]]
}

write_cleanup_attestation() {
  local result="$1"
  local bitcoin_containers bitcoin_networks bitcoin_volumes bitcoin_images
  local lez_containers lez_networks lez_volumes lez_images secure_state_absent
  bitcoin_containers="$(docker container ls --all --quiet \
    --filter "label=org.logos-co.atomic-swaps.run=${bitcoin_run_id}" || true)"
  bitcoin_networks="$(docker network ls --quiet \
    --filter "label=org.logos-co.atomic-swaps.run=${bitcoin_run_id}" || true)"
  bitcoin_volumes="$(docker volume ls --quiet \
    --filter "label=org.logos-co.atomic-swaps.run=${bitcoin_run_id}" || true)"
  bitcoin_images="$(docker image ls --quiet \
    --filter "label=org.logos-co.atomic-swaps.run=${bitcoin_run_id}" || true)"
  lez_containers="$(docker container ls --all --quiet \
    --filter "label=org.logos-co.atomic-swaps.run=${lez_run_id}" || true)"
  lez_networks="$(docker network ls --quiet \
    --filter "label=org.logos-co.atomic-swaps.run=${lez_run_id}" || true)"
  lez_volumes="$(docker volume ls --quiet \
    --filter "label=org.logos-co.atomic-swaps.run=${lez_run_id}" || true)"
  lez_images="$(docker image ls --quiet \
    --filter "label=org.logos-co.atomic-swaps.run=${lez_run_id}" || true)"
  secure_state_absent=false
  if [[ ! -e "$secure_state_root" && ! -L "$secure_state_root" ]]; then
    secure_state_absent=true
  fi
  jq -n \
    --arg run_id "$run_id" \
    --arg journey "$journey" \
    --arg result "$result" \
    --arg bitcoin_run "$bitcoin_run_id" \
    --arg lez_run "$lez_run_id" \
    --arg bitcoin_containers "$bitcoin_containers" \
    --arg bitcoin_networks "$bitcoin_networks" \
    --arg bitcoin_volumes "$bitcoin_volumes" \
    --arg bitcoin_images "$bitcoin_images" \
    --arg lez_containers "$lez_containers" \
    --arg lez_networks "$lez_networks" \
    --arg lez_volumes "$lez_volumes" \
    --arg lez_images "$lez_images" \
    --argjson secure_state_absent "$secure_state_absent" '
    {
      schema_version: 1,
      journey: $journey,
      run_id: $run_id,
      result: $result,
      cleanup_scope: "captured_exact_ids_names_and_secure_state_root",
      broad_cleanup_used: false,
      child_runs: [$bitcoin_run, $lez_run],
      exact_run_resources_absent: {
        containers: ($bitcoin_containers == "" and $lez_containers == ""),
        networks: ($bitcoin_networks == "" and $lez_networks == ""),
        volumes: ($bitcoin_volumes == "" and $lez_volumes == ""),
        images: ($bitcoin_images == "" and $lez_images == ""),
        secure_reservation_state: $secure_state_absent
      },
      all_exact_run_resources_absent:
        ($bitcoin_containers == "" and $lez_containers == ""
         and $bitcoin_networks == "" and $lez_networks == ""
         and $bitcoin_volumes == "" and $lez_volumes == ""
         and $bitcoin_images == "" and $lez_images == ""
         and $secure_state_absent),
      foreign_resources_targeted: false
    }' >"${cleanup_attestation}.partial"
  chmod 0600 "${cleanup_attestation}.partial"
  mv "${cleanup_attestation}.partial" "$cleanup_attestation"
}

cleanup() {
  local run_status=$?
  local cleanup_failed=0
  local final_status="$run_status"
  trap - EXIT
  set +e

  stop_owned_processes || cleanup_failed=1
  remove_exact_container_file "$bitcoin_container_ids" "$bitcoin_run_id" || cleanup_failed=1
  remove_exact_container_file "$lez_container_ids" "$lez_run_id" || cleanup_failed=1

  remove_exact_resource_file volume "$volume_resources" || cleanup_failed=1
  remove_exact_resource_file network "$network_resources" || cleanup_failed=1
  remove_exact_resource_file image "$image_resources" || cleanup_failed=1
  remove_secure_state_root || cleanup_failed=1

  if [[ "$cleanup_failed" == "0" ]]; then
    write_cleanup_attestation passed || cleanup_failed=1
  else
    write_cleanup_attestation failed
  fi
  if [[ "$cleanup_failed" != "0" ]]; then
    echo "M3 actor local PoC could not prove exact owned-resource cleanup" >&2
    final_status=1
  fi
  exit "$final_status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
mkdir -m 0700 "$secure_state_root"
mkdir -m 0700 "$secure_state_root/directions"

verify_direction_driver_contract() {
  local contract driver_sha
  driver_sha="$(sha256sum "$direction_driver" | sed 's/ .*//')"
  [[ "$driver_sha" =~ ^[0-9a-f]{64}$ ]] || fail "direction-driver SHA-256 is invalid"
  contract="$(M3_POC_JOURNEY="$journey" "$direction_driver" contract)" ||
    fail "direction-driver contract is unavailable"
  jq -e --arg journey "$journey" '
    .schema_version == 1
    and .kind == "m3_actor_direction_driver_contract"
    and .stage_two_spec_uses_actual_node_facts == true
    and .fresh_actor_process_per_command == true
    and .separate_role_state_and_signing_journals == true
    and .taker_first_effects == true
    and .dual_locks_before_scalar_use == true
    and .bitcoin_exact_signed_depth == true
    and .lez_exact_finalized_ancestry == true
    and .journeys == ["claim", "refund", "first_lock_refund"]
    and .default_journey == "claim"
    and .actor_owned_claim_effects == true
    and .actor_owned_refund_effects == true
    and .actor_owned_first_lock_refund_effects == true
    and .first_lock_refund_terminal_revision == 2
    and .first_lock_refund_requires_signed_maker_cutoff == true
    and .first_lock_refund_requires_two_fresh_absence_and_unspent_reads == true
    and .first_lock_refund_owner_restart_never_resubmits == true
    and .first_lock_refund_fresh_maker_observer == true
    and .first_lock_refund_abandoned_maker_after_activation_until_finality == true
    and .first_lock_refund_taker_only_revision_one_and_refund_projection == true
    and .timeout_terminal_phase == "refunded"
    and (if $journey == "claim" then .actor_owned_claim_effects
         elif $journey == "refund" then .actor_owned_refund_effects
         else .actor_owned_first_lock_refund_effects end) == true
    and .actor_config_schema_version == 3
    and .role_shaped_bitcoin_refund_authority == true
    and .submission_count_query == true
    and .owned_process_registry == true
  ' <<<"$contract" >/dev/null || fail "direction-driver contract is incomplete"
  M3_POC_JOURNEY="$journey" "$direction_driver" preflight ||
    fail "direction runtime backend is not ready"
}

verify_lez_bootstrap_contract() {
  local contract
  contract="$($lez_bootstrap_driver contract)" || fail "LEZ bootstrap contract is unavailable"
  jq -e '
    .schema_version == 1
    and .kind == "m3_lez_bootstrap_contract"
    and .verified_artifact_target_required == true
    and .embedded_guest_sha256 == "a199c5be062adcb27cf63c62d9f5688b37058b4699ce7e1767fd26eeceb5e293"
    and .escrow_program_id == "39b6a4db85374de9359ea82164ef415019919475f656d597c5ab2231bc104dec"
    and .deployment_submission_count == 1
    and .fresh_identity_vault_claims == ["maker", "taker"]
    and .vault_claim_submission_count_per_role == 1
    and .public_rpc_used == false
    and .faucet_used == false
  ' <<<"$contract" >/dev/null || fail "LEZ bootstrap contract is incomplete"
}

prebuild() {
  echo "Prebuilding every M3 actor binary before service startup"
  if ! cargo +"$toolchain" build --locked --offline \
      -p btc-local-poc-provision -p btc-reference-actor -p lez-adaptor-role-runner --bins; then
    fail "offline M3 prebuild failed; populate the pinned Cargo cache before certification"
  fi
  if ! cargo +"$toolchain" build --locked --offline \
      -p lez-btc-swap-sdk --example btc-core-p2tr-fixture; then
    fail "offline BTC fixture prebuild failed; populate the pinned Cargo cache before certification"
  fi
  if ! cargo +"$toolchain" build --locked --offline \
      -p lez-bridge-client --example m3_witnessed_lez_operator; then
    fail "offline witnessed LEZ operator prebuild failed; populate the pinned Cargo cache"
  fi
  if ! cargo +"$toolchain" build --manifest-path compat/lez-v0_2-sidecar/Cargo.toml \
      --locked --offline --bin lez-v02-bridge-poc --bin lez-v02-vault-claim-poc \
      --bin lez-v02-native-escrow-poc \
      --example lez-v02-local-actor-identity --example lez-v02-account-id; then
    fail "offline LEZ sidecar prebuild failed; populate its pinned Cargo cache before certification"
  fi
}

readonly actor_bin="${repo_root}/target/debug/btc-reference-actor"
readonly provisioner_bin="${repo_root}/target/debug/btc-local-poc-provision"
readonly role_runner_bin="${repo_root}/target/debug/lez-adaptor-role-runner"
readonly core_fixture_bin="${repo_root}/target/debug/examples/btc-core-p2tr-fixture"
readonly lez_operator_bin="${repo_root}/target/debug/examples/m3_witnessed_lez_operator"
readonly sidecar_target="${repo_root}/compat/lez-v0_2-sidecar/target/debug"
readonly sidecar_bin="${sidecar_target}/lez-v02-bridge-poc"
readonly vault_claim_bin="${sidecar_target}/lez-v02-vault-claim-poc"
readonly native_escrow_bin="${sidecar_target}/lez-v02-native-escrow-poc"
readonly identity_bin="${sidecar_target}/examples/lez-v02-local-actor-identity"
readonly nssa_mapping_bin="${sidecar_target}/examples/lez-v02-account-id"

assert_prebuilt() {
  local binary
  for binary in "$actor_bin" "$provisioner_bin" "$role_runner_bin" \
    "$core_fixture_bin" "$lez_operator_bin" "$sidecar_bin" "$vault_claim_bin" \
    "$native_escrow_bin" "$identity_bin" "$nssa_mapping_bin" "$lez_deployer"; do
    [[ -x "$binary" && ! -L "$binary" ]] || fail "prebuilt binary is missing: ${binary}"
  done
}

provision_actor_identities() {
  "$identity_bin" --output-directory "${identities_dir}/maker" \
    >"${evidence_dir}/maker-lez-identity.json"
  "$identity_bin" --output-directory "${identities_dir}/taker" \
    >"${evidence_dir}/taker-lez-identity.json"
  chmod 0600 "${evidence_dir}/maker-lez-identity.json" \
    "${evidence_dir}/taker-lez-identity.json"
  jq -e '.schema == "lez-v0.2-local-actor-identity" and .version == 2
    and (.vault_account_id | test("^[1-9A-HJ-NP-Za-km-z]{32,64}$"))
    and (.vault_account_id_hex | test("^[0-9a-f]{64}$"))' \
    "${evidence_dir}/maker-lez-identity.json" >/dev/null
  jq -e '.schema == "lez-v0.2-local-actor-identity" and .version == 2
    and (.vault_account_id | test("^[1-9A-HJ-NP-Za-km-z]{32,64}$"))
    and (.vault_account_id_hex | test("^[0-9a-f]{64}$"))' \
    "${evidence_dir}/taker-lez-identity.json" >/dev/null
  [[ "$(jq -r '.account_id_hex' "${evidence_dir}/maker-lez-identity.json")" != \
      "$(jq -r '.account_id_hex' "${evidence_dir}/taker-lez-identity.json")" ]] ||
    fail "fresh maker and taker LEZ identities collided"
}

run_stage_one() {
  local direction="$1"
  local direction_root="${directions_dir}/${direction}"
  local planning_file="${direction_root}/planning.json"
  local fixture_root="${direction_root}/fixture"
  mkdir -m 0700 "$direction_root"
  jq -n \
    --arg maker "$(jq -er '.account_id_hex' "${evidence_dir}/maker-lez-identity.json")" \
    --arg taker "$(jq -er '.account_id_hex' "${evidence_dir}/taker-lez-identity.json")" '
    {
      schema_version: 1,
      maker_lez_owner_account: $maker,
      taker_lez_owner_account: $taker,
      refund_csv_blocks: 144
    }' >"$planning_file"
  chmod 0600 "$planning_file"
  "$provisioner_bin" generate --planning-file "$planning_file" --output-root "$fixture_root" \
    >"${evidence_dir}/${direction}-stage-one.json"
  chmod 0600 "${evidence_dir}/${direction}-stage-one.json"
  jq -e --arg root "$fixture_root" '
    .schema_version == 1
    and .public_spec_file == ($root + "/public-spec.json")
    and (.public_spec_sha256 | test("^[0-9a-f]{64}$"))
    and (.aggregate_internal_key | test("^[0-9a-f]{64}$"))
    and .lez_authority_helper.example == "lez-v02-account-id"
    and .private_material_disclosed == false
  ' "${evidence_dir}/${direction}-stage-one.json" >/dev/null
}

run_official_nssa_mapping() {
  local direction="$1"
  local summary="${evidence_dir}/${direction}-stage-one.json"
  local mapping="${evidence_dir}/${direction}-official-nssa-mapping.json"
  local argument
  argument="$(jq -er '.lez_authority_helper.argument' "$summary")"
  "$nssa_mapping_bin" "$argument" >"$mapping"
  chmod 0600 "$mapping"
  jq -e --arg argument "$argument" '
    .schema == "lez-v0.2-nssa-account-id"
    and .version == 1
    and .x_only_public_key == $argument
    and (.account_id | test("^[0-9a-f]{64}$"))
  ' "$mapping" >/dev/null
}

capture_owned_containers() {
  local child_run="$1"
  local expected_count="$2"
  local output="$3"
  local -a ids=()
  local container_id
  mapfile -t ids < <(docker container ls --all --quiet \
    --filter "label=org.logos-co.atomic-swaps.run=${child_run}")
  [[ "${#ids[@]}" == "$expected_count" ]] ||
    fail "child run ${child_run} has ${#ids[@]} containers; expected ${expected_count}"
  : >"$output"
  chmod 0600 "$output"
  for container_id in "${ids[@]}"; do
    assert_exact_owned_resource container "$container_id" "$child_run" ||
      fail "container ${container_id} is not owned by ${child_run}"
    printf '%s\n' "$container_id" >>"$output"
  done
}

start_actual_nodes() {
  local maker_account maker_vault taker_account taker_vault
  maker_account="$(jq -er '.account_id' "${evidence_dir}/maker-lez-identity.json")"
  maker_vault="$(jq -er '.vault_account_id' "${evidence_dir}/maker-lez-identity.json")"
  taker_account="$(jq -er '.account_id' "${evidence_dir}/taker-lez-identity.json")"
  taker_vault="$(jq -er '.vault_account_id' "${evidence_dir}/taker-lez-identity.json")"

  RUN_ID="$bitcoin_run_id" BITCOIN_CORE_E2E_MODE=service \
    BITCOIN_CORE_E2E_KEEP_RUNNING=1 ./scripts/run-bitcoin-core-e2e.sh \
    >"${evidence_dir}/bitcoin-service.log" 2>&1
  capture_owned_containers "$bitcoin_run_id" 1 "$bitcoin_container_ids"
  capture_owned_resources network "$bitcoin_run_id" 1 "$network_resources"
  capture_owned_resources volume "$bitcoin_run_id" 1 "$volume_resources"
  capture_owned_resources image "$bitcoin_run_id" 1 "$image_resources"

  RUN_ID="$lez_run_id" LEZ_V02_KEEP_RUNNING=1 \
    LEZ_V02_SLOT_DURATION_SECONDS="$lez_slot_duration_seconds" \
    LEZ_V02_MAKER_ACCOUNT_ID="$maker_account" LEZ_V02_MAKER_VAULT_ACCOUNT_ID="$maker_vault" \
    LEZ_V02_TAKER_ACCOUNT_ID="$taker_account" LEZ_V02_TAKER_VAULT_ACCOUNT_ID="$taker_vault" \
    ./scripts/run-lez-v02-stack.sh >"${evidence_dir}/lez-service.log" 2>&1
  capture_owned_containers "$lez_run_id" 3 "$lez_container_ids"
  capture_owned_resources network "$lez_run_id" 1 "$network_resources"
  capture_owned_resources volume "$lez_run_id" 0 "$volume_resources"
  capture_owned_resources image "$lez_run_id" 1 "$image_resources"

  [[ -f "$bitcoin_manifest" && -f "$lez_manifest" ]] ||
    fail "retained node manifests are unavailable"
  [[ "$(manifest_value "$lez_manifest" LEZ_V02_SLOT_DURATION_SECONDS)" == "$lez_slot_duration_seconds" ]] ||
    fail "LEZ child manifest does not attest the journey-selected slot duration"
  chmod 0600 "${evidence_dir}/bitcoin-service.log" "${evidence_dir}/lez-service.log"
}

bootstrap_lez_runtime() {
  M3_POC_RUN_ID="$run_id" \
  M3_POC_EVIDENCE_DIR="$evidence_dir" \
  M3_POC_LEZ_BOOTSTRAP_ROOT="$lez_bootstrap_root" \
  M3_POC_LEZ_BOOTSTRAP_MANIFEST="$lez_bootstrap_manifest" \
  M3_POC_LEZ_MANIFEST="$lez_manifest" \
  M3_POC_LEZ_SEQUENCER_RPC_URL="$(manifest_value "$lez_manifest" LEZ_SEQUENCER_RPC_URL)" \
  M3_POC_LEZ_INDEXER_RPC_URL="$(manifest_value "$lez_manifest" LEZ_INDEXER_RPC_URL)" \
  M3_POC_LEZ_CHANNEL_ID="$(manifest_value "$lez_manifest" LEZ_V02_CHANNEL_PUBLIC_KEY)" \
  M3_POC_MAKER_LEZ_PRIVATE_KEY="${identities_dir}/maker/lez-signer.key" \
  M3_POC_TAKER_LEZ_PRIVATE_KEY="${identities_dir}/taker/lez-signer.key" \
  M3_POC_VAULT_CLAIM_BIN="$vault_claim_bin" \
  "$lez_bootstrap_driver" execute
  [[ -f "$lez_bootstrap_manifest" && ! -L "$lez_bootstrap_manifest" ]] ||
    fail "LEZ bootstrap manifest is unavailable"
}

manifest_value() {
  local manifest="$1"
  local key="$2"
  local -a values=()
  mapfile -t values < <(sed -n "s/^${key}=//p" "$manifest")
  [[ "${#values[@]}" == 1 && -n "${values[0]}" ]] ||
    fail "manifest ${manifest} does not contain exactly one ${key}"
  printf '%s\n' "${values[0]}"
}

with_direction_environment() {
  local direction="$1"
  shift
  local direction_root="${directions_dir}/${direction}"
  M3_POC_RUN_ID="$run_id" \
  M3_POC_JOURNEY="$journey" \
  M3_POC_DIRECTION="$direction" \
  M3_POC_DIRECTION_ROOT="$direction_root" \
  M3_POC_SECURE_STATE_ROOT="${secure_state_root}/directions/${direction}" \
  M3_POC_EVIDENCE_DIR="$evidence_dir" \
  M3_POC_PROCESS_REGISTRY="$process_registry" \
  M3_POC_ACTOR_BIN="$actor_bin" \
  M3_POC_PROVISIONER_BIN="$provisioner_bin" \
  M3_POC_ROLE_RUNNER_BIN="$role_runner_bin" \
  M3_POC_CORE_FIXTURE_BIN="$core_fixture_bin" \
  M3_POC_LEZ_SIDECAR_BIN="$sidecar_bin" \
  M3_POC_LEZ_OPERATOR_BIN="$lez_operator_bin" \
  M3_POC_LEZ_NATIVE_ESCROW_BIN="$native_escrow_bin" \
  M3_POC_BITCOIN_MANIFEST="$bitcoin_manifest" \
  M3_POC_BITCOIN_RPC_URL="$(manifest_value "$bitcoin_manifest" BITCOIN_CORE_RPC_URL)" \
  M3_POC_BITCOIN_MAKER_CURL_CONFIG="$(manifest_value "$bitcoin_manifest" BITCOIN_CORE_MAKER_CURL_CONFIG)" \
  M3_POC_BITCOIN_TAKER_CURL_CONFIG="$(manifest_value "$bitcoin_manifest" BITCOIN_CORE_TAKER_CURL_CONFIG)" \
  M3_POC_BITCOIN_MAKER_BASIC="$(manifest_value "$bitcoin_manifest" BITCOIN_CORE_MAKER_BASIC_CREDENTIALS)" \
  M3_POC_BITCOIN_TAKER_BASIC="$(manifest_value "$bitcoin_manifest" BITCOIN_CORE_TAKER_BASIC_CREDENTIALS)" \
  M3_POC_BITCOIN_FUNDING_CREDENTIALS="$(manifest_value "$bitcoin_manifest" BITCOIN_CORE_FUNDING_CREDENTIALS)" \
  M3_POC_BITCOIN_CONTAINER_ID="$(sed -n '1p' "$bitcoin_container_ids")" \
  M3_POC_LEZ_MANIFEST="$lez_manifest" \
  M3_POC_LEZ_SEQUENCER_RPC_URL="$(manifest_value "$lez_manifest" LEZ_SEQUENCER_RPC_URL)" \
  M3_POC_LEZ_INDEXER_RPC_URL="$(manifest_value "$lez_manifest" LEZ_INDEXER_RPC_URL)" \
  M3_POC_LEZ_CHANNEL_ID="$(manifest_value "$lez_manifest" LEZ_V02_CHANNEL_PUBLIC_KEY)" \
  M3_POC_LEZ_BOOTSTRAP_MANIFEST="$lez_bootstrap_manifest" \
  M3_POC_LEZ_ESCROW_PROGRAM_ID="$(manifest_value "$lez_bootstrap_manifest" M3_POC_LEZ_ESCROW_PROGRAM_ID)" \
  M3_POC_LEZ_AUTH_TRANSFER_PROGRAM_ID="$(manifest_value "$lez_bootstrap_manifest" M3_POC_LEZ_AUTH_TRANSFER_PROGRAM_ID)" \
  M3_POC_LEZ_GENESIS_BLOCK_HASH="$(manifest_value "$lez_bootstrap_manifest" M3_POC_LEZ_GENESIS_BLOCK_HASH)" \
  M3_POC_MAKER_LEZ_IDENTITY="${identities_dir}/maker/identity.json" \
  M3_POC_MAKER_LEZ_PRIVATE_KEY="${identities_dir}/maker/lez-signer.key" \
  M3_POC_TAKER_LEZ_IDENTITY="${identities_dir}/taker/identity.json" \
  M3_POC_TAKER_LEZ_PRIVATE_KEY="${identities_dir}/taker/lez-signer.key" \
  "$@"
}

run_stage_two() {
  local direction="$1"
  local direction_root="${directions_dir}/${direction}"
  local fixture_root="${direction_root}/fixture"
  local spec_file="${direction_root}/stage-two.json"
  with_direction_environment "$direction" \
    "$direction_driver" prepare-stage-two-spec "$spec_file"
  [[ -f "$spec_file" && ! -L "$spec_file" ]] || fail "stage-two spec is unavailable"
  [[ "$(stat -c '%a' "$spec_file")" == 600 ]] || fail "stage-two spec is not owner-private"
  "$provisioner_bin" finalize --spec-file "$spec_file" --output-root "$fixture_root" \
    >"${evidence_dir}/${direction}-stage-two.json"
  chmod 0600 "${evidence_dir}/${direction}-stage-two.json"
  jq -e --arg direction "$direction" '
    .schema_version == 1
    and .direction == $direction
    and .agreement_revalidated == true
    and .private_material_disclosed == false
    and (.agreement_sha256 | test("^[0-9a-f]{64}$"))
  ' "${evidence_dir}/${direction}-stage-two.json" >/dev/null
}

run_direction_actor_flow() {
  local direction="$1"
  with_direction_environment "$direction" "$direction_driver" run-actor-flow
}

assert_actor_terminal_status() {
  local status_file="$1"
  local role="$2"
  jq -e --arg role "$role" --arg terminal_phase "$terminal_phase" \
    --argjson terminal_revision "$terminal_revision" '
    .schema_version == 1
    and .role == $role
    and .state == "active"
    and .phase == $terminal_phase
    and .revision == $terminal_revision
    and .next_action == "complete"
  ' "$status_file" >/dev/null
}

assert_terminal_and_replay() {
  local direction="$1"
  local direction_root="${directions_dir}/${direction}"
  local counts_before="${evidence_dir}/${direction}-submission-counts-before-replay.json"
  local counts_after="${evidence_dir}/${direction}-submission-counts-after-replay.json"
  local role config terminal replay after expected_bitcoin expected_lez

  with_direction_environment "$direction" "$direction_driver" submission-counts >"$counts_before"
  chmod 0600 "$counts_before"
  expected_bitcoin="$(jq -er '.expected_unique_effects.bitcoin | numbers' \
    "${evidence_dir}/${direction}-actual-effects.json")"
  expected_lez="$(jq -er '.expected_unique_effects.lez | numbers' \
    "${evidence_dir}/${direction}-actual-effects.json")"
  jq -e --argjson bitcoin "$expected_bitcoin" --argjson lez "$expected_lez" '
    .schema_version == 1 and .bitcoin == $bitcoin and .lez == $lez
  ' "$counts_before" >/dev/null

  for role in maker taker; do
    config="${direction_root}/actors/${role}/actor-config.json"
    terminal="${evidence_dir}/${direction}-${role}-terminal.json"
    replay="${evidence_dir}/${direction}-${role}-replay-${replay_command}.json"
    after="${evidence_dir}/${direction}-${role}-after-replay.json"
    [[ -f "$config" && ! -L "$config" ]] || fail "${role} actor config is unavailable"
    "$actor_bin" --config "$config" status >"$terminal"
    assert_actor_terminal_status "$terminal" "$role"
    "$actor_bin" --config "$config" "$replay_command" >"$replay"
    jq -e --arg role "$role" --arg replay_command "$replay_command" \
      --arg terminal_phase "$terminal_phase" \
      --argjson terminal_revision "$terminal_revision" '
      .schema_version == 1
      and .role == $role
      and .command == $replay_command
      and .outcome == "not_yet_composed"
      and .durable_revision == $terminal_revision
      and .phase == $terminal_phase
      and .revision == $terminal_revision
    ' "$replay" >/dev/null
    "$actor_bin" --config "$config" status >"$after"
    assert_actor_terminal_status "$after" "$role"
    chmod 0600 "$terminal" "$replay" "$after"
  done

  with_direction_environment "$direction" "$direction_driver" submission-counts >"$counts_after"
  chmod 0600 "$counts_after"
  [[ "$(jq -S -c . "$counts_before")" == "$(jq -S -c . "$counts_after")" ]] ||
    fail "fresh-process terminal replay resubmitted a public effect"
}

validate_actual_effect_manifests() {
  local direction manifest
  for direction in "${directions[@]}"; do
    manifest="${evidence_dir}/${direction}-actual-effects.json"
    [[ -f "$manifest" && ! -L "$manifest" ]] ||
      fail "${direction} actual-effect manifest is unavailable"
    jq -e --arg direction "$direction" --arg journey "$journey" '
      .schema_version == 1
      and .direction == $direction
      and .journey == $journey
      and .expected_unique_effects ==
        (if $journey == "first_lock_refund" and $direction == "taker_sells_foreign"
         then {bitcoin:2,lez:0}
         elif $journey == "first_lock_refund"
         then {bitcoin:0,lez:3}
         else {bitcoin:2,lez:3}
         end)
      and (.bitcoin_effect_ids | type) == "array"
      and (.bitcoin_effect_ids | length) == .expected_unique_effects.bitcoin
      and (.bitcoin_effect_ids | unique | length) == .expected_unique_effects.bitcoin
      and (.lez_effect_ids | type) == "array"
      and (.lez_effect_ids | length) == .expected_unique_effects.lez
      and (.lez_effect_ids | unique | length) == .expected_unique_effects.lez
      and all(.bitcoin_effect_ids[]; type == "string" and test("^[0-9a-f]{64}$"))
      and all(.lez_effect_ids[]; type == "string" and test("^[0-9a-f]{64}$"))
      and if $journey == "claim" then
        .actor_owned_claims == {
          bitcoin:.bitcoin_effect_ids[1], lez:.lez_effect_ids[2]
        }
        and (has("actor_owned_refunds") | not)
      elif $journey == "refund" then
        .actor_owned_refunds == {
          bitcoin:.bitcoin_effect_ids[1], lez:.lez_effect_ids[2]
        }
        and .cooperative_claim_effects_present == false
        and (has("actor_owned_claims") | not)
      elif $direction == "taker_sells_foreign" then
        .actor_owned_refunds == {bitcoin:.bitcoin_effect_ids[1]}
        and .maker_second_lock == {chain:"lez",effect_count:0}
        and .cooperative_claim_effects_present == false
        and .dual_lock_gate_opened == false
        and (has("actor_owned_claims") | not)
      else
        .actor_owned_refunds == {lez:.lez_effect_ids[2]}
        and .maker_second_lock == {chain:"bitcoin",effect_count:0}
        and .cooperative_claim_effects_present == false
        and .dual_lock_gate_opened == false
        and (has("actor_owned_claims") | not)
      end
    ' "$manifest" >/dev/null ||
      fail "${direction} actual-effect manifest does not match the ${journey} journey"
  done
}

write_run_evidence() {
  local repository_commit completed_at outer_runner_sha direction_driver_sha lez_bootstrap_sha
  repository_commit="$(git rev-parse HEAD)"
  completed_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  outer_runner_sha="$(sha256sum scripts/run-m3-actor-local-poc.sh | sed 's/ .*//')"
  direction_driver_sha="$(sha256sum "$direction_driver" | sed 's/ .*//')"
  lez_bootstrap_sha="$(sha256sum "$lez_bootstrap_driver" | sed 's/ .*//')"
  jq -n \
    --arg run_id "$run_id" \
    --arg journey "$journey" \
    --arg packet_kind "$packet_kind" \
    --argjson terminal_revision "$terminal_revision" \
    --arg terminal_phase "$terminal_phase" \
    --arg replay_command "$replay_command" \
    --arg actor_owned_effect_semantics "$actor_owned_effect_semantics" \
    --arg repository_commit "$repository_commit" \
    --arg completed_at "$completed_at" \
    --arg outer_runner "scripts/run-m3-actor-local-poc.sh" \
    --arg outer_runner_sha "$outer_runner_sha" \
    --arg direction_driver "scripts/run-m3-actor-direction.sh" \
    --arg direction_driver_sha "$direction_driver_sha" \
    --arg lez_bootstrap "scripts/run-m3-lez-bootstrap.sh" \
    --arg lez_bootstrap_sha "$lez_bootstrap_sha" \
    --arg rapidsnark_dir "$RAPIDSNARK_LIB_DIR" \
    --arg rapidsnark_sha "$rapidsnark_sha" --arg gmp_sha "$gmp_sha" \
    --arg fq_sha "$fq_sha" --arg fr_sha "$fr_sha" \
    --arg bindgen_extra_clang_args "$BINDGEN_EXTRA_CLANG_ARGS" \
    --arg bitcoin_run "$bitcoin_run_id" \
    --arg lez_run "$lez_run_id" \
    --arg lez_slot_duration_seconds "$lez_slot_duration_seconds" \
    --arg foreign_stage2_sha "$(sha256sum "${evidence_dir}/taker_sells_foreign-stage-two.json" | sed 's/ .*//')" \
    --arg lez_stage2_sha "$(sha256sum "${evidence_dir}/taker_sells_lez-stage-two.json" | sed 's/ .*//')" '
    {
      schema_version: 1,
      kind: $packet_kind,
      journey: $journey,
      result: "passed",
      run_id: $run_id,
      repository_commit: $repository_commit,
      completed_at: $completed_at,
      certified_executable_scripts: {
        outer_runner: {repository_path:$outer_runner,sha256:$outer_runner_sha},
        direction_driver: {repository_path:$direction_driver,sha256:$direction_driver_sha},
        lez_bootstrap: {repository_path:$lez_bootstrap,sha256:$lez_bootstrap_sha},
        external_override_allowed: false
      },
      native_build_prerequisites: {
        rapidsnark_lib_dir: $rapidsnark_dir,
        files: {
          "librapidsnark.a": $rapidsnark_sha,
          "libgmp.a": $gmp_sha,
          "libfq.a": $fq_sha,
          "libfr.a": $fr_sha
        },
        bindgen_extra_clang_args: $bindgen_extra_clang_args,
        verified_before_offline_build: true
      },
      services: {
        bitcoin_core: {run_id: $bitcoin_run, version: "31.1", network: "regtest"},
        lez: {run_id: $lez_run, version: "v0.2.0", network: "private_local",
              slot_duration_seconds:$lez_slot_duration_seconds}
      },
      directions: [
        {direction: "taker_sells_foreign", terminal_revision: $terminal_revision,
         terminal_phase: $terminal_phase,
         expected_unique_effects:
           (if $journey == "first_lock_refund" then {bitcoin:2,lez:0}
            else {bitcoin:2,lez:3} end),
         maker_second_lock_effect_count:
           (if $journey == "first_lock_refund" then 0 else 1 end),
         stage_two_evidence_sha256: $foreign_stage2_sha},
        {direction: "taker_sells_lez", terminal_revision: $terminal_revision,
         terminal_phase: $terminal_phase,
         expected_unique_effects:
           (if $journey == "first_lock_refund" then {bitcoin:0,lez:3}
            else {bitcoin:2,lez:3} end),
         maker_second_lock_effect_count:
           (if $journey == "first_lock_refund" then 0 else 1 end),
         stage_two_evidence_sha256: $lez_stage2_sha}
      ],
      actor_process_model: "fresh_one_shot_process_per_command",
      actor_owned_effect_semantics: $actor_owned_effect_semantics,
      first_lock_refund_admission:
        (if $journey == "first_lock_refund" then {
          signed_cutoff:true,two_fresh_cross_chain_reads:true,
          lez_absence_window_reaches_current_finalized_tip:true,
          bitcoin_cutoff_clock:"stable_core_median_time",
          actor_internal_gate_authoritative:true,
          owner_restart_without_resubmission:true,
          fresh_maker_observer:true,
          maker_offline_after_activation_until_refund_finality:true,
          taker_only_revision_one_and_refund_projection:true
        } else null end),
      expected_unique_effects_by_direction:
        (if $journey == "first_lock_refund" then
           {taker_sells_foreign:{bitcoin:2,lez:0},
            taker_sells_lez:{bitcoin:0,lez:3}}
         else
           {taker_sells_foreign:{bitcoin:2,lez:3},
            taker_sells_lez:{bitcoin:2,lez:3}}
         end),
      replay_command: $replay_command,
      replay_resubmission_count: 0,
      prerequisite_cache: {
        cargo_offline_required: true,
        network_dependency_during_certification: false,
        cold_cache_is_a_setup_prerequisite_not_a_runtime_rpc: true
      },
      public_rpc_used: false,
      faucet_used: false,
      public_funds_used: false,
      private_material_disclosed: false
    }' >"${run_evidence}.partial"
  chmod 0600 "${run_evidence}.partial"
  mv "${run_evidence}.partial" "$run_evidence"
  jq -e --arg journey "$journey" --arg packet_kind "$packet_kind" \
    --argjson terminal_revision "$terminal_revision" \
    --arg terminal_phase "$terminal_phase" --arg replay_command "$replay_command" \
    --arg actor_owned_effect_semantics "$actor_owned_effect_semantics" '
    .schema_version == 1
    and .kind == $packet_kind
    and .journey == $journey
    and .result == "passed"
    and (.directions | length == 2)
    and all(.directions[];
      .terminal_revision == $terminal_revision and .terminal_phase == $terminal_phase)
    and .actor_owned_effect_semantics == $actor_owned_effect_semantics
    and .expected_unique_effects_by_direction ==
      (if $journey == "first_lock_refund" then
         {taker_sells_foreign:{bitcoin:2,lez:0},
          taker_sells_lez:{bitcoin:0,lez:3}}
       else
         {taker_sells_foreign:{bitcoin:2,lez:3},
          taker_sells_lez:{bitcoin:2,lez:3}}
       end)
    and (if $journey == "first_lock_refund" then
      all(.directions[]; .maker_second_lock_effect_count == 0)
      and .first_lock_refund_admission == {
        signed_cutoff:true,two_fresh_cross_chain_reads:true,
        lez_absence_window_reaches_current_finalized_tip:true,
        bitcoin_cutoff_clock:"stable_core_median_time",
        actor_internal_gate_authoritative:true,
        owner_restart_without_resubmission:true,
        fresh_maker_observer:true,
        maker_offline_after_activation_until_refund_finality:true,
        taker_only_revision_one_and_refund_projection:true
      }
    else true end)
    and .replay_command == $replay_command
    and .replay_resubmission_count == 0
  ' "$run_evidence" >/dev/null || fail "final journey evidence packet is inconsistent"
}

verify_direction_driver_contract
verify_lez_bootstrap_contract
prebuild
assert_prebuilt
provision_actor_identities

# Both agreements receive independent stage-one private material before any
# node fact or endpoint exists. The exact pinned official NSSA mapping is then
# recorded before either agreement can be finalized.
for direction in "${directions[@]}"; do
  run_stage_one "$direction"
  run_official_nssa_mapping "$direction"
done

start_actual_nodes
bootstrap_lez_runtime

# Directions share only the actual local nodes. The driver contract requires
# fresh sidecars, actor stores, role journals, and one-shot processes per
# direction; the second direction cannot begin until the first proves replay.
for direction in "${directions[@]}"; do
  run_stage_two "$direction"
  run_direction_actor_flow "$direction"
  assert_terminal_and_replay "$direction"
done

validate_actual_effect_manifests
write_run_evidence
echo "${success_label} passed: ${run_evidence}"
