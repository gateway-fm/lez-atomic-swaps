#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

export LC_ALL=C
umask 077

readonly mode="${M3_ACTOR_POC_MODE:-execute}"
asset_mode="${M3_ACTOR_POC_ASSET_MODE:-native}"
if [[ "$asset_mode" != "native" && "$asset_mode" != "custom_token" ]]; then
  echo "M3_ACTOR_POC_ASSET_MODE must be native or custom_token" >&2
  exit 2
fi
schedule="${M3_ACTOR_POC_SCHEDULE:-sequential}"
if [[ "$schedule" != "sequential" && "$schedule" != "overlap" ]]; then
  echo "M3_ACTOR_POC_SCHEDULE must be sequential or overlap" >&2
  exit 2
fi
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
if [[ "$journey" != "claim" && "$journey" != "survivor_claim" &&
      "$journey" != "refund" &&
      "$journey" != "first_lock_refund" ]]; then
  echo "M3_ACTOR_POC_JOURNEY must be claim, survivor_claim, refund, or first_lock_refund" >&2
  exit 2
fi
if [[ "$schedule" == "overlap" && "$journey" != "claim" ]]; then
  echo "M3_ACTOR_POC_SCHEDULE=overlap currently requires the claim journey" >&2
  exit 2
fi
if [[ "$asset_mode" == "custom_token" && "$journey" != "claim" ]]; then
  echo "M3_ACTOR_POC_ASSET_MODE=custom_token currently requires the claim journey" >&2
  exit 2
fi
if [[ "$asset_mode" == "custom_token" && "$schedule" != "sequential" ]]; then
  echo "M3_ACTOR_POC_ASSET_MODE=custom_token currently requires the sequential schedule" >&2
  exit 2
fi
case "$journey" in
  claim)
    terminal_revision=4
    terminal_phase="completed"
    replay_command="drive"
    if [[ "$asset_mode" == "custom_token" ]]; then
      packet_kind="m3_actor_two_direction_custom_token_local_poc"
      success_label="M3 actor two-direction custom-token local PoC"
    elif [[ "$schedule" == "overlap" ]]; then
      packet_kind="m3_actor_overlapping_two_swap_local_poc"
      success_label="M3 actor overlapping two-swap local PoC"
    else
      packet_kind="m3_actor_two_direction_local_poc"
      success_label="M3 actor two-direction local PoC"
    fi
    actor_owned_effect_semantics="claim"
    if [[ "$asset_mode" == "custom_token" ]]; then
      # Four-effect token proof performs independent stable finalized scans.
      # A slower run-owned devnet slot gives those fail-closed brackets a
      # deterministic quiet interval without changing protocol semantics.
      lez_slot_duration_seconds="10.0"
    else
      lez_slot_duration_seconds="1.0"
    fi
    ;;
  survivor_claim)
    terminal_revision=4
    terminal_phase="completed"
    replay_command="drive"
    packet_kind="m3_actor_two_direction_survivor_claim_local_poc"
    actor_owned_effect_semantics="survivor_claim"
    success_label="M3 actor two-direction survivor-claim local PoC"
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

readonly asset_mode schedule journey terminal_revision terminal_phase replay_command packet_kind
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
readonly f7_token_fixture_root="${private_dir}/f7-token-fixture"
readonly f7_token_fixture_evidence="${f7_token_fixture_root}/evidence/f7-token-fixture.json"
readonly f7_token_fixture_private="${f7_token_fixture_root}/private"
readonly bitcoin_manifest="${repo_root}/.e2e/${bitcoin_run_id}/bitcoin-core/run.env"
readonly bitcoin_funding_sources="${private_dir}/bitcoin-funding-sources.json"
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
readonly f7_token_fixture_driver="${repo_root}/scripts/run-m3-f7-token-fixture.sh"
readonly lez_source_dir="${LEZ_V02_SOURCE_DIR:-/tmp/lez-v020-native-investigation}"
readonly official_wallet_target="${secure_state_root}/official-wallet-target"
readonly official_wallet_bin="${official_wallet_target}/debug/wallet"
readonly lez_v02_source_commit="a58fbce2ff48c58b7bb5001b1a27e64b9596ee3a"
readonly lez_token_program_id="c5d50f88bfe7cb14b421673e9441aade7571e522eef035cc24d80b2e53c69a7c"
readonly lez_ata_program_id="95841cc8bd2c87d7111bc5c7f3aa2a85d35e90f7217e82a397aa05acd51500f8"
readonly official_token_id_declaration='pub const TOKEN_ID: [u32; 8] = [2282739141, 348907455, 1046946228, 3735699860, 585462133, 3426087150, 772528164, 2090518099];'
readonly official_ata_id_declaration='pub const ASSOCIATED_TOKEN_ACCOUNT_ID: [u32; 8] = [3357312149, 3615960253, 3351583505, 2234166003, 4153433811, 2743238177, 2886052503, 4160755157];'
readonly -a directions=(taker_sells_foreign taker_sells_lez)
declare -A overlap_pids=()
declare -A overlap_logs=()
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
    --arg schedule "$schedule" \
    --arg asset_mode "$asset_mode" \
    --arg token_program_id "$lez_token_program_id" \
    --arg ata_program_id "$lez_ata_program_id" \
    --argjson terminal_revision "$terminal_revision" \
    --arg terminal_phase "$terminal_phase" \
    --arg replay_command "$replay_command" \
    --arg packet_kind "$packet_kind" \
    --arg actor_owned_effect_semantics "$actor_owned_effect_semantics" '
    {
      schema_version: 1,
      kind: "m3_actor_local_poc_contract",
      execution_performed: false,
      asset_mode: $asset_mode,
      journey: $journey,
      schedule: $schedule,
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
        directions_are_sequential: ($schedule == "sequential"),
        overlapping_revision_two_barrier: ($schedule == "overlap"),
        settlements_released_only_after_both_locks: ($schedule == "overlap"),
        official_token_fixture_after_bootstrap_before_stage_two:
          ($asset_mode == "custom_token")
      },
      survivor:
        (if $journey == "survivor_claim" then {
          revealer:"taker",follower_role:"maker",
          revealer_absent_after_reveal_until_follower_terminal:true,
          fresh_follower_observes_revision_three:true,
          intermediate_phase:"claim_evidence_available",
          intermediate_lifecycle_disposition:"recovering",
          intermediate_terminal:false,
          remaining_leg_must_be_canonical_and_claimable:true,
          follower_restart_before_followup:true,
          delayed_revealer_catchup_observation_only:true
        } else null end),
      finality: {
        bitcoin: "exact_signed_confirmation_depth",
        lez: "exact_finalized_indexer_ancestry"
      },
      effect_semantics: {
        actor_owned: $actor_owned_effect_semantics,
        expected_unique_effects_by_direction:
          (if $asset_mode == "custom_token" then
             {taker_sells_foreign:{bitcoin:2,lez:4},
              taker_sells_lez:{bitcoin:2,lez:4}}
           elif $journey == "first_lock_refund" then
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
      asset:
        (if $asset_mode == "custom_token" then {
          custom_token:{fixture:"official_lez_v0_2_wallet",
            provisioned_once_after_bootstrap:true,
            directions_use_distinct_definitions_and_depositors:true,
            evidence_path_passed_to_direction:true,
            private_path_passed_to_direction:true,
            token_program_id:$token_program_id,
            ata_program_id:$ata_program_id}
        } else {native:{base_agreement_terms:true}} end),
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
        official_wallet_build_target:
          (if $asset_mode == "custom_token" then
             "exact_run_owned_secure_state_root" else null end),
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
        executable_script_sha256s:
          (["outer_runner", "direction_driver", "lez_bootstrap"] +
           (if $asset_mode == "custom_token" then ["f7_token_fixture"] else [] end))
      },
      build_prerequisites: {
        rapidsnark_lib_dir: "explicit_absolute_canonical_verified_v0_0_8",
        rapidsnark_files: ["librapidsnark.a","libgmp.a","libfq.a","libfr.a"],
        bindgen_extra_clang_args: "explicit_nonempty",
        inherited_by_offline_sidecar_build: true,
        official_wallet:
          (if $asset_mode == "custom_token" then {
            source:"same_exact_clean_pinned_lez_v0_2_checkout",
            cargo:"locked_offline",target:"exact_run_owned_secure_state_root"
          } else null end)
      },
      external_resources: {
        public_rpc: false,
        faucet: false,
        public_funds: false,
        test_funds:
          (if $asset_mode == "custom_token" then
             "deterministic_local_genesis_regtest_and_official_tokens"
           else "deterministic_local_genesis_and_regtest_outputs" end),
        bedrock_ntp: {
          endpoint: "pool.ntp.org:123/udp",
          attempted_by_pinned_component: true,
          required_for_certification: false
        }
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

for command_name in cargo chmod curl date docker find git id jq kill mkdir mv readlink rg rm sed sha256sum sleep stat; do
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
if [[ "$asset_mode" == "custom_token" ]]; then
  [[ -x "$f7_token_fixture_driver" && ! -L "$f7_token_fixture_driver" ]] ||
    fail "M3 F7 token-fixture driver is missing or unsafe"
  [[ "$(readlink -f "$f7_token_fixture_driver")" == "$f7_token_fixture_driver" ]] ||
    fail "M3 F7 token-fixture driver path is not canonical"
fi
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

validate_official_wallet_source() {
  [[ "$lez_source_dir" == /* && -d "$lez_source_dir/.git" && ! -L "$lez_source_dir" &&
     "$(readlink -f "$lez_source_dir")" == "$lez_source_dir" ]] ||
    fail "LEZ v0.2 source must be one canonical absolute Git checkout"
  [[ -z "$(git -C "$lez_source_dir" status --porcelain --untracked-files=all)" ]] ||
    fail "LEZ v0.2 source checkout is dirty"
  [[ "$(git -C "$lez_source_dir" rev-parse HEAD)" == "$lez_v02_source_commit" ]] ||
    fail "LEZ v0.2 source checkout is not the pinned commit"
  [[ "$(git -C "$lez_source_dir" rev-parse 'refs/tags/v0.2.0^{}')" == \
     "$lez_v02_source_commit" ]] || fail "LEZ v0.2 source tag does not match the pinned commit"
}

if [[ "$asset_mode" == "custom_token" ]]; then
  validate_official_wallet_source
fi

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
  contract="$(M3_POC_ASSET_MODE="$asset_mode" M3_POC_JOURNEY="$journey" \
    "$direction_driver" contract)" ||
    fail "direction-driver contract is unavailable"
  jq -e --arg journey "$journey" --arg asset_mode "$asset_mode" '
    .schema_version == 1
    and .kind == "m3_actor_direction_driver_contract"
    and .stage_two_spec_uses_actual_node_facts == true
    and .fresh_actor_process_per_command == true
    and .separate_role_state_and_signing_journals == true
    and .taker_first_effects == true
    and .dual_locks_before_scalar_use == true
    and .bitcoin_exact_signed_depth == true
    and .lez_exact_finalized_ancestry == true
    and .actor_owned_maker_lock_effects == true
    and .taker_first_lock_external_runner_submission == true
    and .maker_lock_submission_actor_output == "awaiting_observation"
    and .maker_lock_restart_never_resubmits == true
    and .runner_only_confirms_actor_submitted_maker_locks == true
    and .journeys == ["claim", "survivor_claim", "refund", "first_lock_refund"]
    and .default_journey == "claim"
    and .actor_owned_claim_effects == true
    and .actor_owned_survivor_claim_effects == true
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
         elif $journey == "survivor_claim" then .actor_owned_survivor_claim_effects
         elif $journey == "refund" then .actor_owned_refund_effects
         else .actor_owned_first_lock_refund_effects end) == true
    and .asset_mode == $asset_mode
    and .actor_config_schema_version ==
      (if $asset_mode == "custom_token" then 5 else 4 end)
    and .asset_extension_required == ($asset_mode == "custom_token")
    and .official_token_ata_derivation_required == ($asset_mode == "custom_token")
    and .expected_unique_effects ==
      {bitcoin:2,lez:(if $asset_mode == "custom_token" then 4 else 3 end)}
    and .asset_first_lock_order ==
      (if $asset_mode == "custom_token" then
        ["initialize_witnessed","create_custody_ata","fund"] else [] end)
    and .role_shaped_bitcoin_refund_authority == true
    and .submission_count_query == true
    and .owned_process_registry == true
  ' <<<"$contract" >/dev/null || fail "direction-driver contract is incomplete"
  M3_POC_ASSET_MODE="$asset_mode" M3_POC_JOURNEY="$journey" \
    "$direction_driver" preflight ||
    fail "direction runtime backend is not ready"
}

verify_lez_bootstrap_contract() {
  local contract
  contract="$($lez_bootstrap_driver contract)" || fail "LEZ bootstrap contract is unavailable"
  jq -e '
    .schema_version == 1
    and .kind == "m3_lez_bootstrap_contract"
    and .verified_artifact_target_required == true
    and .embedded_guest_sha256 == "bc2ea18eaacb917727934fcf0366dd54c1f9a2b69b61ea53080c926850967fd7"
    and .escrow_program_id == "f3ead24b95d316ce91980cb3531a70b83a27fd1640f47c1b857757aef26c244e"
    and .deployment_submission_count == 1
    and .fresh_identity_vault_claims == ["maker", "taker"]
    and .vault_claim_submission_count_per_role == 1
    and .public_rpc_used == false
    and .faucet_used == false
  ' <<<"$contract" >/dev/null || fail "LEZ bootstrap contract is incomplete"
}

prebuild() {
  local registry
  local -a program_registries=()
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
  if [[ "$asset_mode" == "custom_token" ]]; then
    if ! cargo +"$toolchain" build --manifest-path compat/lez-v0_2-sidecar/Cargo.toml \
        --locked --offline --example lez-v02-account-codec; then
      fail "offline LEZ account-codec prebuild failed; populate the pinned Cargo cache"
    fi
    mkdir -m 0700 "$official_wallet_target"
    if ! cargo +"$toolchain" build --manifest-path "${lez_source_dir}/Cargo.toml" \
        --locked --offline -p wallet --target-dir "$official_wallet_target"; then
      fail "offline official LEZ v0.2 wallet build failed; populate the pinned Cargo cache"
    fi
    mapfile -t program_registries < <(find "${official_wallet_target}/debug/build" \
      -path '*/out/lez/programs/mod.rs' -type f -print)
    [[ "${#program_registries[@]}" -ge 1 ]] ||
      fail "official wallet build did not retain its generated program registry"
    for registry in "${program_registries[@]}"; do
      [[ ! -L "$registry" ]] || fail "official program registry became a symlink"
      rg -Fqx "$official_token_id_declaration" "$registry" ||
        fail "official Token program ID differs from the verified v0.2 value"
      rg -Fqx "$official_ata_id_declaration" "$registry" ||
        fail "official ATA program ID differs from the verified v0.2 value"
    done
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
readonly account_codec_bin="${sidecar_target}/examples/lez-v02-account-codec"

assert_prebuilt() {
  local binary
  for binary in "$actor_bin" "$provisioner_bin" "$role_runner_bin" \
    "$core_fixture_bin" "$lez_operator_bin" "$sidecar_bin" "$vault_claim_bin" \
    "$native_escrow_bin" "$identity_bin" "$nssa_mapping_bin" "$lez_deployer"; do
    [[ -x "$binary" && ! -L "$binary" ]] || fail "prebuilt binary is missing: ${binary}"
  done
  if [[ "$asset_mode" == "custom_token" ]]; then
    [[ -x "$account_codec_bin" && ! -L "$account_codec_bin" ]] ||
      fail "prebuilt LEZ account codec is missing"
    [[ -x "$official_wallet_bin" && -f "$official_wallet_bin" &&
       ! -L "$official_wallet_bin" ]] || fail "official LEZ v0.2 wallet binary is missing"
    [[ "$(readlink -f "$official_wallet_target")" == "$official_wallet_target" ]] ||
      fail "official wallet target is not canonical"
  fi
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

  RUN_ID="$lez_run_id" LEZ_V02_KEEP_RUNNING=1 LEZ_V02_SOURCE_DIR="$lez_source_dir" \
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

core_admin() {
  local container_id actual_run
  container_id="$(sed -n '1p' "$bitcoin_container_ids")"
  [[ -n "$container_id" ]] || fail "Bitcoin container inventory is empty"
  actual_run="$(docker container inspect --format \
    '{{ index .Config.Labels "org.logos-co.atomic-swaps.run" }}' "$container_id")"
  [[ "$actual_run" == "$bitcoin_run_id" ]] ||
    fail "captured Bitcoin container ownership label drifted"
  docker exec "$container_id" bitcoin-cli \
    -conf=/run-config/bitcoin.conf -datadir=/var/lib/bitcoin "$@"
}

provision_bitcoin_funding_sources() {
  local address before_height after_height mined block_hash block txid vout utxo
  local direction height source_file mempool
  local -a source_files=()
  address="$(manifest_value \
    "$(manifest_value "$bitcoin_manifest" BITCOIN_CORE_FUNDING_CREDENTIALS)" \
    BITCOIN_CORE_FUNDING_ADDRESS)"
  before_height="$(core_admin getblockcount)"
  [[ "$before_height" == 101 ]] ||
    fail "Bitcoin funding-source allocation requires the clean service tip at height 101"
  mempool="$(core_admin getrawmempool)"
  jq -e 'type == "array" and length == 0' <<<"$mempool" >/dev/null ||
    fail "Bitcoin funding-source allocation requires an empty mempool"

  mined="$(core_admin generatetoaddress 1 "$address")"
  jq -e 'type == "array" and length == 1 and
    (.[0] | test("^[0-9a-f]{64}$"))' <<<"$mined" >/dev/null ||
    fail "Bitcoin funding-source maturity extension did not mine exactly one block"
  block_hash="$(jq -er '.[0]' <<<"$mined")"
  after_height="$(core_admin getblockcount)"
  [[ "$after_height" == 102 ]] ||
    fail "Bitcoin funding-source maturity extension did not reach height 102"
  block="$(core_admin getblock "$block_hash" 2)"
  jq -e --arg hash "$block_hash" --argjson previous "$before_height" '
    .hash == $hash and .height == ($previous + 1)
    and (.tx | length) == 1 and .tx[0].vin[0].coinbase != null
  ' <<<"$block" >/dev/null ||
    fail "Bitcoin maturity extension was not an exact coinbase-only child block"

  for direction in "${directions[@]}"; do
    case "$direction" in
      taker_sells_foreign) height=1 ;;
      taker_sells_lez) height=2 ;;
    esac
    block_hash="$(core_admin getblockhash "$height")"
    block="$(core_admin getblock "$block_hash" 2)"
    txid="$(jq -er '.tx[0].txid' <<<"$block")"
    vout="$(jq -er --arg address "$address" '
      [.tx[0].vout[] | select(.scriptPubKey.address == $address) | .n]
      | if length == 1 then .[0] else error("funding output") end
    ' <<<"$block")"
    utxo="$(core_admin gettxout "$txid" "$vout" true)"
    jq -e --arg address "$address" '
      .value == 50 and .confirmations >= 101 and .coinbase == true
      and .scriptPubKey.type == "witness_v1_taproot"
      and .scriptPubKey.address == $address
      and .scriptPubKey.hex ==
        "512079be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
    ' <<<"$utxo" >/dev/null ||
      fail "${direction} Bitcoin funding source is not an exact mature local coinbase UTXO"
    source_file="${private_dir}/${direction}-bitcoin-funding-source.json"
    jq -n --arg direction "$direction" --arg txid "$txid" \
      --argjson vout "$vout" --arg script "$(jq -er '.scriptPubKey.hex' <<<"$utxo")" \
      --arg block_hash "$block_hash" --argjson block_height "$height" \
      --argjson confirmations "$(jq -er '.confirmations | numbers' <<<"$utxo")" \
      --argjson planned_anchor "$((after_height + height))" '
      {direction:$direction,source:{transaction_id:$txid,output_index:$vout,
       value_sat:5000000000,script_pubkey:$script,coinbase:true,
       containing_block_hash:$block_hash,containing_block_height:$block_height,
       confirmations:$confirmations},planned_bitcoin_funding_anchor_height:$planned_anchor}
    ' >"$source_file"
    chmod 0600 "$source_file"
    source_files+=("$source_file")
  done

  jq -s --argjson base_height "$after_height" \
    '{schema_version:1,network:"regtest",allocation:"two_distinct_mature_coinbase_outpoints",
      shared_fixture_custody_key:true,base_height:$base_height,sources:.}' \
    "${source_files[@]}" >"${bitcoin_funding_sources}.partial"
  chmod 0600 "${bitcoin_funding_sources}.partial"
  mv "${bitcoin_funding_sources}.partial" "$bitcoin_funding_sources"
  jq -e '
    .schema_version == 1 and .network == "regtest" and .base_height == 102
    and (.sources | length) == 2
    and ([.sources[].direction] | unique | length) == 2
    and ([.sources[].source.transaction_id] | unique | length) == 2
    and ([.sources[] | [.source.transaction_id,.source.output_index]] | unique | length) == 2
    and [.sources[].planned_bitcoin_funding_anchor_height] == [103,104]
    and all(.sources[]; .source.confirmations >= 101)
  ' "$bitcoin_funding_sources" >/dev/null ||
    fail "independent Bitcoin funding-source manifest is inconsistent"
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

provision_f7_token_fixture() {
  [[ "$asset_mode" == "custom_token" ]] || return 0
  M3_F7_TOKEN_FIXTURE_RUN_ID="$run_id" \
  M3_F7_TOKEN_FIXTURE_OUTPUT_ROOT="$f7_token_fixture_root" \
  M3_F7_TOKEN_FIXTURE_WALLET_BIN="$official_wallet_bin" \
  M3_F7_TOKEN_FIXTURE_SOURCE_DIR="$lez_source_dir" \
  M3_F7_TOKEN_FIXTURE_LEZ_MANIFEST="$lez_manifest" \
  M3_F7_TOKEN_FIXTURE_MAKER_IDENTITY="${identities_dir}/maker/identity.json" \
  M3_F7_TOKEN_FIXTURE_MAKER_KEY="${identities_dir}/maker/lez-signer.key" \
  M3_F7_TOKEN_FIXTURE_TAKER_IDENTITY="${identities_dir}/taker/identity.json" \
  M3_F7_TOKEN_FIXTURE_TAKER_KEY="${identities_dir}/taker/lez-signer.key" \
  "$f7_token_fixture_driver"
  [[ -f "$f7_token_fixture_evidence" && ! -L "$f7_token_fixture_evidence" &&
     -d "$f7_token_fixture_private" && ! -L "$f7_token_fixture_private" ]] ||
    fail "official F7 token fixture did not retain its evidence and private state"
  [[ "$(stat -c '%a' "$f7_token_fixture_evidence")" == 600 &&
     "$(stat -c '%a' "$f7_token_fixture_private")" == 700 ]] ||
    fail "official F7 token fixture paths are not owner-private"
  jq -e --arg commit "$lez_v02_source_commit" '
    .schema_version == 1 and .kind == "m3_f7_official_token_fixture"
    and .result == "passed" and .upstream.source_commit == $commit
    and (.transactions | length) == 8
    and .assets.M3F7A.depositor == "maker"
    and .assets.M3F7B.depositor == "taker"
    and .external_resources.public_rpc == false
    and .external_resources.faucet == false
    and .external_resources.public_funds == false
  ' "$f7_token_fixture_evidence" >/dev/null ||
    fail "official F7 token fixture evidence is inconsistent"
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

direction_command() {
  if [[ "$asset_mode" == "custom_token" ]]; then
    M3_POC_ASSET_MODE="$asset_mode" \
    M3_POC_LEZ_ACCOUNT_CODEC_BIN="$account_codec_bin" \
    M3_POC_F7_FIXTURE_ROOT="$f7_token_fixture_root" \
    M3_POC_F7_FIXTURE_EVIDENCE="$f7_token_fixture_evidence" \
    M3_POC_F7_FIXTURE_PRIVATE_DIR="$f7_token_fixture_private" \
    M3_POC_F7_WALLET_BIN="$official_wallet_bin" \
    M3_POC_F7_TOKEN_PROGRAM_ID="$lez_token_program_id" \
    M3_POC_F7_ATA_PROGRAM_ID="$lez_ata_program_id" \
    "$@"
  else
    M3_POC_ASSET_MODE="$asset_mode" "$@"
  fi
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
  M3_POC_BITCOIN_FUNDING_SOURCES="$bitcoin_funding_sources" \
  M3_POC_BITCOIN_PLANNED_ANCHOR_HEIGHT="$(jq -er --arg direction "$direction" \
    '.sources[] | select(.direction == $direction) |
     .planned_bitcoin_funding_anchor_height' "$bitcoin_funding_sources")" \
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
  direction_command "$@"
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

register_overlap_driver_process() {
  local direction="$1" pid="$2" start executable
  for _ in {1..100}; do
    if [[ -r "/proc/${pid}/stat" ]]; then
      start="$(awk '{print $22}' "/proc/${pid}/stat")"
      executable="$(readlink -f "/proc/${pid}/exe")"
      if [[ -n "$start" && "$executable" == /* ]]; then break; fi
    fi
    sleep 0.01
  done
  [[ -n "${start:-}" && "${executable:-}" == /* ]] ||
    fail "overlap ${direction} driver exited before registration"
  jq -nc --arg role "controller-${direction}" --arg phase overlap-driver \
    --argjson pid "$pid" --arg start "$start" --arg executable "$executable" \
    '{role:$role,phase:$phase,pid:$pid,start_ticks:$start,executable:$executable}' \
    >>"$process_registry"
}

start_overlap_direction() {
  local direction="$1" log pid
  log="${evidence_dir}/${direction}-overlap-driver.log"
  [[ ! -e "$log" && ! -L "$log" ]] ||
    fail "refusing to overwrite overlap driver log"
  with_direction_environment "$direction" "$direction_driver" \
    run-overlap-actor-flow >"$log" 2>&1 &
  pid=$!
  chmod 0600 "$log"
  overlap_pids["$direction"]="$pid"
  overlap_logs["$direction"]="$log"
  register_overlap_driver_process "$direction" "$pid"
}

wait_overlap_arrival() {
  local direction="$1" phase="$2" revision="$3" marker pid
  marker="${directions_dir}/${direction}/overlap-${phase}-arrived.json"
  pid="${overlap_pids[$direction]:-}"
  [[ "$pid" =~ ^[1-9][0-9]*$ ]] || fail "overlap ${direction} driver PID is unavailable"
  for _ in {1..14400}; do
    if [[ -f "$marker" && ! -L "$marker" ]]; then
      [[ "$(stat -c '%a' "$marker")" == 600 ]] ||
        fail "overlap ${direction} ${phase} marker is not owner private"
      jq -e --arg run "$run_id" --arg direction "$direction" --arg phase "$phase" \
        --argjson revision "$revision" '
        .schema_version == 1 and .run_id == $run and .direction == $direction
        and .phase == $phase and .revision == $revision
        and (.recorded_at | test("^[0-9]{4}-[0-9]{2}-[0-9]{2}T"))
      ' "$marker" >/dev/null || fail "overlap ${direction} ${phase} marker is inconsistent"
      return
    fi
    kill -0 "$pid" 2>/dev/null ||
      fail "overlap ${direction} driver exited before ${phase}: ${overlap_logs[$direction]}"
    sleep 0.25
  done
  fail "overlap ${direction} ${phase} arrival timed out"
}

release_overlap_phase() {
  local direction="$1" phase="$2" revision="$3" permit partial
  permit="${directions_dir}/${direction}/overlap-${phase}-permit.json"
  partial="${permit}.partial"
  [[ ! -e "$permit" && ! -L "$permit" && ! -e "$partial" && ! -L "$partial" ]] ||
    fail "refusing to overwrite overlap ${direction} ${phase} permit"
  jq -n --arg run "$run_id" --arg direction "$direction" --arg phase "$phase" \
    --argjson revision "$revision" '
    {schema_version:1,run_id:$run,direction:$direction,phase:$phase,
     expected_revision:$revision}
  ' >"$partial"
  chmod 0600 "$partial"
  mv "$partial" "$permit"
}

wait_overlap_driver_exit() {
  local direction="$1" pid
  pid="${overlap_pids[$direction]}"
  if ! wait "$pid"; then
    fail "overlap ${direction} driver failed: ${overlap_logs[$direction]}"
  fi
}

assert_overlap_revision_two_window() {
  local direction role status config
  local inventory="${evidence_dir}/overlap-isolation-inventory.ndjson"
  : >"$inventory"
  chmod 0600 "$inventory"
  for direction in "${directions[@]}"; do
    for role in maker taker; do
      status="${evidence_dir}/${direction}-overlap-locked-status-${role}.json"
      config="${directions_dir}/${direction}/actors/${role}/actor-config.json"
      jq -e --arg role "$role" '
        .schema_version == 1 and .role == $role and .state == "active"
        and .revision == 2 and .phase == "both_legs_locked"
        and .next_action == "observe_revealing_claim"
      ' "$status" >/dev/null ||
        fail "${direction} ${role} was not simultaneously in flight at revision two"
      jq -nc --arg direction "$direction" --arg role "$role" \
        --arg config "$config" \
        --arg state_db "$(jq -er '.state_db' "$config")" \
        --arg btc_journal "$(jq -er '.signing.bitcoin.journal_db' "$config")" \
        --arg lez_journal "$(jq -er '.signing.lez.journal_db' "$config")" \
        --arg btc_session "$(jq -er '.signing.bitcoin.session_id' "$config")" \
        --arg lez_session "$(jq -er '.signing.lez.session_id' "$config")" \
        --argjson state_inode "$(stat -c '%i' "$(jq -er '.state_db' "$config")")" \
        --argjson btc_inode "$(stat -c '%i' "$(jq -er '.signing.bitcoin.journal_db' "$config")")" \
        --argjson lez_inode "$(stat -c '%i' "$(jq -er '.signing.lez.journal_db' "$config")")" '
        {direction:$direction,role:$role,config:$config,state_db:$state_db,
         state_inode:$state_inode,btc_signing_journal:$btc_journal,
         btc_signing_inode:$btc_inode,lez_signing_journal:$lez_journal,
         lez_signing_inode:$lez_inode,btc_session_id:$btc_session,
         lez_session_id:$lez_session}
      ' >>"$inventory"
    done
  done
  jq -s --slurpfile funding "$bitcoin_funding_sources" \
    --slurpfile foreign "${directions_dir}/taker_sells_foreign/stage-two.json" \
    --slurpfile lez "${directions_dir}/taker_sells_lez/stage-two.json" \
    --slurpfile foreign_summary "${evidence_dir}/taker_sells_foreign-stage-two.json" \
    --slurpfile lez_summary "${evidence_dir}/taker_sells_lez-stage-two.json" '
    {schema_version:1,kind:"m3_overlapping_revision_two_window",
     simultaneous_in_flight:true,revision:2,phase:"both_legs_locked",
     chain_mutations_serialized_for_exact_observation:true,
     funding_sources:$funding[0],actors:.,agreements:[
       {direction:"taker_sells_foreign",swap_id:$foreign[0].swap_id,
        agreement_sha256:$foreign_summary[0].agreement_sha256,
        bitcoin_funding_transaction_id:$foreign[0].bitcoin.funding_transaction_id,
        metadata_account:$foreign[0].lez_terms.metadata_account,
        custody_account:$foreign[0].lez_terms.custody_account,
        refund_at_ms:$foreign[0].lez_terms.refund_at_ms,
        bitcoin_refund_height:$foreign[0].recovery.bitcoin_refund_height},
       {direction:"taker_sells_lez",swap_id:$lez[0].swap_id,
        agreement_sha256:$lez_summary[0].agreement_sha256,
        bitcoin_funding_transaction_id:$lez[0].bitcoin.funding_transaction_id,
        metadata_account:$lez[0].lez_terms.metadata_account,
        custody_account:$lez[0].lez_terms.custody_account,
        refund_at_ms:$lez[0].lez_terms.refund_at_ms,
        bitcoin_refund_height:$lez[0].recovery.bitcoin_refund_height}]}
  ' "$inventory" >"${evidence_dir}/overlap-revision-two-window.json"
  chmod 0600 "${evidence_dir}/overlap-revision-two-window.json"
  jq -e '
    .simultaneous_in_flight == true and (.actors | length) == 4
    and ([.actors[].state_db] | unique | length) == 4
    and ([.actors[].state_inode] | unique | length) == 4
    and ([.actors[].btc_signing_journal] | unique | length) == 4
    and ([.actors[].lez_signing_journal] | unique | length) == 4
    and ([.actors[] | .btc_session_id] | unique | length) == 2
    and ([.actors[] | .lez_session_id] | unique | length) == 2
    and ([.agreements[].swap_id] | unique | length) == 2
    and ([.agreements[].agreement_sha256] | unique | length) == 2
    and ([.agreements[].bitcoin_funding_transaction_id] | unique | length) == 2
    and ([.agreements[].metadata_account] | unique | length) == 2
    and ([.agreements[].custody_account] | unique | length) == 2
    and ([.agreements[].refund_at_ms] | unique | length) == 2
    and ([.agreements[].bitcoin_refund_height] | unique | length) == 2
  ' "${evidence_dir}/overlap-revision-two-window.json" >/dev/null ||
    fail "overlap revision-two isolation packet is inconsistent"
}

run_overlapping_actor_flows() {
  local direction
  for direction in "${directions[@]}"; do run_stage_two "$direction"; done
  for direction in "${directions[@]}"; do
    start_overlap_direction "$direction"
    wait_overlap_arrival "$direction" ready 0
  done
  for direction in "${directions[@]}"; do
    release_overlap_phase "$direction" lock 0
    wait_overlap_arrival "$direction" locked 2
  done
  assert_overlap_revision_two_window
  for direction in "${directions[@]}"; do
    release_overlap_phase "$direction" settle 2
    wait_overlap_arrival "$direction" terminal 4
    wait_overlap_driver_exit "$direction"
    assert_terminal_and_replay "$direction"
  done
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
    jq -e --arg direction "$direction" --arg journey "$journey" \
      --arg asset_mode "$asset_mode" '
      .schema_version == 1
      and .direction == $direction
      and .journey == $journey
      and .expected_unique_effects ==
        (if $asset_mode == "custom_token" then {bitcoin:2,lez:4}
         elif $journey == "first_lock_refund" and $direction == "taker_sells_foreign"
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
      and if ($journey == "claim" or $journey == "survivor_claim") then
        .actor_owned_claims == {
          bitcoin:.bitcoin_effect_ids[1],
          lez:.lez_effect_ids[(if $asset_mode == "custom_token" then 3 else 2 end)]
        }
        and (has("actor_owned_refunds") | not)
        and (if $journey == "survivor_claim" then
          .survivor_evidence_file == ($direction + "-survivor-claim.json")
          and .revealer == "taker" and .follower == "maker"
          and .intermediate_phase == "claim_evidence_available"
          and .intermediate_terminal == false
        else (has("survivor_evidence_file") | not) end)
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
  jq -e --slurpfile foreign "${evidence_dir}/taker_sells_foreign-actual-effects.json" \
    --slurpfile lez "${evidence_dir}/taker_sells_lez-actual-effects.json" '
    (($foreign[0].bitcoin_effect_ids + $lez[0].bitcoin_effect_ids) as $ids |
      ($ids | unique | length) == ($ids | length))
    and (($foreign[0].lez_effect_ids + $lez[0].lez_effect_ids) as $ids |
      ($ids | unique | length) == ($ids | length))
  ' <<<null >/dev/null || fail "direction effect IDs overlap across independent swaps"
}

validate_survivor_direction_evidence() {
  local direction="$1" reveal_chain="$2" followup_chain="$3"
  local completion="${evidence_dir}/${direction}-survivor-claim.json"
  local recovering="${evidence_dir}/${direction}-survivor-recovering.json"
  local reveal_output="${evidence_dir}/${direction}-survivor-delayed-taker-reveal-taker.json"
  local followup_output="${evidence_dir}/${direction}-survivor-delayed-taker-followup-taker.json"
  local bitcoin_before="${evidence_dir}/${direction}-survivor-catchup-bitcoin-mempool-before.json"
  local bitcoin_after="${evidence_dir}/${direction}-survivor-catchup-bitcoin-mempool-after.json"
  local maker_observation="${evidence_dir}/${direction}-survivor-maker-observe-reveal-maker.json"
  local maker_revision_three="${evidence_dir}/${direction}-survivor-maker-revision-three-status-maker.json"
  local followup_submit="${evidence_dir}/${direction}-survivor-${followup_chain}-followup-submit-maker.json"
  local terminal_projection="${evidence_dir}/${direction}-survivor-maker-project-followup-maker.json"
  local maker_terminal="${evidence_dir}/${direction}-survivor-maker-terminal-status-maker.json"
  local file completion_sha recovering_sha reveal_output_sha followup_output_sha
  local bitcoin_before_sha bitcoin_after_sha maker_observation_sha
  local maker_revision_three_sha followup_submit_sha terminal_projection_sha maker_terminal_sha
  for file in "$completion" "$recovering" "$reveal_output" "$followup_output" \
    "$bitcoin_before" "$bitcoin_after" "$maker_observation" "$maker_revision_three" \
    "$followup_submit" "$terminal_projection" "$maker_terminal"; do
    [[ -f "$file" && ! -L "$file" ]] ||
      fail "${direction} survivor evidence input is unavailable: ${file##*/}"
  done
  completion_sha="$(sha256sum "$completion" | sed 's/ .*//')"
  recovering_sha="$(sha256sum "$recovering" | sed 's/ .*//')"
  reveal_output_sha="$(sha256sum "$reveal_output" | sed 's/ .*//')"
  followup_output_sha="$(sha256sum "$followup_output" | sed 's/ .*//')"
  bitcoin_before_sha="$(sha256sum "$bitcoin_before" | sed 's/ .*//')"
  bitcoin_after_sha="$(sha256sum "$bitcoin_after" | sed 's/ .*//')"
  maker_observation_sha="$(sha256sum "$maker_observation" | sed 's/ .*//')"
  maker_revision_three_sha="$(sha256sum "$maker_revision_three" | sed 's/ .*//')"
  followup_submit_sha="$(sha256sum "$followup_submit" | sed 's/ .*//')"
  terminal_projection_sha="$(sha256sum "$terminal_projection" | sed 's/ .*//')"
  maker_terminal_sha="$(sha256sum "$maker_terminal" | sed 's/ .*//')"

  jq -e --arg direction "$direction" --arg reveal_chain "$reveal_chain" \
    --arg followup_chain "$followup_chain" --arg recovering_sha "$recovering_sha" \
    --arg reveal_output_sha "$reveal_output_sha" --arg followup_output_sha "$followup_output_sha" \
    --arg bitcoin_before_sha "$bitcoin_before_sha" --arg bitcoin_after_sha "$bitcoin_after_sha" \
    --arg maker_observation_sha "$maker_observation_sha" \
    --arg maker_revision_three_sha "$maker_revision_three_sha" \
    --arg followup_submit_sha "$followup_submit_sha" \
    --arg terminal_projection_sha "$terminal_projection_sha" \
    --arg maker_terminal_sha "$maker_terminal_sha" \
    --slurpfile recovering "$recovering" --slurpfile reveal_output "$reveal_output" \
    --slurpfile followup_output "$followup_output" \
    --slurpfile bitcoin_before "$bitcoin_before" --slurpfile bitcoin_after "$bitcoin_after" \
    --slurpfile maker_observation "$maker_observation" \
    --slurpfile maker_revision_three "$maker_revision_three" \
    --slurpfile followup_submit "$followup_submit" \
    --slurpfile terminal_projection "$terminal_projection" \
    --slurpfile maker_terminal "$maker_terminal" '
    .schema_version == 1 and .journey == "survivor_claim" and .direction == $direction
    and .reveal.role == "taker" and .reveal.chain == $reveal_chain
    and (.reveal.transaction_id | test("^[0-9a-f]{64}$")) and .reveal.canonical == true
    and .continuation.follower_role == "maker"
    and .continuation.canonical_reveal_observed_by_fresh_process == true
    and .continuation.caller_supplied_secret == false
    and .continuation.related_presignature_and_adaptor_point_validated == true
    and .continuation.projected_revision == 3
    and .intermediate.protocol_phase == "claim_evidence_available"
    and .intermediate.lifecycle_disposition == "recovering"
    and .intermediate.terminal == false
    and .intermediate.recovering_evidence_sha256 == $recovering_sha
    and .availability.taker_invocations_after_reveal_before_maker_terminal == 0
    and .availability.taker_absence_guard_enforced == true
    and .availability.follower_process_exited_at_revision_three == true
    and .availability.fresh_follower_process_submitted_followup == true
    and .availability.distinct_fresh_follower_process_projected_terminal == true
    and .availability.followup_submission_output_sha256 == $followup_submit_sha
    and .availability.terminal_projection_output_sha256 == $terminal_projection_sha
    and .completion.followup_role == "maker" and .completion.chain == $followup_chain
    and (.completion.transaction_id | test("^[0-9a-f]{64}$"))
    and .completion.canonical == true and .completion.maker_revision == 4
    and .completion.phase == "completed"
    and .completion.maker_terminal_status_sha256 == $maker_terminal_sha
    and .completion.boundary.chain == $followup_chain
    and .completion.boundary.completed_before_signed_refund_boundary == true
    and .delayed_revealer_catchup.began_after_maker_terminal == true
    and .delayed_revealer_catchup.revisions == [3,4]
    and .delayed_revealer_catchup.observation_only == true
    and .delayed_revealer_catchup.actor_observations.reveal == {
      chain:$reveal_chain,revision:3,outcome:"observed_then_projected",sha256:$reveal_output_sha}
    and .delayed_revealer_catchup.actor_observations.followup == {
      chain:$followup_chain,revision:4,outcome:"observed_then_projected",sha256:$followup_output_sha}
    and .delayed_revealer_catchup.per_chain.bitcoin.bitcoin_mempool_before_sha256 == $bitcoin_before_sha
    and .delayed_revealer_catchup.per_chain.bitcoin.bitcoin_mempool_after_sha256 == $bitcoin_after_sha
    and .delayed_revealer_catchup.per_chain.bitcoin.mempool_before_count == 0
    and .delayed_revealer_catchup.per_chain.bitcoin.mempool_after_count == 0
    and .delayed_revealer_catchup.per_chain.bitcoin.successful_resubmission_count == 0
    and .delayed_revealer_catchup.per_chain.lez.durable_submission_count_before == 3
    and .delayed_revealer_catchup.per_chain.lez.durable_submission_count_after == 3
    and .delayed_revealer_catchup.per_chain.lez.successful_resubmission_count == 0
    and .delayed_revealer_catchup.successful_resubmission_count == 0
    and .secret_recorded == false and .delivery_or_chat_used == false
    and $recovering[0].schema_version == 1
    and $recovering[0].journey == "survivor_claim"
    and $recovering[0].direction == $direction
    and $recovering[0].reveal.role == "taker"
    and $recovering[0].reveal.chain == $reveal_chain
    and $recovering[0].reveal.transaction_id == .reveal.transaction_id
    and $recovering[0].reveal.canonical == true
    and $recovering[0].continuation.follower_role == "maker"
    and $recovering[0].continuation.canonical_reveal_observed_by_fresh_process == true
    and $recovering[0].continuation.caller_supplied_secret == false
    and $recovering[0].continuation.related_presignature_and_adaptor_point_validated == true
    and $recovering[0].continuation.projected_revision == 3
    and $recovering[0].intermediate.protocol_phase == "claim_evidence_available"
    and $recovering[0].intermediate.lifecycle_disposition == "recovering"
    and $recovering[0].intermediate.terminal == false
    and $recovering[0].intermediate.remaining_leg.chain == $followup_chain
    and $recovering[0].intermediate.remaining_leg.canonical == true
    and $recovering[0].intermediate.remaining_leg.unspent_or_funded == true
    and $recovering[0].intermediate.remaining_leg.before_signed_later_refund_boundary == true
    and $recovering[0].intermediate.followup_effect_present == false
    and $recovering[0].availability.taker_invocations_after_reveal_before_maker_terminal == 0
    and $recovering[0].availability.taker_absence_guard_enforced == true
    and $recovering[0].availability.follower_process_exited_at_revision_three == true
    and $recovering[0].process_evidence.maker_reveal_observation_sha256 == $maker_observation_sha
    and $recovering[0].process_evidence.maker_revision_three_status_sha256 == $maker_revision_three_sha
    and $recovering[0].secret_recorded == false
    and $recovering[0].delivery_or_chat_used == false
    and $reveal_output[0].role == "taker" and $reveal_output[0].revision == 3
    and $reveal_output[0].phase == "claim_evidence_available"
    and $reveal_output[0].outcome == "observed_then_projected"
    and $followup_output[0].role == "taker" and $followup_output[0].revision == 4
    and $followup_output[0].phase == "completed"
    and $followup_output[0].outcome == "observed_then_projected"
    and $bitcoin_before[0].error == null and $bitcoin_before[0].result == []
    and $bitcoin_after[0].error == null and $bitcoin_after[0].result == []
    and $maker_observation[0].role == "maker" and $maker_observation[0].revision == 3
    and $maker_observation[0].phase == "claim_evidence_available"
    and $maker_observation[0].outcome == "observed_then_projected"
    and $maker_revision_three[0].role == "maker" and $maker_revision_three[0].revision == 3
    and $maker_revision_three[0].phase == "claim_evidence_available"
    and $maker_revision_three[0].next_action == "observe_followup_claim"
    and $followup_submit[0].role == "maker" and $followup_submit[0].revision == 3
    and $followup_submit[0].phase == "claim_evidence_available"
    and $followup_submit[0].chain == $followup_chain
    and $followup_submit[0].outcome == "awaiting_observation"
    and $terminal_projection[0].role == "maker" and $terminal_projection[0].revision == 4
    and $terminal_projection[0].phase == "completed"
    and $terminal_projection[0].chain == $followup_chain
    and $terminal_projection[0].outcome == "observed_then_projected"
    and $maker_terminal[0].role == "maker" and $maker_terminal[0].revision == 4
    and $maker_terminal[0].phase == "completed" and $maker_terminal[0].next_action == "complete"
    and (if $direction == "taker_sells_foreign" then
      $recovering[0].intermediate.bitcoin_effect_count == 1
      and $recovering[0].intermediate.lez_effect_count == 3
      and .completion.boundary.chain == "bitcoin"
      and (.completion.boundary.confirmed_tip_height | numbers) <
        (.completion.boundary.signed_refund_height | numbers)
    else
      $recovering[0].intermediate.bitcoin_effect_count == 2
      and $recovering[0].intermediate.lez_effect_count == 2
      and $recovering[0].intermediate.remaining_leg.metadata_status == "funded"
      and $recovering[0].intermediate.remaining_leg.custody_balance ==
        $recovering[0].intermediate.remaining_leg.amount
      and .completion.boundary.chain == "lez"
      and (.completion.boundary.finalized_containing_block_timestamp_ms | numbers) <
        (.completion.boundary.signed_refund_at_ms | numbers)
      and (.completion.boundary.finality_evidence_sha256 | test("^[0-9a-f]{64}$"))
      and (.completion.boundary.containing_block_evidence_sha256 | test("^[0-9a-f]{64}$"))
    end)
  ' "$completion" >/dev/null || fail "${direction} survivor direction evidence is inconsistent"

  jq -c --arg completion_sha "$completion_sha" --arg recovering_sha "$recovering_sha" \
    --slurpfile recovering "$recovering" '
    {direction:.direction,revealer:.reveal.role,follower_role:.continuation.follower_role,
     protected_absence:{
       starts_after_reveal_submission:(.reveal.canonical and .availability.taker_absence_guard_enforced),
       ends_after_follower_terminal:.delayed_revealer_catchup.began_after_maker_terminal,
       revealer_actor_invocation_count:.availability.taker_invocations_after_reveal_before_maker_terminal},
     intermediate:{phase:.intermediate.protocol_phase,
       lifecycle_disposition:.intermediate.lifecycle_disposition,terminal:.intermediate.terminal,
       remaining_leg_canonical_and_claimable:
         ($recovering[0].intermediate.remaining_leg.canonical and
          $recovering[0].intermediate.remaining_leg.unspent_or_funded and
          $recovering[0].intermediate.remaining_leg.before_signed_later_refund_boundary),
       followup_effect_present:$recovering[0].intermediate.followup_effect_present},
     follower:{fresh_revision_three_observer:.continuation.canonical_reveal_observed_by_fresh_process,
       process_exited_before_followup:.availability.follower_process_exited_at_revision_three,
       fresh_followup_submitter:.availability.fresh_follower_process_submitted_followup,
       fresh_terminal_projector:.availability.distinct_fresh_follower_process_projected_terminal},
     delayed_revealer_catchup:{observation_only:.delayed_revealer_catchup.observation_only,
       bitcoin_successful_resubmission_count:
         .delayed_revealer_catchup.per_chain.bitcoin.successful_resubmission_count,
       lez_successful_resubmission_count:
         .delayed_revealer_catchup.per_chain.lez.successful_resubmission_count,
       successful_resubmission_count:.delayed_revealer_catchup.successful_resubmission_count},
     completion_boundary:.completion.boundary,
     completion_evidence_sha256:$completion_sha,recovering_evidence_sha256:$recovering_sha,
     caller_supplied_secret:.continuation.caller_supplied_secret,
     secret_recorded:.secret_recorded}
  ' "$completion"
}

write_run_evidence() {
  local repository_commit completed_at outer_runner_sha direction_driver_sha lez_bootstrap_sha
  local bedrock_log bedrock_ntp_timeout_count
  local foreign_survivor_summary="null" lez_survivor_summary="null"
  local overlap_summary="null"
  local f7_token_fixture_summary="null" f7_token_fixture_sha=""
  local f7_token_fixture_driver_sha=""
  repository_commit="$(git rev-parse HEAD)"
  completed_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  outer_runner_sha="$(sha256sum scripts/run-m3-actor-local-poc.sh | sed 's/ .*//')"
  direction_driver_sha="$(sha256sum "$direction_driver" | sed 's/ .*//')"
  lez_bootstrap_sha="$(sha256sum "$lez_bootstrap_driver" | sed 's/ .*//')"
  if [[ "$asset_mode" == "custom_token" ]]; then
    f7_token_fixture_sha="$(sha256sum "$f7_token_fixture_evidence" | sed 's/ .*//')"
    f7_token_fixture_driver_sha="$(sha256sum "$f7_token_fixture_driver" | sed 's/ .*//')"
    f7_token_fixture_summary="$(jq -c \
      --arg evidence_path "${relative_run_root}/private/f7-token-fixture/evidence/f7-token-fixture.json" \
      --arg private_path "${relative_run_root}/private/f7-token-fixture/private" \
      --arg evidence_sha "$f7_token_fixture_sha" \
      --arg token_program_id "$lez_token_program_id" \
      --arg ata_program_id "$lez_ata_program_id" '
      {kind:.kind,result:.result,upstream:.upstream,assets:.assets,
       transaction_count:(.transactions | length),evidence_path:$evidence_path,
       private_path:$private_path,evidence_sha256:$evidence_sha,
       token_program_id:$token_program_id,ata_program_id:$ata_program_id,
       provisioned_once_after_bootstrap:true,
       passed_to_each_direction:{fixture_root:true,evidence:true,private_dir:true,
         wallet_binary:true,account_codec:true,program_ids:true}}
    ' "$f7_token_fixture_evidence")"
  fi
  bedrock_log="${repo_root}/.e2e/${lez_run_id}/lez-v02/bedrock/logs/logos-blockchain.log"
  [[ -f "$bedrock_log" && ! -L "$bedrock_log" ]] ||
    fail "run-owned Bedrock log is unavailable for external-resource evidence"
  bedrock_ntp_timeout_count="$(rg -c "NTP sync failed from pool.ntp.org:123" "$bedrock_log" || true)"
  [[ -n "$bedrock_ntp_timeout_count" ]] || bedrock_ntp_timeout_count=0
  [[ "$bedrock_ntp_timeout_count" =~ ^[0-9]+$ ]] ||
    fail "Bedrock NTP timeout count is malformed"
  if [[ "$journey" == "survivor_claim" ]]; then
    foreign_survivor_summary="$(validate_survivor_direction_evidence taker_sells_foreign lez bitcoin)"
    lez_survivor_summary="$(validate_survivor_direction_evidence taker_sells_lez bitcoin lez)"
  fi
  if [[ "$schedule" == "overlap" ]]; then
    overlap_summary="$(jq -c . "${evidence_dir}/overlap-revision-two-window.json")"
  fi
  jq -n \
    --arg run_id "$run_id" \
    --arg journey "$journey" \
    --arg schedule "$schedule" \
    --arg asset_mode "$asset_mode" \
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
    --arg f7_token_fixture "scripts/run-m3-f7-token-fixture.sh" \
    --arg f7_token_fixture_driver_sha "$f7_token_fixture_driver_sha" \
    --arg rapidsnark_dir "$RAPIDSNARK_LIB_DIR" \
    --arg rapidsnark_sha "$rapidsnark_sha" --arg gmp_sha "$gmp_sha" \
    --arg fq_sha "$fq_sha" --arg fr_sha "$fr_sha" \
    --arg bindgen_extra_clang_args "$BINDGEN_EXTRA_CLANG_ARGS" \
    --arg bitcoin_run "$bitcoin_run_id" \
    --arg lez_run "$lez_run_id" \
    --arg lez_slot_duration_seconds "$lez_slot_duration_seconds" \
    --argjson bedrock_ntp_timeout_count "$bedrock_ntp_timeout_count" \
    --argjson foreign_survivor "$foreign_survivor_summary" \
    --argjson lez_survivor "$lez_survivor_summary" \
    --argjson overlap "$overlap_summary" \
    --argjson f7_token_fixture_summary "$f7_token_fixture_summary" \
    --arg foreign_stage2_sha "$(sha256sum "${evidence_dir}/taker_sells_foreign-stage-two.json" | sed 's/ .*//')" \
    --arg lez_stage2_sha "$(sha256sum "${evidence_dir}/taker_sells_lez-stage-two.json" | sed 's/ .*//')" '
    {
      schema_version: 1,
      kind: $packet_kind,
      journey: $journey,
      schedule: $schedule,
      asset_mode: $asset_mode,
      result: "passed",
      run_id: $run_id,
      repository_commit: $repository_commit,
      completed_at: $completed_at,
      certified_executable_scripts: ({
        outer_runner: {repository_path:$outer_runner,sha256:$outer_runner_sha},
        direction_driver: {repository_path:$direction_driver,sha256:$direction_driver_sha},
        lez_bootstrap: {repository_path:$lez_bootstrap,sha256:$lez_bootstrap_sha},
        external_override_allowed: false
      } + (if $asset_mode == "custom_token" then {
        f7_token_fixture:{repository_path:$f7_token_fixture,
          sha256:$f7_token_fixture_driver_sha}
      } else {} end)),
      native_build_prerequisites: ({
        rapidsnark_lib_dir: $rapidsnark_dir,
        files: {
          "librapidsnark.a": $rapidsnark_sha,
          "libgmp.a": $gmp_sha,
          "libfq.a": $fq_sha,
          "libfr.a": $fr_sha
        },
        bindgen_extra_clang_args: $bindgen_extra_clang_args,
        verified_before_offline_build: true
      } + (if $asset_mode == "custom_token" then {
        official_wallet:{source_commit:"a58fbce2ff48c58b7bb5001b1a27e64b9596ee3a",
          source_same_as_local_stack:true,source_clean_and_tag_verified:true,
          cargo_locked_offline:true,target_in_exact_cleaned_secure_root:true}
      } else {} end)),
      asset:
        (if $asset_mode == "custom_token" then {custom_token:$f7_token_fixture_summary}
         else {native:{base_agreement_terms:true}} end),
      services: {
        bitcoin_core: {run_id: $bitcoin_run, version: "31.1", network: "regtest"},
        lez: {run_id: $lez_run, version: "v0.2.0", network: "private_local",
              slot_duration_seconds:$lez_slot_duration_seconds}
      },
      directions: [
        {direction: "taker_sells_foreign", terminal_revision: $terminal_revision,
         terminal_phase: $terminal_phase,
         expected_unique_effects:
           (if $asset_mode == "custom_token" then {bitcoin:2,lez:4}
            elif $journey == "first_lock_refund" then {bitcoin:2,lez:0}
            else {bitcoin:2,lez:3} end),
         maker_second_lock_effect_count:
           (if $journey == "first_lock_refund" then 0 else 1 end),
         stage_two_evidence_sha256: $foreign_stage2_sha},
        {direction: "taker_sells_lez", terminal_revision: $terminal_revision,
         terminal_phase: $terminal_phase,
         expected_unique_effects:
           (if $asset_mode == "custom_token" then {bitcoin:2,lez:4}
            elif $journey == "first_lock_refund" then {bitcoin:0,lez:3}
            else {bitcoin:2,lez:3} end),
         maker_second_lock_effect_count:
           (if $journey == "first_lock_refund" then 0 else 1 end),
         stage_two_evidence_sha256: $lez_stage2_sha}
      ],
      actor_process_model: "fresh_one_shot_process_per_command",
      concurrency:
        (if $schedule == "overlap" then {
          simultaneous_in_flight:$overlap.simultaneous_in_flight,
          overlap_revision:$overlap.revision,
          overlap_phase:$overlap.phase,
          distinct_funding_outpoints:
            (([$overlap.funding_sources.sources[] |
              [.source.transaction_id,.source.output_index]] | unique | length) == 2),
          distinct_agreements:
            (([$overlap.agreements[].agreement_sha256] | unique | length) == 2),
          distinct_actor_state_dbs:
            (([$overlap.actors[].state_db] | unique | length) == 4),
          distinct_signing_journals:
            (([$overlap.actors[].btc_signing_journal,
               $overlap.actors[].lez_signing_journal] | unique | length) == 8),
          distinct_signer_sessions_per_domain:
            (([$overlap.actors[].btc_session_id] | unique | length) == 2 and
             ([$overlap.actors[].lez_session_id] | unique | length) == 2),
          distinct_escrows:
            (([$overlap.agreements[].metadata_account] | unique | length) == 2 and
             ([$overlap.agreements[].custody_account] | unique | length) == 2),
          distinct_deadlines:
            (([$overlap.agreements[].refund_at_ms] | unique | length) == 2 and
             ([$overlap.agreements[].bitcoin_refund_height] | unique | length) == 2),
          chain_mutations_serialized_for_exact_observation:
            $overlap.chain_mutations_serialized_for_exact_observation,
          shared_local_nodes:true,
          shared_fixture_custody_key:
            $overlap.funding_sources.shared_fixture_custody_key,
          arbitrary_n_or_same_direction_scheduler_proven:false
        } else null end),
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
      survivor:
        (if $journey == "survivor_claim" then {
          revealer:$foreign_survivor.revealer,
          follower_role:$foreign_survivor.follower_role,
          protected_absence:{
            starts_after_reveal_submission:
              ($foreign_survivor.protected_absence.starts_after_reveal_submission and
               $lez_survivor.protected_absence.starts_after_reveal_submission),
            ends_after_follower_terminal:
              ($foreign_survivor.protected_absence.ends_after_follower_terminal and
               $lez_survivor.protected_absence.ends_after_follower_terminal),
            revealer_actor_invocation_count:
              ($foreign_survivor.protected_absence.revealer_actor_invocation_count +
               $lez_survivor.protected_absence.revealer_actor_invocation_count)
          },
          intermediate:{phase:$foreign_survivor.intermediate.phase,
            lifecycle_disposition:$foreign_survivor.intermediate.lifecycle_disposition,
            terminal:($foreign_survivor.intermediate.terminal or
              $lez_survivor.intermediate.terminal),
            remaining_leg_canonical_and_claimable:
              ($foreign_survivor.intermediate.remaining_leg_canonical_and_claimable and
               $lez_survivor.intermediate.remaining_leg_canonical_and_claimable),
            followup_effect_present:
              ($foreign_survivor.intermediate.followup_effect_present or
               $lez_survivor.intermediate.followup_effect_present)},
          follower:{
            fresh_revision_three_observer:
              ($foreign_survivor.follower.fresh_revision_three_observer and
               $lez_survivor.follower.fresh_revision_three_observer),
            process_exited_before_followup:
              ($foreign_survivor.follower.process_exited_before_followup and
               $lez_survivor.follower.process_exited_before_followup),
            fresh_followup_submitter:
              ($foreign_survivor.follower.fresh_followup_submitter and
               $lez_survivor.follower.fresh_followup_submitter),
            fresh_terminal_projector:
              ($foreign_survivor.follower.fresh_terminal_projector and
               $lez_survivor.follower.fresh_terminal_projector)},
          delayed_revealer_catchup:{
            observation_only:
              ($foreign_survivor.delayed_revealer_catchup.observation_only and
               $lez_survivor.delayed_revealer_catchup.observation_only),
            bitcoin_successful_resubmission_count:
              ($foreign_survivor.delayed_revealer_catchup.bitcoin_successful_resubmission_count +
               $lez_survivor.delayed_revealer_catchup.bitcoin_successful_resubmission_count),
            lez_successful_resubmission_count:
              ($foreign_survivor.delayed_revealer_catchup.lez_successful_resubmission_count +
               $lez_survivor.delayed_revealer_catchup.lez_successful_resubmission_count),
            successful_resubmission_count:
              ($foreign_survivor.delayed_revealer_catchup.successful_resubmission_count +
               $lez_survivor.delayed_revealer_catchup.successful_resubmission_count)},
          direction_evidence:{
            taker_sells_foreign:$foreign_survivor,
            taker_sells_lez:$lez_survivor},
          caller_supplied_secret:
            ($foreign_survivor.caller_supplied_secret or $lez_survivor.caller_supplied_secret),
          secret_recorded:
            ($foreign_survivor.secret_recorded or $lez_survivor.secret_recorded)
        } else null end),
      expected_unique_effects_by_direction:
        (if $asset_mode == "custom_token" then
           {taker_sells_foreign:{bitcoin:2,lez:4},
            taker_sells_lez:{bitcoin:2,lez:4}}
         elif $journey == "first_lock_refund" then
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
      external_resources: {
        public_rpc:false,
        faucet:false,
        public_funds:false,
        test_funds:
          (if $asset_mode == "custom_token" then
             "deterministic_local_genesis_regtest_and_official_tokens"
           else "deterministic_local_genesis_and_regtest_outputs" end),
        bedrock_ntp:{
          endpoint:"pool.ntp.org:123/udp",
          attempted_by_pinned_component:true,
          required_for_certification:false,
          observed_timeout_count:$bedrock_ntp_timeout_count
        },
        certification_success_depends_on_external_network:false
      },
      public_rpc_used: false,
      faucet_used: false,
      public_funds_used: false,
      private_material_disclosed: false
    }' >"${run_evidence}.partial"
  chmod 0600 "${run_evidence}.partial"
  mv "${run_evidence}.partial" "$run_evidence"
  jq -e --arg journey "$journey" --arg schedule "$schedule" \
    --arg asset_mode "$asset_mode" \
    --arg packet_kind "$packet_kind" \
    --argjson terminal_revision "$terminal_revision" \
    --arg terminal_phase "$terminal_phase" --arg replay_command "$replay_command" \
    --arg actor_owned_effect_semantics "$actor_owned_effect_semantics" '
    .schema_version == 1
    and .kind == $packet_kind
    and .journey == $journey
    and .schedule == $schedule
    and .asset_mode == $asset_mode
    and .result == "passed"
    and (.directions | length == 2)
    and all(.directions[];
      .terminal_revision == $terminal_revision and .terminal_phase == $terminal_phase)
    and .actor_owned_effect_semantics == $actor_owned_effect_semantics
    and (if $schedule == "overlap" then
      .concurrency.simultaneous_in_flight == true
      and .concurrency.overlap_revision == 2
      and .concurrency.overlap_phase == "both_legs_locked"
      and .concurrency.distinct_funding_outpoints == true
      and .concurrency.distinct_agreements == true
      and .concurrency.distinct_actor_state_dbs == true
      and .concurrency.distinct_signing_journals == true
      and .concurrency.distinct_signer_sessions_per_domain == true
      and .concurrency.distinct_escrows == true
      and .concurrency.distinct_deadlines == true
      and .concurrency.chain_mutations_serialized_for_exact_observation == true
      and .concurrency.shared_local_nodes == true
      and .concurrency.arbitrary_n_or_same_direction_scheduler_proven == false
    else .concurrency == null end)
    and .expected_unique_effects_by_direction ==
      (if $asset_mode == "custom_token" then
         {taker_sells_foreign:{bitcoin:2,lez:4},
          taker_sells_lez:{bitcoin:2,lez:4}}
       elif $journey == "first_lock_refund" then
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
    and (if $journey == "survivor_claim" then
      .survivor.revealer == "taker"
      and .survivor.follower_role == "maker"
      and .survivor.protected_absence.revealer_actor_invocation_count == 0
      and .survivor.intermediate.phase == "claim_evidence_available"
      and .survivor.intermediate.lifecycle_disposition == "recovering"
      and .survivor.intermediate.terminal == false
      and .survivor.intermediate.remaining_leg_canonical_and_claimable == true
      and .survivor.follower.fresh_revision_three_observer == true
      and .survivor.follower.process_exited_before_followup == true
      and .survivor.follower.fresh_followup_submitter == true
      and .survivor.follower.fresh_terminal_projector == true
      and .survivor.delayed_revealer_catchup.observation_only == true
      and .survivor.delayed_revealer_catchup.bitcoin_successful_resubmission_count == 0
      and .survivor.delayed_revealer_catchup.lez_successful_resubmission_count == 0
      and .survivor.delayed_revealer_catchup.successful_resubmission_count == 0
      and .survivor.direction_evidence.taker_sells_foreign.direction == "taker_sells_foreign"
      and .survivor.direction_evidence.taker_sells_lez.direction == "taker_sells_lez"
      and (.survivor.direction_evidence.taker_sells_foreign.completion_evidence_sha256 |
        test("^[0-9a-f]{64}$"))
      and (.survivor.direction_evidence.taker_sells_lez.completion_evidence_sha256 |
        test("^[0-9a-f]{64}$"))
      and (.survivor.direction_evidence.taker_sells_foreign.recovering_evidence_sha256 |
        test("^[0-9a-f]{64}$"))
      and (.survivor.direction_evidence.taker_sells_lez.recovering_evidence_sha256 |
        test("^[0-9a-f]{64}$"))
      and .survivor.caller_supplied_secret == false
      and .survivor.secret_recorded == false
    else .survivor == null end)
    and (if $asset_mode == "custom_token" then
      .asset.custom_token.kind == "m3_f7_official_token_fixture"
      and .asset.custom_token.result == "passed"
      and .asset.custom_token.transaction_count == 8
      and (.asset.custom_token.evidence_sha256 | test("^[0-9a-f]{64}$"))
      and .asset.custom_token.provisioned_once_after_bootstrap == true
      and .asset.custom_token.passed_to_each_direction == {
        fixture_root:true,evidence:true,private_dir:true,wallet_binary:true,
        account_codec:true,program_ids:true}
      and .asset.custom_token.token_program_id ==
        "c5d50f88bfe7cb14b421673e9441aade7571e522eef035cc24d80b2e53c69a7c"
      and .asset.custom_token.ata_program_id ==
        "95841cc8bd2c87d7111bc5c7f3aa2a85d35e90f7217e82a397aa05acd51500f8"
      and .native_build_prerequisites.official_wallet.source_same_as_local_stack == true
      and .native_build_prerequisites.official_wallet.cargo_locked_offline == true
      and .native_build_prerequisites.official_wallet.target_in_exact_cleaned_secure_root == true
    else .asset == {native:{base_agreement_terms:true}}
      and (.native_build_prerequisites | has("official_wallet") | not) end)
    and .replay_command == $replay_command
    and .replay_resubmission_count == 0
    and .external_resources.public_rpc == false
    and .external_resources.faucet == false
    and .external_resources.public_funds == false
    and .external_resources.test_funds ==
      (if $asset_mode == "custom_token" then
         "deterministic_local_genesis_regtest_and_official_tokens"
       else "deterministic_local_genesis_and_regtest_outputs" end)
    and .external_resources.bedrock_ntp.endpoint == "pool.ntp.org:123/udp"
    and .external_resources.bedrock_ntp.attempted_by_pinned_component == true
    and .external_resources.bedrock_ntp.required_for_certification == false
    and (.external_resources.bedrock_ntp.observed_timeout_count | numbers) >= 0
    and .external_resources.certification_success_depends_on_external_network == false
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
provision_bitcoin_funding_sources
bootstrap_lez_runtime
provision_f7_token_fixture

# Directions share only the actual local nodes. The sequential schedule retains
# the historical one-direction-at-a-time proof. The overlap schedule keeps both
# direction controllers alive and withholds settlement until both independent
# swaps have durably reached revision two.
if [[ "$schedule" == "overlap" ]]; then
  run_overlapping_actor_flows
else
  for direction in "${directions[@]}"; do
    run_stage_two "$direction"
    run_direction_actor_flow "$direction"
    assert_terminal_and_replay "$direction"
  done
fi

validate_actual_effect_manifests
write_run_evidence
echo "${success_label} passed: ${run_evidence}"
