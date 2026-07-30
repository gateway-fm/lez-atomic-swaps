#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

export LC_ALL=C
umask 077

readonly mode="${M3_ACTOR_POC_MODE:-execute}"
readonly m5_btc_application_mode="${M5_BTC_APPLICATION_MODE:-0}"
if [[ "$m5_btc_application_mode" != 0 && "$m5_btc_application_mode" != 1 ]]; then
  echo 'M5_BTC_APPLICATION_MODE must be 0 or 1' >&2
  exit 2
fi
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
if [[ "$m5_btc_application_mode" == 1 && "$asset_mode" != native ]]; then
  echo 'M5_BTC_APPLICATION_MODE=1 requires M3_ACTOR_POC_ASSET_MODE=native' >&2
  exit 2
fi
if [[ "$m5_btc_application_mode" == 1 && "$schedule" != sequential ]]; then
  echo 'M5_BTC_APPLICATION_MODE=1 requires M3_ACTOR_POC_SCHEDULE=sequential' >&2
  exit 2
fi
if [[ "$m5_btc_application_mode" == 1 && "$journey" != claim ]]; then
  echo 'M5_BTC_APPLICATION_MODE=1 requires M3_ACTOR_POC_JOURNEY=claim' >&2
  exit 2
fi
case "$journey" in
  claim)
    terminal_revision=4
    terminal_phase="completed"
    replay_command="drive"
    if [[ "$m5_btc_application_mode" == 1 ]]; then
      packet_kind="m5_btc_application_local_poc"
      success_label="M5 BTC application local PoC"
    elif [[ "$asset_mode" == "custom_token" ]]; then
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
    # Native and custom-token observation both bind to immutable requested
    # finalized windows; live tip advancement no longer needs a slow slot.
    lez_slot_duration_seconds="1.0"
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
readonly timing_dir="${private_dir}/timings"
readonly phase_timing_journal="${timing_dir}/outer.ndjson.partial"
readonly phase_timings_evidence="${evidence_dir}/m3-phase-timings.json"
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
readonly bitcoin_service_driver="${repo_root}/scripts/run-bitcoin-core-e2e.sh"
readonly lez_service_driver="${repo_root}/scripts/run-lez-v02-stack.sh"
readonly f7_token_fixture_driver="${repo_root}/scripts/run-m3-f7-token-fixture.sh"
readonly bitcoin_anchor_assignment_filter="${repo_root}/scripts/jq/m3-bitcoin-anchor-assignment.jq"
readonly lez_source_dir="${LEZ_V02_SOURCE_DIR:-/tmp/lez-v020-native-investigation}"
readonly official_wallet_target="${secure_state_root}/official-wallet-target"
readonly official_wallet_bin="${official_wallet_target}/debug/wallet"
readonly official_wallet_cache_helper="${repo_root}/scripts/prepare-m3-official-wallet-artifact.sh"
readonly official_wallet_cache_evidence="${evidence_dir}/official-wallet-artifact.json"
readonly official_wallet_cache_root="${M3_OFFICIAL_WALLET_CACHE_ROOT:-/tmp/lez-atomic-swaps-cache-$(id -u)/m3-official-wallet-v1}"
readonly lez_v02_source_commit="a58fbce2ff48c58b7bb5001b1a27e64b9596ee3a"
readonly m3_f7_lez_guest_sha256="bc2ea18eaacb917727934fcf0366dd54c1f9a2b69b61ea53080c926850967fd7"
readonly m3_f7_lez_program_id="f3ead24b95d316ce91980cb3531a70b83a27fd1640f47c1b857757aef26c244e"
readonly m3_f7_lez_deployer_sha256="a7f1e2593844bef8fc61cab4b37566fb5c6b8cb8eba27efb50f985e995ba191c"
if [[ "$m5_btc_application_mode" == 1 ]]; then
  expected_lez_guest_sha256="dc370bc34b432317730c51b49342760dbc675fca700e300b30b5fadefe5b7292"
  expected_lez_program_id="4d6590332948743c2db88a183755815354ef92560550cd206ac27bddeea12c82"
  expected_lez_deployer_sha256="${M5_LEZ_DEPLOYER_SHA256:-}"
  expected_lez_deployment_profile="m4_checked_local"
else
  expected_lez_guest_sha256="$m3_f7_lez_guest_sha256"
  expected_lez_program_id="$m3_f7_lez_program_id"
  expected_lez_deployer_sha256="$m3_f7_lez_deployer_sha256"
  expected_lez_deployment_profile="m3_f7_checked_local"
fi
readonly expected_lez_guest_sha256 expected_lez_program_id
readonly expected_lez_deployer_sha256 expected_lez_deployment_profile
readonly lez_token_program_id="c5d50f88bfe7cb14b421673e9441aade7571e522eef035cc24d80b2e53c69a7c"
readonly lez_ata_program_id="95841cc8bd2c87d7111bc5c7f3aa2a85d35e90f7217e82a397aa05acd51500f8"
if [[ "$m5_btc_application_mode" == 1 ]]; then
  readonly -a directions=(taker_sells_foreign)
else
  readonly -a directions=(taker_sells_foreign taker_sells_lez)
fi
declare -A m5_btc_swap_ids=()
declare -A overlap_pids=()
declare -A overlap_logs=()
phase_timing_now_ms=0
phase_timing_origin_ms=0
phase_timing_started_at_utc=""
phase_timing_active_phase=""
phase_timing_active_direction=""
phase_timing_active_start_ms=0
phase_timing_sequence=0
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
    --arg m5_btc_application_mode "$m5_btc_application_mode" \
    --arg deployment_profile "$expected_lez_deployment_profile" \
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
      m5_btc_application_mode: ($m5_btc_application_mode == "1"),
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
        lez_v0_2: {
          slot_duration_seconds: $lez_slot_duration_seconds,
          deployment_profile: $deployment_profile
        }
      },
      directions:
        (if $m5_btc_application_mode == "1" then
           ["taker_sells_foreign"]
         else ["taker_sells_foreign", "taker_sells_lez"] end),
      application_route:
        (if $m5_btc_application_mode == "1" then {
          pair:"bitcoin", direction:"taker_sells_foreign",
          delivery_before_stage_two:true, authenticated_swap_id:true,
          real_maker_cli:true, real_taker_cli:true,
          schema_6_role_provisioning:true
        } else null end),
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
            terminal_balance_evidence:{
              official_wallet_owner_ata_reads:true,
              finalized_actor_custody_read:true,
              exact_direction_balances:true,
              conservation_total:250},
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
             "verified_cache_copy_in_exact_run_owned_secure_state_root" else null end),
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
        repository_clean_exact_head: true,
        origin_main_equals_head: true,
        executable_hashes_stable_from_start_to_publication: true,
        executable_script_sha256s:
          (["outer_runner", "direction_driver", "lez_bootstrap",
            "bitcoin_service_driver", "lez_service_driver"] +
           (if $asset_mode == "custom_token" then
              ["f7_token_fixture","official_wallet_cache"] else [] end))
      },
      build_prerequisites: {
        rapidsnark_lib_dir: "explicit_absolute_canonical_verified_v0_0_8",
        rapidsnark_files: ["librapidsnark.a","libgmp.a","libfq.a","libfr.a"],
        bindgen_extra_clang_args: "explicit_nonempty",
        inherited_by_offline_sidecar_build: true,
        official_wallet:
          (if $asset_mode == "custom_token" then {
            source:"same_exact_clean_pinned_lez_v0_2_checkout",
            cargo:"locked_offline",
            cache:"content_addressed_executable_only_fail_closed",
            target:"verified_copy_in_exact_run_owned_secure_state_root"
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

if [[ "$m5_btc_application_mode" == 1 ]]; then
  [[ -n "$expected_lez_deployer_sha256" ]] ||
    fail "M5_LEZ_DEPLOYER_SHA256 is required"
  [[ "$expected_lez_deployer_sha256" =~ ^[0-9a-f]{64}$ ]] ||
    fail "M5_LEZ_DEPLOYER_SHA256 must be a lowercase SHA-256 digest"
fi

for command_name in awk cargo chmod curl date docker find git id jq kill mkdir mv ps readlink rg rm sed setsid sha256sum sleep stat; do
  command -v "$command_name" >/dev/null || fail "missing required tool: ${command_name}"
done

repository_status="$(git status --porcelain --untracked-files=all)" ||
  fail "repository status query failed"
[[ -z "$repository_status" ]] ||
  fail "M3 actor execution requires one clean exact-HEAD repository"
repository_commit_at_start="$(git rev-parse HEAD)" || fail "repository HEAD query failed"
origin_main_at_start="$(git rev-parse refs/remotes/origin/main)" ||
  fail "origin/main remote-tracking commit query failed"
[[ "$origin_main_at_start" == "$repository_commit_at_start" ]] ||
  fail "M3 actor execution requires HEAD to equal the already-pushed origin/main commit"
readonly repository_commit_at_start origin_main_at_start
for tracked_path in scripts/run-m3-actor-local-poc.sh \
  scripts/run-m3-actor-direction.sh scripts/run-m3-lez-bootstrap.sh \
  scripts/run-bitcoin-core-e2e.sh scripts/run-lez-v02-stack.sh; do
  git ls-files --error-unmatch "$tracked_path" >/dev/null ||
    fail "M3 executable is not tracked at HEAD: $tracked_path"
done
outer_runner_sha_at_start="$(sha256sum scripts/run-m3-actor-local-poc.sh | sed 's/ .*//')"
direction_driver_sha_at_start="$(sha256sum "$direction_driver" | sed 's/ .*//')"
lez_bootstrap_sha_at_start="$(sha256sum "$lez_bootstrap_driver" | sed 's/ .*//')"
bitcoin_service_driver_sha_at_start="$(sha256sum "$bitcoin_service_driver" | sed 's/ .*//')"
lez_service_driver_sha_at_start="$(sha256sum "$lez_service_driver" | sed 's/ .*//')"
readonly outer_runner_sha_at_start direction_driver_sha_at_start lez_bootstrap_sha_at_start
readonly bitcoin_service_driver_sha_at_start lez_service_driver_sha_at_start
f7_token_fixture_driver_sha_at_start=""
official_wallet_cache_helper_sha_at_start=""

[[ -z "${M3_ACTOR_DIRECTION_DRIVER+x}" ]] ||
  fail "M3_ACTOR_DIRECTION_DRIVER overrides are non-certifying and forbidden"
[[ -x "$direction_driver" && ! -L "$direction_driver" ]] ||
  fail "M3 actor direction driver is missing or unsafe: ${direction_driver}"
[[ "$(readlink -f "$direction_driver")" == "$direction_driver" ]] ||
  fail "M3 actor direction driver path is not canonical"
[[ -x "$lez_bootstrap_driver" && ! -L "$lez_bootstrap_driver" ]] ||
  fail "M3 LEZ bootstrap driver is missing or unsafe"
[[ -x "$bitcoin_service_driver" && ! -L "$bitcoin_service_driver" &&
   "$(readlink -f "$bitcoin_service_driver")" == "$bitcoin_service_driver" ]] ||
  fail "Bitcoin service driver is missing or unsafe"
[[ -x "$lez_service_driver" && ! -L "$lez_service_driver" &&
   "$(readlink -f "$lez_service_driver")" == "$lez_service_driver" ]] ||
  fail "LEZ service driver is missing or unsafe"
if [[ "$asset_mode" == "custom_token" ]]; then
  command -v sqlite3 >/dev/null || fail "missing required tool for actor evidence: sqlite3"
  command -v timeout >/dev/null || fail "missing required tool for official wallet reads: timeout"
  [[ -x "$f7_token_fixture_driver" && ! -L "$f7_token_fixture_driver" ]] ||
    fail "M3 F7 token-fixture driver is missing or unsafe"
  [[ "$(readlink -f "$f7_token_fixture_driver")" == "$f7_token_fixture_driver" ]] ||
    fail "M3 F7 token-fixture driver path is not canonical"
  for tracked_path in scripts/run-m3-f7-token-fixture.sh \
    scripts/prepare-m3-official-wallet-artifact.sh; do
    git ls-files --error-unmatch "$tracked_path" >/dev/null ||
      fail "M3 custom-token executable is not tracked at HEAD: $tracked_path"
  done
  f7_token_fixture_driver_sha_at_start="$(sha256sum "$f7_token_fixture_driver" |
    sed 's/ .*//')"
  official_wallet_cache_helper_sha_at_start="$(sha256sum "$official_wallet_cache_helper" |
    sed 's/ .*//')"
fi
readonly f7_token_fixture_driver_sha_at_start official_wallet_cache_helper_sha_at_start
[[ -n "${LEZ_V02_ARTIFACT_TARGET_DIR:-}" && "$LEZ_V02_ARTIFACT_TARGET_DIR" == /* &&
   -d "$LEZ_V02_ARTIFACT_TARGET_DIR" && ! -L "$LEZ_V02_ARTIFACT_TARGET_DIR" ]] ||
  fail "set LEZ_V02_ARTIFACT_TARGET_DIR to one verified absolute artifact target"
readonly lez_deployer="${LEZ_V02_ARTIFACT_TARGET_DIR}/debug/lez-zec-escrow-v02-deployer"
readonly lez_guest_elf="${LEZ_V02_ARTIFACT_TARGET_DIR}/riscv-guest/lez-zec-escrow-v02-methods/lez-zec-escrow-v02-guest/riscv32im-risc0-zkvm-elf/docker/zec_escrow_v02.bin"
[[ -x "$lez_deployer" && -f "$lez_deployer" && ! -L "$lez_deployer" ]] ||
  fail "verified LEZ deployer is unavailable in the artifact target"

validate_exact_regular_file_sha256() {
  local label="$1" file="$2" expected="$3" actual
  [[ -f "$file" && ! -L "$file" ]] ||
    fail "${label} is missing or is not a regular non-symlink file"
  [[ "$(readlink -f "$file")" == "$file" ]] ||
    fail "${label} path is not canonical"
  actual="$(sha256sum "$file")" ||
    fail "${label} SHA-256 query failed"
  actual="${actual%% *}"
  [[ "$actual" == "$expected" ]] ||
    fail "${label} SHA-256 does not match the pinned F7 artifact"
}

validate_lez_artifact_identity() {
  validate_exact_regular_file_sha256 "LEZ deployer" "$lez_deployer" "$expected_lez_deployer_sha256"
  validate_exact_regular_file_sha256 "LEZ canonical guest ELF" "$lez_guest_elf" "$expected_lez_guest_sha256"
}

validate_lez_artifact_identity

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
  local source_status
  [[ "$lez_source_dir" == /* && -d "$lez_source_dir/.git" && ! -L "$lez_source_dir" &&
     "$(readlink -f "$lez_source_dir")" == "$lez_source_dir" ]] ||
    fail "LEZ v0.2 source must be one canonical absolute Git checkout"
  source_status="$(git -C "$lez_source_dir" status --porcelain --untracked-files=all \
    --ignored=matching)" ||
    fail "LEZ v0.2 source status query failed"
  [[ -z "$source_status" ]] ||
    fail "LEZ v0.2 source checkout is dirty"
  [[ "$(git -C "$lez_source_dir" rev-parse HEAD)" == "$lez_v02_source_commit" ]] ||
    fail "LEZ v0.2 source checkout is not the pinned commit"
  [[ "$(git -C "$lez_source_dir" rev-parse 'refs/tags/v0.2.0^{}')" == \
     "$lez_v02_source_commit" ]] || fail "LEZ v0.2 source tag does not match the pinned commit"
}

if [[ "$asset_mode" == "custom_token" ]]; then
  validate_official_wallet_source
fi

parse_proc_uptime_ms() {
  local raw="$1" output_name="$2"
  local seconds fraction fraction_ms seconds_number milliseconds
  [[ "$output_name" =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]] || return 1
  [[ "$raw" =~ ^(0|[1-9][0-9]*)\.([0-9]+)$ ]] || return 1
  seconds="${BASH_REMATCH[1]}"
  fraction="${BASH_REMATCH[2]}"
  if (( ${#seconds} > 13 )); then
    return 1
  fi
  if (( ${#seconds} == 13 && 10#$seconds > 9007199254740 )); then
    return 1
  fi
  fraction_ms="${fraction}000"
  fraction_ms="${fraction_ms:0:3}"
  seconds_number=$((10#$seconds))
  milliseconds=$((10#$fraction_ms))
  if (( seconds_number == 9007199254740 && milliseconds > 991 )); then
    return 1
  fi
  printf -v "$output_name" '%d' "$((seconds_number * 1000 + milliseconds))"
}

read_monotonic_ms() {
  local uptime_value _ignored
  IFS=' ' read -r uptime_value _ignored </proc/uptime || return 1
  parse_proc_uptime_ms "$uptime_value" phase_timing_now_ms
}

expected_phase_timings_json() {
  [[ "$schedule" == "sequential" || "$schedule" == "overlap" ]] || return 1
  [[ "$asset_mode" == "native" || "$asset_mode" == "custom_token" ]] || return 1
  jq -cn --arg schedule "$schedule" --arg asset_mode "$asset_mode" \
    --arg m5_btc_application_mode "$m5_btc_application_mode" '
    [
      {phase_id:"contract_validation",direction:null},
      {phase_id:"prebuild",direction:null},
      {phase_id:"identities_stage_one",direction:null},
      {phase_id:"node_startup",direction:null},
      {phase_id:"bitcoin_funding",direction:null},
      {phase_id:"lez_bootstrap",direction:null}
    ]
    + (if $asset_mode == "custom_token" then
        [{phase_id:"f7_fixture",direction:null}]
      else [] end)
    + (if $schedule == "overlap" then
        [{phase_id:"directions_overlap",direction:null}]
      else
        ([
          {phase_id:"direction_taker_sells_foreign_reserve_funding",
            direction:"taker_sells_foreign"},
          {phase_id:"direction_taker_sells_foreign_stage_two",
            direction:"taker_sells_foreign"},
          {phase_id:"direction_taker_sells_foreign_actor_flow",
            direction:"taker_sells_foreign"},
          {phase_id:"direction_taker_sells_foreign_terminal_replay",
            direction:"taker_sells_foreign"}
        ]
        + (if $asset_mode == "custom_token" then
            [{phase_id:"direction_taker_sells_foreign_terminal_balances",
              direction:"taker_sells_foreign"}]
          else [] end)
        + (if $m5_btc_application_mode == "1" then [] else [
          {phase_id:"direction_taker_sells_lez_reserve_funding",
            direction:"taker_sells_lez"},
          {phase_id:"direction_taker_sells_lez_stage_two",
            direction:"taker_sells_lez"},
          {phase_id:"direction_taker_sells_lez_actor_flow",
            direction:"taker_sells_lez"},
          {phase_id:"direction_taker_sells_lez_terminal_replay",
            direction:"taker_sells_lez"}
        ]
        + (if $asset_mode == "custom_token" then
            [{phase_id:"direction_taker_sells_lez_terminal_balances",
              direction:"taker_sells_lez"}]
          else [] end) end))
      end)
    + [{phase_id:"effect_validation",direction:null}]
  '
}

initialize_phase_timings() {
  local expected
  expected="$(expected_phase_timings_json)" || return 1
  jq -e 'length > 0' <<<"$expected" >/dev/null || return 1
  [[ -d "$(dirname "$timing_dir")" && ! -L "$(dirname "$timing_dir")" ]] || return 1
  [[ ! -e "$timing_dir" && ! -L "$timing_dir" ]] || return 1
  [[ ! -e "$phase_timings_evidence" && ! -L "$phase_timings_evidence" &&
     ! -e "${phase_timings_evidence}.partial" &&
     ! -L "${phase_timings_evidence}.partial" ]] || return 1
  mkdir -m 0700 "$timing_dir" || return 1
  [[ -d "$timing_dir" && ! -L "$timing_dir" &&
     "$(stat -c '%u:%a' "$timing_dir")" == "$(id -u):700" ]] || return 1
  : >"$phase_timing_journal" || return 1
  chmod 0600 "$phase_timing_journal" || return 1
  [[ -f "$phase_timing_journal" && ! -L "$phase_timing_journal" &&
     "$(stat -c '%u:%a' "$phase_timing_journal")" == "$(id -u):600" ]] || return 1
  read_monotonic_ms || return 1
  phase_timing_origin_ms="$phase_timing_now_ms"
  phase_timing_started_at_utc="$(date -u +%Y-%m-%dT%H:%M:%SZ)" || return 1
  phase_timing_active_phase=""
  phase_timing_active_direction=""
  phase_timing_active_start_ms=0
  phase_timing_sequence=0
}

phase_timing_begin() {
  local phase_id="$1" direction="${2:-}" expected expected_phase expected_direction
  [[ -z "$phase_timing_active_phase" ]] || return 1
  [[ -f "$phase_timing_journal" && ! -L "$phase_timing_journal" &&
     "$(stat -c '%u:%a' "$phase_timing_journal")" == "$(id -u):600" ]] || return 1
  expected="$(expected_phase_timings_json)" || return 1
  expected_phase="$(jq -er --argjson index "$phase_timing_sequence" \
    '.[$index].phase_id // empty' <<<"$expected")" || return 1
  expected_direction="$(jq -er --argjson index "$phase_timing_sequence" \
    '.[$index].direction // ""' <<<"$expected")" || return 1
  [[ "$phase_id" == "$expected_phase" && "$direction" == "$expected_direction" ]] || return 1
  read_monotonic_ms || return 1
  (( phase_timing_now_ms >= phase_timing_origin_ms )) || return 1
  phase_timing_active_phase="$phase_id"
  phase_timing_active_direction="$direction"
  phase_timing_active_start_ms=$((phase_timing_now_ms - phase_timing_origin_ms))
}

phase_timing_end() {
  local phase_id="$1" end_offset duration direction_json next_sequence
  [[ -n "$phase_timing_active_phase" && "$phase_id" == "$phase_timing_active_phase" ]] || return 1
  [[ -f "$phase_timing_journal" && ! -L "$phase_timing_journal" &&
     "$(stat -c '%u:%a' "$phase_timing_journal")" == "$(id -u):600" ]] || return 1
  read_monotonic_ms || return 1
  (( phase_timing_now_ms >= phase_timing_origin_ms )) || return 1
  end_offset=$((phase_timing_now_ms - phase_timing_origin_ms))
  (( end_offset >= phase_timing_active_start_ms )) || return 1
  duration=$((end_offset - phase_timing_active_start_ms))
  next_sequence=$((phase_timing_sequence + 1))
  if [[ -n "$phase_timing_active_direction" ]]; then
    direction_json="\"${phase_timing_active_direction}\""
  else
    direction_json="null"
  fi
  printf '{"schema_version":1,"sequence":%d,"producer":"outer","phase_id":"%s","direction":%s,"start_offset_ms":%d,"end_offset_ms":%d,"duration_ms":%d,"outcome":"passed"}\n' \
    "$next_sequence" "$phase_timing_active_phase" "$direction_json" \
    "$phase_timing_active_start_ms" "$end_offset" "$duration" \
    >>"$phase_timing_journal" || return 1
  phase_timing_sequence="$next_sequence"
  phase_timing_active_phase=""
  phase_timing_active_direction=""
  phase_timing_active_start_ms=0
}

finalize_phase_timings() {
  local expected completed_at total_duration partial
  [[ -z "$phase_timing_active_phase" ]] || return 1
  [[ -f "$phase_timing_journal" && ! -L "$phase_timing_journal" &&
     "$(stat -c '%u:%a' "$phase_timing_journal")" == "$(id -u):600" ]] || return 1
  [[ -d "$evidence_dir" && ! -L "$evidence_dir" ]] || return 1
  partial="${phase_timings_evidence}.partial"
  [[ ! -e "$phase_timings_evidence" && ! -L "$phase_timings_evidence" &&
     ! -e "$partial" && ! -L "$partial" ]] || return 1
  expected="$(expected_phase_timings_json)" || return 1
  read_monotonic_ms || return 1
  (( phase_timing_now_ms >= phase_timing_origin_ms )) || return 1
  total_duration=$((phase_timing_now_ms - phase_timing_origin_ms))
  completed_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)" || return 1
  jq -s \
    --argjson expected "$expected" \
    --arg run_id "$run_id" --arg journey "$journey" \
    --arg schedule "$schedule" --arg asset_mode "$asset_mode" \
    --arg started_at "$phase_timing_started_at_utc" --arg completed_at "$completed_at" \
    --argjson total_duration "$total_duration" '
    def exact_keys($expected_keys): (keys | sort) == ($expected_keys | sort);
    . as $records
    | ($expected | length) as $count
    | if
        ($records | length) == $count
        and all($records[];
          exact_keys(["schema_version","sequence","producer","phase_id","direction",
            "start_offset_ms","end_offset_ms","duration_ms","outcome"])
          and .schema_version == 1 and .producer == "outer" and .outcome == "passed"
          and (.sequence | type) == "number" and .sequence == (.sequence | floor)
          and (.phase_id | type) == "string"
          and ((.direction == null) or (.direction | type) == "string")
          and (.start_offset_ms | type) == "number"
          and .start_offset_ms == (.start_offset_ms | floor)
          and (.end_offset_ms | type) == "number"
          and .end_offset_ms == (.end_offset_ms | floor)
          and (.duration_ms | type) == "number"
          and .duration_ms == (.duration_ms | floor)
          and .start_offset_ms >= 0
          and .end_offset_ms >= .start_offset_ms
          and .duration_ms == (.end_offset_ms - .start_offset_ms)
          and .end_offset_ms <= $total_duration)
        and [$records[].sequence] == [range(1; $count + 1)]
        and [$records[].phase_id] == [$expected[].phase_id]
        and [$records[].direction] == [$expected[].direction]
        and all(range(1; $count);
          . as $index
          | $records[$index].start_offset_ms >= $records[$index - 1].end_offset_ms)
      then
        ([$records[].duration_ms] | add // 0) as $measured
        | {
            schema_version:1,
            kind:"m3_monotonic_phase_timings",
            result:"execution_passed_pre_cleanup",
            run_id:$run_id,
            journey:$journey,
            schedule:$schedule,
            asset_mode:$asset_mode,
            coverage:{
              starts_after_run_directory_initialization:true,
              ends_before_run_evidence_publication:true,
              cleanup_in_separate_attestation:true
            },
            clock:{
              source:"linux_proc_uptime",
              unit:"milliseconds",
              resolution_ms:10,
              includes_suspend:true,
              wall_clock_used_for_duration:false
            },
            started_at_utc:$started_at,
            completed_at_utc:$completed_at,
            total_duration_ms:$total_duration,
            unattributed_duration_ms:($total_duration - $measured),
            phases:$records,
            private_material_disclosed:false
          }
      else error("invalid M3 phase timing journal") end
  ' "$phase_timing_journal" >"$partial" || {
    rm -f -- "$partial"
    return 1
  }
  chmod 0600 "$partial" || {
    rm -f -- "$partial"
    return 1
  }
  [[ -f "$partial" && ! -L "$partial" &&
     "$(stat -c '%u:%a' "$partial")" == "$(id -u):600" ]] || {
    rm -f -- "$partial"
    return 1
  }
  jq -e --arg run_id "$run_id" --arg journey "$journey" \
    --arg schedule "$schedule" --arg asset_mode "$asset_mode" \
    --argjson count "$(jq 'length' <<<"$expected")" '
    (keys | sort) == (["schema_version","kind","result","run_id","journey","schedule",
      "asset_mode","coverage","clock","started_at_utc","completed_at_utc",
      "total_duration_ms","unattributed_duration_ms","phases",
      "private_material_disclosed"] | sort)
    and .schema_version == 1 and .kind == "m3_monotonic_phase_timings"
    and .result == "execution_passed_pre_cleanup"
    and .run_id == $run_id and .journey == $journey
    and .schedule == $schedule and .asset_mode == $asset_mode
    and (.phases | length) == $count
    and .private_material_disclosed == false
  ' "$partial" >/dev/null || {
    rm -f -- "$partial"
    return 1
  }
  mv -n -- "$partial" "$phase_timings_evidence" || {
    rm -f -- "$partial"
    return 1
  }
  [[ ! -e "$partial" && ! -L "$partial" &&
     -f "$phase_timings_evidence" && ! -L "$phase_timings_evidence" &&
     "$(stat -c '%u:%a' "$phase_timings_evidence")" == "$(id -u):600" ]] || {
    rm -f -- "$partial"
    return 1
  }
}

phase_timings_hash_stable() {
  local expected_sha="$1" actual_sha
  [[ "$expected_sha" =~ ^[0-9a-f]{64}$ ]] || return 1
  [[ -f "$phase_timings_evidence" && ! -L "$phase_timings_evidence" &&
     "$(stat -c '%u:%a' "$phase_timings_evidence")" == "$(id -u):600" ]] || return 1
  actual_sha="$(sha256sum "$phase_timings_evidence")" || return 1
  actual_sha="${actual_sha%% *}"
  [[ "$actual_sha" == "$expected_sha" ]]
}

validate_phase_timings_for_run_evidence() {
  local output_name="$1" expected summary sha_before sha_after
  [[ "$output_name" =~ ^[A-Za-z_][A-Za-z0-9_]*$ &&
     "$output_name" != "phase_timing_sha" ]] || return 1
  [[ -f "$phase_timings_evidence" && ! -L "$phase_timings_evidence" &&
     "$(stat -c '%u:%a' "$phase_timings_evidence")" == "$(id -u):600" ]] || return 1
  expected="$(expected_phase_timings_json)" || return 1
  sha_before="$(sha256sum "$phase_timings_evidence")" || return 1
  sha_before="${sha_before%% *}"
  [[ "$sha_before" =~ ^[0-9a-f]{64}$ ]] || return 1
  jq -e --arg run_id "$run_id" --arg journey "$journey" \
    --arg schedule "$schedule" --arg asset_mode "$asset_mode" \
    --argjson expected "$expected" '
    def exact_keys($expected_keys): (keys | sort) == ($expected_keys | sort);
    . as $packet
    | exact_keys(["schema_version","kind","result","run_id","journey","schedule",
      "asset_mode","coverage","clock","started_at_utc","completed_at_utc",
      "total_duration_ms","unattributed_duration_ms","phases",
      "private_material_disclosed"])
    and .schema_version == 1
    and .kind == "m3_monotonic_phase_timings"
    and .result == "execution_passed_pre_cleanup"
    and .run_id == $run_id and .journey == $journey
    and .schedule == $schedule and .asset_mode == $asset_mode
    and .coverage == {
      starts_after_run_directory_initialization:true,
      ends_before_run_evidence_publication:true,
      cleanup_in_separate_attestation:true
    }
    and .clock == {
      source:"linux_proc_uptime",
      unit:"milliseconds",
      resolution_ms:10,
      includes_suspend:true,
      wall_clock_used_for_duration:false
    }
    and (.started_at_utc | type) == "string"
    and (.started_at_utc | test("^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$"))
    and (try (.started_at_utc | fromdateiso8601 | type == "number") catch false)
    and (.completed_at_utc | type) == "string"
    and (.completed_at_utc | test("^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$"))
    and (try (.completed_at_utc | fromdateiso8601 | type == "number") catch false)
    and (.total_duration_ms | type) == "number"
    and .total_duration_ms == (.total_duration_ms | floor)
    and .total_duration_ms >= 0
    and (.unattributed_duration_ms | type) == "number"
    and .unattributed_duration_ms == (.unattributed_duration_ms | floor)
    and .unattributed_duration_ms >= 0
    and .private_material_disclosed == false
    and (.phases | type) == "array"
    and (.phases | length) == ($expected | length)
    and [.phases[].sequence] == [range(1; ($expected | length) + 1)]
    and [.phases[].phase_id] == [$expected[].phase_id]
    and [.phases[].direction] == [$expected[].direction]
    and all(.phases[];
      exact_keys(["schema_version","sequence","producer","phase_id","direction",
        "start_offset_ms","end_offset_ms","duration_ms","outcome"])
      and .schema_version == 1 and .producer == "outer" and .outcome == "passed"
      and (.sequence | type) == "number" and .sequence == (.sequence | floor)
      and (.phase_id | type) == "string"
      and ((.direction == null) or (.direction | type) == "string")
      and (.start_offset_ms | type) == "number"
      and .start_offset_ms == (.start_offset_ms | floor)
      and (.end_offset_ms | type) == "number"
      and .end_offset_ms == (.end_offset_ms | floor)
      and (.duration_ms | type) == "number"
      and .duration_ms == (.duration_ms | floor)
      and .start_offset_ms >= 0
      and .end_offset_ms >= .start_offset_ms
      and .duration_ms == (.end_offset_ms - .start_offset_ms)
      and .end_offset_ms <= $packet.total_duration_ms)
    and all(range(1; (.phases | length));
      . as $index
      | $packet.phases[$index].start_offset_ms >=
        $packet.phases[$index - 1].end_offset_ms)
    and .unattributed_duration_ms ==
      (.total_duration_ms - ([.phases[].duration_ms] | add // 0))
  ' "$phase_timings_evidence" >/dev/null || return 1
  summary="$(jq -ce \
    --arg evidence_path "${relative_run_root}/evidence/m3-phase-timings.json" \
    --arg evidence_sha "$sha_before" '
    {
      kind:.kind,
      result:.result,
      evidence_path:$evidence_path,
      evidence_sha256:$evidence_sha,
      clock:.clock,
      coverage:.coverage,
      total_duration_ms:.total_duration_ms,
      unattributed_duration_ms:.unattributed_duration_ms,
      phase_count:(.phases | length)
    }
  ' "$phase_timings_evidence")" || return 1
  sha_after="$(sha256sum "$phase_timings_evidence")" || return 1
  sha_after="${sha_after%% *}"
  [[ "$sha_after" == "$sha_before" ]] || return 1
  phase_timing_sha="$sha_before"
  printf -v "$output_name" '%s' "$summary"
}

expected_actor_direction_phase_ids_json() {
  local direction="$1"
  [[ "$direction" == "taker_sells_foreign" ||
     "$direction" == "taker_sells_lez" ]] || return 1
  [[ "$asset_mode" == "native" || "$asset_mode" == "custom_token" ]] || return 1
  if [[ "$schedule" == "overlap" ]]; then
    [[ "$journey" == "claim" ]] || return 1
    jq -cn '[
      "final_transcript",
      "presign_and_activate",
      "overlap_ready_barrier",
      "first_lock_to_revision_one",
      "second_lock_to_revision_two",
      "dual_lock_gate",
      "overlap_locked_barrier",
      "revealing_claim_to_revision_three",
      "followup_claim_to_revision_four",
      "terminal_evidence",
      "overlap_terminal_marker"
    ]'
    return
  fi
  [[ "$schedule" == "sequential" ]] || return 1
  case "$journey" in
    claim)
      jq -cn '[
        "final_transcript",
        "presign_and_activate",
        "first_lock_to_revision_one",
        "second_lock_to_revision_two",
        "dual_lock_gate",
        "revealing_claim_to_revision_three",
        "followup_claim_to_revision_four",
        "terminal_evidence"
      ]'
      ;;
    survivor_claim)
      jq -cn '[
        "final_transcript",
        "presign_and_activate",
        "first_lock_to_revision_one",
        "second_lock_to_revision_two",
        "dual_lock_gate",
        "survivor_settlement_to_revision_four",
        "terminal_evidence"
      ]'
      ;;
    refund)
      jq -cn '[
        "final_transcript",
        "presign_and_activate",
        "first_lock_to_revision_one",
        "second_lock_to_revision_two",
        "dual_lock_gate",
        "refund_settlement_to_revision_four",
        "terminal_evidence"
      ]'
      ;;
    first_lock_refund)
      jq -cn '[
        "final_transcript",
        "presign_and_activate",
        "first_lock_refund_to_revision_two",
        "terminal_evidence"
      ]'
      ;;
    *) return 1 ;;
  esac
}

validate_actor_direction_phase_timing_for_run_evidence() {
  local direction="$1" output_name="$2" expected_parent_sha="$3"
  local child_path effects_path expected_phase_ids expected_execution_mode
  local parent_phase_id parent_duration parent_fd parent_fd_path
  local parent_sha_before parent_sha_after child_sha_before child_sha_after
  local effects_sha_before effects_sha_after summary
  [[ "$direction" == "taker_sells_foreign" ||
     "$direction" == "taker_sells_lez" ]] || return 1
  [[ "$output_name" =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]] || return 1
  [[ "$expected_parent_sha" =~ ^[0-9a-f]{64}$ ]] || return 1
  child_path="${evidence_dir}/${direction}-actor-phase-timings.json"
  effects_path="${evidence_dir}/${direction}-actual-effects.json"
  [[ -f "$child_path" && ! -L "$child_path" &&
     "$(stat -c '%u:%a' "$child_path")" == "$(id -u):600" &&
     -f "$effects_path" && ! -L "$effects_path" &&
     "$(stat -c '%u:%a' "$effects_path")" == "$(id -u):600" &&
     -f "$phase_timings_evidence" && ! -L "$phase_timings_evidence" &&
     "$(stat -c '%u:%a' "$phase_timings_evidence")" == "$(id -u):600" ]] ||
    return 1
  expected_phase_ids="$(expected_actor_direction_phase_ids_json "$direction")" ||
    return 1
  if [[ "$schedule" == "overlap" ]]; then
    expected_execution_mode="overlap"
    parent_phase_id="directions_overlap"
  else
    expected_execution_mode="sequential"
    parent_phase_id="direction_${direction}_actor_flow"
  fi
  exec {parent_fd}<"$phase_timings_evidence" || return 1
  parent_fd_path="/proc/$$/fd/${parent_fd}"
  parent_sha_before="$(sha256sum "$parent_fd_path")" || {
    exec {parent_fd}<&-
    return 1
  }
  parent_sha_before="${parent_sha_before%% *}"
  if [[ "$parent_sha_before" != "$expected_parent_sha" ]]; then
    exec {parent_fd}<&-
    return 1
  fi
  if ! parent_duration="$(jq -er --arg run_id "$run_id" --arg journey "$journey" \
      --arg schedule "$schedule" --arg asset_mode "$asset_mode" \
      --arg phase_id "$parent_phase_id" '
    . as $packet
    | ($packet.schema_version == 1
    and $packet.kind == "m3_monotonic_phase_timings"
    and $packet.run_id == $run_id and $packet.journey == $journey
    and $packet.schedule == $schedule and $packet.asset_mode == $asset_mode
    and ($packet.phases | type) == "array"
    and ([$packet.phases[] | select(.phase_id == $phase_id)] | length) == 1
    and ([$packet.phases[] | select(.phase_id == $phase_id)][0].duration_ms | type) ==
      "number"
    and ([$packet.phases[] | select(.phase_id == $phase_id)][0].duration_ms | floor) ==
      [$packet.phases[] | select(.phase_id == $phase_id)][0].duration_ms
    and [$packet.phases[] | select(.phase_id == $phase_id)][0].duration_ms >= 0)
    | if . then
        [$packet.phases[] | select(.phase_id == $phase_id)][0].duration_ms
      else empty end
  ' "$parent_fd_path")"; then
    exec {parent_fd}<&-
    return 1
  fi
  parent_sha_after="$(sha256sum "$parent_fd_path")" || {
    exec {parent_fd}<&-
    return 1
  }
  parent_sha_after="${parent_sha_after%% *}"
  exec {parent_fd}<&-
  [[ "$parent_sha_after" == "$parent_sha_before" &&
     "$parent_sha_after" == "$expected_parent_sha" ]] || return 1
  [[ "$parent_duration" =~ ^(0|[1-9][0-9]*)$ ]] || return 1
  child_sha_before="$(sha256sum "$child_path")" || return 1
  child_sha_before="${child_sha_before%% *}"
  effects_sha_before="$(sha256sum "$effects_path")" || return 1
  effects_sha_before="${effects_sha_before%% *}"
  [[ "$child_sha_before" =~ ^[0-9a-f]{64}$ &&
     "$effects_sha_before" =~ ^[0-9a-f]{64}$ ]] || return 1
  jq -e --arg run_id "$run_id" --arg direction "$direction" \
    --arg journey "$journey" --arg asset_mode "$asset_mode" \
    --arg execution_mode "$expected_execution_mode" \
    --arg effects_sha "$effects_sha_before" \
    --argjson expected_phase_ids "$expected_phase_ids" \
    --argjson parent_duration "$parent_duration" '
    def exact_keys($allowed): (keys | sort) == ($allowed | sort);
    . as $packet
    | exact_keys(["schema_version","kind","result","run_id","direction","journey",
      "asset_mode","execution_mode","actual_effects_sha256","coverage","clock",
      "started_at_utc","completed_at_utc","total_duration_ms",
      "unattributed_duration_ms","phases","private_material_disclosed"])
    and .schema_version == 1
    and .kind == "m3_actor_direction_phase_timings"
    and .result == "actor_flow_passed"
    and .run_id == $run_id and .direction == $direction
    and .journey == $journey and .asset_mode == $asset_mode
    and .execution_mode == $execution_mode
    and .actual_effects_sha256 == $effects_sha
    and .coverage == {
      starts_before_final_transcript:true,
      ends_after_actual_effect_manifest:true,
      excludes_outer_stage_two_replay_and_balances:true
    }
    and .clock == {
      source:"linux_proc_uptime",
      unit:"milliseconds",
      resolution_ms:10,
      includes_suspend:true,
      wall_clock_used_for_duration:false
    }
    and (.started_at_utc | type) == "string"
    and (.started_at_utc |
      test("^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$"))
    and (try (.started_at_utc | fromdateiso8601 | type == "number") catch false)
    and (.completed_at_utc | type) == "string"
    and (.completed_at_utc |
      test("^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$"))
    and (try (.completed_at_utc | fromdateiso8601 | type == "number") catch false)
    and ((.completed_at_utc | fromdateiso8601) >=
      (.started_at_utc | fromdateiso8601))
    and (.total_duration_ms | type) == "number"
    and .total_duration_ms == (.total_duration_ms | floor)
    and .total_duration_ms >= 0 and .total_duration_ms <= $parent_duration
    and .total_duration_ms <= 9007199254740991
    and (.unattributed_duration_ms | type) == "number"
    and .unattributed_duration_ms == (.unattributed_duration_ms | floor)
    and .unattributed_duration_ms >= 0
    and .private_material_disclosed == false
    and (.phases | type) == "array"
    and [.phases[].phase_id] == $expected_phase_ids
    and [.phases[].sequence] == [range(1; ($expected_phase_ids | length) + 1)]
    and ([$packet.phases[] |
      exact_keys(["schema_version","sequence","producer","phase_id",
        "start_offset_ms","end_offset_ms","duration_ms","outcome"])
      and .schema_version == 1 and .producer == "direction_actor"
      and .outcome == "passed"
      and (.sequence | type) == "number" and .sequence == (.sequence | floor)
      and (.phase_id | type) == "string"
      and (.start_offset_ms | type) == "number"
      and .start_offset_ms == (.start_offset_ms | floor)
      and (.end_offset_ms | type) == "number"
      and .end_offset_ms == (.end_offset_ms | floor)
      and (.duration_ms | type) == "number"
      and .duration_ms == (.duration_ms | floor)
      and .start_offset_ms >= 0 and .end_offset_ms >= .start_offset_ms
      and .duration_ms == (.end_offset_ms - .start_offset_ms)
      and .end_offset_ms <= $packet.total_duration_ms
      and .end_offset_ms <= 9007199254740991] | all)
    and ([range(1; ($packet.phases | length)) as $index |
      $packet.phases[$index].start_offset_ms >=
        $packet.phases[$index - 1].end_offset_ms] | all)
    and .unattributed_duration_ms ==
      (.total_duration_ms - ([.phases[].duration_ms] | add // 0))
  ' "$child_path" >/dev/null || return 1
  jq -e --arg direction "$direction" '
    .schema_version == 1 and .direction == $direction
    and (.bitcoin_effect_ids | type) == "array"
    and (.lez_effect_ids | type) == "array"
    and (.expected_unique_effects | type) == "object"
  ' "$effects_path" >/dev/null || return 1
  summary="$(jq -ce \
    --arg evidence_path "${relative_run_root}/evidence/${direction}-actor-phase-timings.json" \
    --arg evidence_sha "$child_sha_before" \
    --arg effects_path "${relative_run_root}/evidence/${direction}-actual-effects.json" \
    --arg effects_sha "$effects_sha_before" \
    --arg parent_phase_id "$parent_phase_id" \
    --argjson parent_duration "$parent_duration" '
    {
      kind:.kind,
      result:.result,
      direction:.direction,
      journey:.journey,
      asset_mode:.asset_mode,
      execution_mode:.execution_mode,
      evidence_path:$evidence_path,
      evidence_sha256:$evidence_sha,
      actual_effects_path:$effects_path,
      actual_effects_sha256:$effects_sha,
      clock:.clock,
      coverage:.coverage,
      total_duration_ms:.total_duration_ms,
      unattributed_duration_ms:.unattributed_duration_ms,
      phase_count:(.phases | length),
      parent:{
        phase_id:$parent_phase_id,
        duration_ms:$parent_duration,
        contains_child:true,
        residual_ms:($parent_duration - .total_duration_ms)
      }
    }
  ' "$child_path")" || return 1
  child_sha_after="$(sha256sum "$child_path")" || return 1
  child_sha_after="${child_sha_after%% *}"
  effects_sha_after="$(sha256sum "$effects_path")" || return 1
  effects_sha_after="${effects_sha_after%% *}"
  [[ "$child_sha_after" == "$child_sha_before" &&
     "$effects_sha_after" == "$effects_sha_before" ]] || return 1
  printf -v "$output_name" '%s' "$summary"
}

actor_direction_phase_timings_hash_stable() {
  local expected="$1" direction child_path effects_path
  local expected_child_sha expected_effects_sha child_sha effects_sha
  jq -e --arg m5_btc_application_mode "$m5_btc_application_mode" '
    (keys | sort) ==
      (if $m5_btc_application_mode == "1" then ["taker_sells_foreign"]
       else (["taker_sells_foreign","taker_sells_lez"] | sort) end)
    and .taker_sells_foreign.direction == "taker_sells_foreign"
    and (if $m5_btc_application_mode == "1" then has("taker_sells_lez") | not
         else .taker_sells_lez.direction == "taker_sells_lez" end)
    and ([.[] |
      (.evidence_sha256 | type) == "string"
      and (.evidence_sha256 | test("^[0-9a-f]{64}$"))
      and (.actual_effects_sha256 | type) == "string"
      and (.actual_effects_sha256 | test("^[0-9a-f]{64}$"))] | all)
  ' <<<"$expected" >/dev/null || return 1
  for direction in "${directions[@]}"; do
    child_path="${evidence_dir}/${direction}-actor-phase-timings.json"
    effects_path="${evidence_dir}/${direction}-actual-effects.json"
    [[ -f "$child_path" && ! -L "$child_path" &&
       "$(stat -c '%u:%a' "$child_path")" == "$(id -u):600" &&
       -f "$effects_path" && ! -L "$effects_path" &&
       "$(stat -c '%u:%a' "$effects_path")" == "$(id -u):600" ]] || return 1
    expected_child_sha="$(jq -er --arg direction "$direction" \
      '.[$direction].evidence_sha256' <<<"$expected")" || return 1
    expected_effects_sha="$(jq -er --arg direction "$direction" \
      '.[$direction].actual_effects_sha256' <<<"$expected")" || return 1
    child_sha="$(sha256sum "$child_path")" || return 1
    child_sha="${child_sha%% *}"
    effects_sha="$(sha256sum "$effects_path")" || return 1
    effects_sha="${effects_sha%% *}"
    [[ "$child_sha" == "$expected_child_sha" &&
       "$effects_sha" == "$expected_effects_sha" ]] || return 1
  done
}

for path in "$run_root" ".e2e/${bitcoin_run_id}" ".e2e/${lez_run_id}" \
  "$secure_state_root"; do
  [[ ! -e "$path" && ! -L "$path" ]] || fail "refusing to reuse run state: ${path}"
done

for child_run in "$bitcoin_run_id" "$lez_run_id"; do
  child_containers="$(docker container ls --all --quiet \
    --filter "label=org.logos-co.atomic-swaps.run=${child_run}")" ||
    fail "Docker container collision query failed for child run ${child_run}"
  child_networks="$(docker network ls --quiet \
    --filter "label=org.logos-co.atomic-swaps.run=${child_run}")" ||
    fail "Docker network collision query failed for child run ${child_run}"
  child_volumes="$(docker volume ls --quiet \
    --filter "label=org.logos-co.atomic-swaps.run=${child_run}")" ||
    fail "Docker volume collision query failed for child run ${child_run}"
  child_images="$(docker image ls --quiet \
    --filter "label=org.logos-co.atomic-swaps.run=${child_run}")" ||
    fail "Docker image collision query failed for child run ${child_run}"
  if [[ -n "$child_containers" || -n "$child_networks" ||
        -n "$child_volumes" || -n "$child_images" ]]; then
    fail "refusing to reuse Docker resources for child run ${child_run}"
  fi
done
unset child_run child_containers child_networks child_volumes child_images

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

process_group_matches_registry() {
  local pid="$1"
  local expected_start="$2"
  local expected_executable="$3"
  local expected_pgid="$4"
  local expected_sid="$5"
  local actual_pgid actual_sid
  process_matches_registry "$pid" "$expected_start" "$expected_executable" || return 1
  [[ "$expected_pgid" =~ ^[1-9][0-9]*$ && "$expected_sid" =~ ^[1-9][0-9]*$ ]] ||
    return 1
  actual_pgid="$(awk '{print $5}' "/proc/${pid}/stat" 2>/dev/null || true)"
  actual_sid="$(awk '{print $6}' "/proc/${pid}/stat" 2>/dev/null || true)"
  [[ "$actual_pgid" == "$expected_pgid" && "$actual_pgid" == "$pid" &&
     "$actual_sid" == "$expected_sid" && "$actual_sid" == "$pid" ]]
}

register_owned_process() {
  local role="$1"
  local phase="$2"
  local pid="$3"
  local expected_executable="$4"
  local group_owned="${5:-false}"
  local reap_child="${6:-false}"
  local start_variable="${7:-}" ppid_variable="${8:-}" executable_variable="${9:-}"
  local pgid_variable="${10:-}" sid_variable="${11:-}" output_variable
  local expected_parent_pid="${12:-$$}"
  local process_fields state
  local first_observed_ppid first_observed_pgid first_observed_sid first_observed_start
  local current_ppid current_pgid current_sid current_start current_executable
  [[ "$pid" =~ ^[1-9][0-9]*$ && "$expected_executable" == /* ]] || return 1
  [[ "$group_owned" == true || "$group_owned" == false ]] || return 1
  [[ "$reap_child" == true || "$reap_child" == false ]] || return 1
  [[ "$expected_parent_pid" =~ ^[1-9][0-9]*$ ]] || return 1
  for output_variable in "$start_variable" "$ppid_variable" "$executable_variable" \
    "$pgid_variable" "$sid_variable"; do
    [[ -z "$output_variable" ||
       "$output_variable" =~ ^[a-zA-Z_][a-zA-Z0-9_]*$ ]] || return 1
  done
  [[ -r "/proc/${pid}/stat" ]] || return 1
  process_fields="$(awk '{print $3, $4, $5, $6, $22}' \
    "/proc/${pid}/stat" 2>/dev/null)" || return 1
  read -r state first_observed_ppid first_observed_pgid first_observed_sid \
    first_observed_start <<<"$process_fields"
  [[ "$state" != Z && "$first_observed_ppid" == "$expected_parent_pid" &&
     -n "$first_observed_start" ]] || return 1
  current_executable="$(readlink -f "/proc/${pid}/exe" 2>/dev/null || true)"
  [[ -z "$start_variable" ]] || printf -v "$start_variable" '%s' "$first_observed_start"
  [[ -z "$ppid_variable" ]] || printf -v "$ppid_variable" '%s' "$first_observed_ppid"
  [[ -z "$executable_variable" ]] ||
    printf -v "$executable_variable" '%s' "$current_executable"
  [[ -z "$pgid_variable" ]] || printf -v "$pgid_variable" '%s' "$first_observed_pgid"
  [[ -z "$sid_variable" ]] || printf -v "$sid_variable" '%s' "$first_observed_sid"
  for _ in {1..200}; do
    [[ -r "/proc/${pid}/stat" ]] || return 1
    process_fields="$(awk '{print $3, $4, $5, $6, $22}' \
      "/proc/${pid}/stat" 2>/dev/null)" || return 1
    read -r state current_ppid current_pgid current_sid current_start <<<"$process_fields"
    [[ "$state" != Z && "$current_start" == "$first_observed_start" &&
       "$current_ppid" == "$first_observed_ppid" &&
       "$first_observed_ppid" == "$expected_parent_pid" ]] || return 1
    current_executable="$(readlink -f "/proc/${pid}/exe" 2>/dev/null)" || return 1
    [[ -z "$executable_variable" ]] ||
      printf -v "$executable_variable" '%s' "$current_executable"
    [[ -z "$pgid_variable" ]] || printf -v "$pgid_variable" '%s' "$current_pgid"
    [[ -z "$sid_variable" ]] || printf -v "$sid_variable" '%s' "$current_sid"
    if [[ "$current_executable" == "$expected_executable" &&
          ("$group_owned" == false ||
           ("$current_pgid" == "$pid" && "$current_sid" == "$pid")) ]]; then
      break
    fi
    sleep 0.01
  done
  [[ "$state" != Z && "$current_start" == "$first_observed_start" &&
     "$current_ppid" == "$first_observed_ppid" &&
     "$current_executable" == "$expected_executable" ]] || return 1
  if [[ "$group_owned" == true ]]; then
    [[ "$current_pgid" == "$pid" && "$current_sid" == "$pid" ]] || return 1
    process_group_matches_registry "$pid" "$first_observed_start" \
      "$current_executable" "$current_pgid" "$current_sid" ||
      return 1
  else
    process_matches_registry "$pid" "$first_observed_start" "$current_executable" || return 1
  fi
  jq -nc --arg role "$role" --arg phase "$phase" --argjson pid "$pid" \
    --arg start "$first_observed_start" --arg executable "$current_executable" \
    --argjson ppid "$first_observed_ppid" --argjson pgid "$current_pgid" \
    --argjson sid "$current_sid" \
    --argjson group_owned "$group_owned" --argjson reap_child "$reap_child" \
    '{role:$role,phase:$phase,pid:$pid,start_ticks:$start,executable:$executable,
      ppid:$ppid,pgid:$pgid,sid:$sid,group_owned:$group_owned,reap_child:$reap_child}' \
    >>"$process_registry"
}

stop_provisional_owned_process() {
  local pid="$1" expected_start="$2" expected_ppid="$3" expected_executable="$4"
  local expected_pgid="$5" expected_sid="$6"
  local fields state current_ppid current_pgid current_sid current_start current_executable
  local wait_status
  local trusted_group=false
  [[ "$pid" =~ ^[1-9][0-9]*$ ]] || return 1
  if [[ -z "$expected_start" || -z "$expected_ppid" ]]; then
    [[ ! -e "/proc/${pid}" ]] || return 1
    wait "$pid" 2>/dev/null
    wait_status=$?
    [[ "$wait_status" != 127 ]]
    return
  fi
  [[ "$expected_start" =~ ^[0-9]+$ && "$expected_ppid" =~ ^[1-9][0-9]*$ ]] ||
    return 1
  if [[ "$expected_executable" == /* && "$expected_pgid" == "$pid" &&
        "$expected_sid" == "$pid" ]]; then
    trusted_group=true
  fi
  if [[ -r "/proc/${pid}/stat" ]]; then
    fields="$(awk '{print $3, $4, $5, $6, $22}' \
      "/proc/${pid}/stat" 2>/dev/null)" || return 1
    read -r state current_ppid current_pgid current_sid current_start <<<"$fields"
    [[ "$current_start" == "$expected_start" && "$current_ppid" == "$expected_ppid" ]] ||
      return 1
    if [[ "$state" != Z ]]; then
      current_executable="$(readlink -f "/proc/${pid}/exe" 2>/dev/null)" || return 1
      [[ "$expected_executable" == /* &&
         "$current_executable" == "$expected_executable" ]] || return 1
    fi
    if [[ "$trusted_group" == true ]]; then
      [[ "$current_pgid" == "$expected_pgid" &&
         "$current_sid" == "$expected_sid" ]] || return 1
      stop_trusted_process_group_members "$expected_pgid" "$expected_sid" || return 1
    elif [[ "$state" != Z ]]; then
      kill -TERM "$pid" 2>/dev/null || return 1
      for _ in {1..100}; do
        [[ -r "/proc/${pid}/stat" ]] || break
        fields="$(awk '{print $3, $4, $22}' "/proc/${pid}/stat" 2>/dev/null)" ||
          return 1
        read -r state current_ppid current_start <<<"$fields"
        [[ "$current_start" == "$expected_start" &&
           "$current_ppid" == "$expected_ppid" ]] || return 1
        [[ "$state" == Z ]] && break
        sleep 0.05
      done
      if [[ -r "/proc/${pid}/stat" && "$state" != Z ]]; then
        kill -KILL "$pid" 2>/dev/null || return 1
      fi
    fi
  fi
  if wait "$pid" 2>/dev/null; then wait_status=0; else wait_status=$?; fi
  [[ "$wait_status" != 127 ]] || return 1
  [[ "$trusted_group" == false ]] ||
    stop_trusted_process_group_members "$expected_pgid" "$expected_sid"
}

process_group_has_live_members() {
  local expected_pgid="$1"
  local expected_sid="$2"
  local processes
  [[ "$expected_pgid" =~ ^[1-9][0-9]*$ && "$expected_sid" =~ ^[1-9][0-9]*$ ]] ||
    return 1
  processes="$(ps -eo stat=,pgid=,sid=)" || return 2
  awk -v expected_pgid="$expected_pgid" -v expected_sid="$expected_sid" '
    $1 !~ /^Z/ && $2 == expected_pgid && $3 == expected_sid { found=1 }
    END { exit(found ? 0 : 1) }
  ' <<<"$processes"
}

process_group_anchor_matches_registry() {
  local pid="$1"
  local expected_start="$2"
  local expected_executable="$3"
  local expected_pgid="$4"
  local expected_sid="$5"
  local fields state pgid sid start executable
  [[ "$pid" =~ ^[1-9][0-9]*$ && -r "/proc/${pid}/stat" ]] || return 1
  fields="$(awk '{print $3, $5, $6, $22}' "/proc/${pid}/stat" 2>/dev/null || true)"
  read -r state pgid sid start <<<"$fields"
  [[ "$start" == "$expected_start" && "$pgid" == "$expected_pgid" &&
     "$sid" == "$expected_sid" && "$pgid" == "$pid" && "$sid" == "$pid" ]] ||
    return 1
  if [[ "$state" != Z ]]; then
    executable="$(readlink -f "/proc/${pid}/exe" 2>/dev/null || true)"
    [[ "$executable" == "$expected_executable" ]] || return 1
  fi
}

stop_trusted_process_group_members() {
  local pgid="$1"
  local sid="$2"
  local membership_status=0
  process_group_has_live_members "$pgid" "$sid" || membership_status=$?
  [[ "$membership_status" != 2 ]] || return 1
  if [[ "$membership_status" == 0 ]]; then
    if ! kill -TERM -- "-$pgid" 2>/dev/null; then
      membership_status=0
      process_group_has_live_members "$pgid" "$sid" || membership_status=$?
      [[ "$membership_status" == 1 ]] || return 1
    fi
  fi
  for _ in {1..100}; do
    membership_status=0
    process_group_has_live_members "$pgid" "$sid" || membership_status=$?
    [[ "$membership_status" != 2 ]] || return 1
    [[ "$membership_status" == 0 ]] || break
    sleep 0.05
  done
  if [[ "$membership_status" == 0 ]]; then
    if ! kill -KILL -- "-$pgid" 2>/dev/null; then
      membership_status=0
      process_group_has_live_members "$pgid" "$sid" || membership_status=$?
      [[ "$membership_status" == 1 ]] || return 1
    fi
  fi
  for _ in {1..100}; do
    membership_status=0
    process_group_has_live_members "$pgid" "$sid" || membership_status=$?
    [[ "$membership_status" != 2 ]] || return 1
    [[ "$membership_status" == 0 ]] || break
    sleep 0.05
  done
  [[ "$membership_status" == 1 ]]
}

stop_owned_processes() {
  local record pid start executable pgid sid group_owned reap_child state wait_status
  local cleanup_failed=0 was_present=false
  [[ -f "$process_registry" ]] || return 0
  while IFS= read -r record; do
    [[ -n "$record" ]] || continue
    pid="$(jq -er '.pid | numbers' <<<"$record" 2>/dev/null)" || { cleanup_failed=1; continue; }
    start="$(jq -er '.start_ticks | strings' <<<"$record" 2>/dev/null)" || { cleanup_failed=1; continue; }
    executable="$(jq -er '.executable | strings' <<<"$record" 2>/dev/null)" || { cleanup_failed=1; continue; }
    pgid="$(jq -er '(.pgid // .pid) | numbers' <<<"$record" 2>/dev/null)" || { cleanup_failed=1; continue; }
    sid="$(jq -er '(.sid // .pid) | numbers' <<<"$record" 2>/dev/null)" || { cleanup_failed=1; continue; }
    group_owned="$(jq -er '(.group_owned // false) | booleans | tostring' \
      <<<"$record" 2>/dev/null)" || { cleanup_failed=1; continue; }
    reap_child="$(jq -er '(.reap_child // false) | booleans | tostring' \
      <<<"$record" 2>/dev/null)" || { cleanup_failed=1; continue; }
    [[ -n "$pid" && -n "$start" && "$executable" == /* ]] || { cleanup_failed=1; continue; }
    was_present=false
    if [[ "$group_owned" == true ]] &&
       process_group_anchor_matches_registry "$pid" "$start" "$executable" "$pgid" "$sid"; then
      was_present=true
      stop_trusted_process_group_members "$pgid" "$sid" || cleanup_failed=1
    elif [[ "$group_owned" == false ]] &&
         process_matches_registry "$pid" "$start" "$executable"; then
      was_present=true
      kill -TERM "$pid" 2>/dev/null || cleanup_failed=1
    fi
    if [[ "$was_present" == true && "$group_owned" == false ]]; then
      for _ in {1..100}; do
        process_matches_registry "$pid" "$start" "$executable" || break
        state="$(awk '{print $3}' "/proc/${pid}/stat" 2>/dev/null || true)"
        [[ "$state" == Z ]] && break
        sleep 0.05
      done
    fi
    state="$(awk '{print $3}' "/proc/${pid}/stat" 2>/dev/null || true)"
    if [[ "$state" != Z && "$group_owned" == false ]] &&
         process_matches_registry "$pid" "$start" "$executable"; then
      kill -KILL "$pid" 2>/dev/null || cleanup_failed=1
    fi
    if [[ "$was_present" == true && "$reap_child" == true ]]; then
      if wait "$pid" 2>/dev/null; then
        wait_status=0
      else
        wait_status=$?
      fi
      [[ "$wait_status" != 127 ]] || cleanup_failed=1
    fi
  done <"$process_registry"
  return "$cleanup_failed"
}

assert_exact_owned_resource() {
  local kind="$1"
  local resource="$2"
  local expected_run="$3"
  local expected_scope="$4"
  local expected_component="$5"
  local identity
  case "$kind" in
    container)
      identity="$(docker container inspect --format \
        '{{ index .Config.Labels "org.logos-co.atomic-swaps.run" }}|{{ index .Config.Labels "org.logos-co.atomic-swaps.scope" }}|{{ index .Config.Labels "org.logos-co.atomic-swaps.component" }}' \
        "$resource")" || return 1
      ;;
    image)
      identity="$(docker image inspect --format \
        '{{ index .Config.Labels "org.logos-co.atomic-swaps.run" }}|{{ index .Config.Labels "org.logos-co.atomic-swaps.scope" }}|{{ index .Config.Labels "org.logos-co.atomic-swaps.component" }}' \
        "$resource")" || return 1
      ;;
    network)
      identity="$(docker network inspect --format \
        '{{ index .Labels "org.logos-co.atomic-swaps.run" }}|{{ index .Labels "org.logos-co.atomic-swaps.scope" }}|{{ index .Labels "org.logos-co.atomic-swaps.component" }}' \
        "$resource")" || return 1
      ;;
    volume)
      identity="$(docker volume inspect --format \
        '{{ index .Labels "org.logos-co.atomic-swaps.run" }}|{{ index .Labels "org.logos-co.atomic-swaps.scope" }}|{{ index .Labels "org.logos-co.atomic-swaps.component" }}' \
        "$resource")" || return 1
      ;;
    *) return 1 ;;
  esac
  [[ "$identity" == "${expected_run}|${expected_scope}|${expected_component}" ]]
}

single_owned_container_id() {
  local ids_file="$1"
  local expected_component="$2"
  local container_id record byte_count
  [[ "$expected_component" =~ ^[a-z0-9][a-z0-9.-]*$ ]] || return 1
  [[ -f "$ids_file" && ! -L "$ids_file" ]] || return 1
  [[ "$(stat -c '%u' "$ids_file")" == "$(id -u)" &&
     "$(stat -c '%a' "$ids_file")" == 600 ]] || return 1
  record="$(<"$ids_file")"
  byte_count="$(wc -c <"$ids_file")" || return 1
  [[ "$byte_count" =~ ^[1-9][0-9]*$ &&
     "$byte_count" == $(( ${#record} + 1 )) ]] || return 1
  container_id="${record%%$'\t'*}"
  [[ "$container_id" =~ ^([0-9a-f]{12}|[0-9a-f]{64})$ &&
     "$record" == "$container_id"$'\t'"$expected_component" ]] || return 1
  printf '%s\n' "$container_id"
}

remove_exact_container_file() {
  local ids_file="$1"
  local expected_run="$2"
  local expected_scope="$3"
  local container_id component extra
  [[ -f "$ids_file" ]] || return 0
  while IFS=$'\t' read -r container_id component extra; do
    [[ -n "$container_id" && -n "$component" && -z "$extra" ]] || return 1
    if docker container inspect "$container_id" >/dev/null 2>&1; then
      assert_exact_owned_resource container "$container_id" "$expected_run" \
        "$expected_scope" "$component" || return 1
      docker container rm --force "$container_id" >/dev/null || return 1
    fi
  done <"$ids_file"
}

remove_exact_resource() {
  local kind="$1"
  local resource="$2"
  local expected_run="$3"
  local expected_scope="$4"
  local expected_component="$5"
  if ! docker "$kind" inspect "$resource" >/dev/null 2>&1; then
    return 0
  fi
  assert_exact_owned_resource "$kind" "$resource" "$expected_run" \
    "$expected_scope" "$expected_component" || return 1
  docker "$kind" rm "$resource" >/dev/null
}

collect_owned_containers() {
  local child_run="$1"
  local expected_scope="$2"
  local outcome="$3"
  local output="$4"
  shift 4
  local listing="" container_id component expected_component identity
  local certification_failed=0 identity_query_failed=0 component_allowed
  local -a ids=()
  local -a expected_components=("$@")
  local -A seen=() component_counts=()
  [[ "$outcome" == passed || "$outcome" == failed ]] || return 2
  listing="$(docker container ls --all --quiet \
    --filter "label=org.logos-co.atomic-swaps.run=${child_run}")" || return 2
  if [[ -n "$listing" ]]; then
    mapfile -t ids <<<"$listing"
  fi
  for container_id in "${ids[@]}"; do
    if [[ -z "$container_id" || -n "${seen[$container_id]:-}" ]]; then
      certification_failed=1
      continue
    fi
    seen["$container_id"]=1
    identity="$(docker container inspect --format \
      '{{ index .Config.Labels "org.logos-co.atomic-swaps.run" }}|{{ index .Config.Labels "org.logos-co.atomic-swaps.scope" }}|{{ index .Config.Labels "org.logos-co.atomic-swaps.component" }}' \
      "$container_id")" || { identity_query_failed=1; continue; }
    IFS='|' read -r actual_run actual_scope component <<<"$identity"
    component_allowed=false
    for expected_component in "${expected_components[@]}"; do
      [[ "$component" == "$expected_component" ]] && component_allowed=true
    done
    if [[ "$actual_run" == "$child_run" && "$actual_scope" == "$expected_scope" &&
          "$component_allowed" == true ]]; then
      inventory_line="${container_id}"$'\t'"${component}"
      rg -Fxq -- "$inventory_line" "$output" 2>/dev/null ||
        printf '%s\n' "$inventory_line" >>"$output"
      component_counts["$component"]=$(( ${component_counts[$component]:-0} + 1 ))
    else
      certification_failed=1
    fi
  done
  for expected_component in "${expected_components[@]}"; do
    if [[ "$outcome" == passed && "${component_counts[$expected_component]:-0}" != 1 ]] ||
       [[ "$outcome" == failed && "${component_counts[$expected_component]:-0}" -gt 1 ]]; then
      certification_failed=1
    fi
  done
  [[ "$identity_query_failed" == 0 ]] || return 2
  [[ "$certification_failed" == 0 ]] || return 1
}

collect_owned_resources() {
  local kind="$1"
  local child_run="$2"
  local expected_scope="$3"
  local outcome="$4"
  local output="$5"
  shift 5
  local format listing="" resource component expected_component identity
  local certification_failed=0 identity_query_failed=0 component_allowed
  local -a resources=()
  local -a expected_components=("$@")
  local -A seen=() component_counts=()
  [[ "$outcome" == passed || "$outcome" == failed ]] || return 2
  case "$kind" in
    image) format='{{.Repository}}:{{.Tag}}' ;;
    network) format='{{.ID}}' ;;
    volume) format='{{.Name}}' ;;
    *) return 1 ;;
  esac
  listing="$(docker "$kind" ls --format "$format" \
    --filter "label=org.logos-co.atomic-swaps.run=${child_run}")" || return 2
  if [[ -n "$listing" ]]; then
    mapfile -t resources <<<"$listing"
  fi
  for resource in "${resources[@]}"; do
    if [[ -z "$resource" || "$resource" == '<none>:<none>' ||
          -n "${seen[$resource]:-}" ]]; then
      certification_failed=1
      continue
    fi
    seen["$resource"]=1
    case "$kind" in
      image)
        identity="$(docker image inspect --format \
          '{{ index .Config.Labels "org.logos-co.atomic-swaps.run" }}|{{ index .Config.Labels "org.logos-co.atomic-swaps.scope" }}|{{ index .Config.Labels "org.logos-co.atomic-swaps.component" }}' \
          "$resource")" || { identity_query_failed=1; continue; }
        ;;
      network | volume)
        identity="$(docker "$kind" inspect --format \
          '{{ index .Labels "org.logos-co.atomic-swaps.run" }}|{{ index .Labels "org.logos-co.atomic-swaps.scope" }}|{{ index .Labels "org.logos-co.atomic-swaps.component" }}' \
          "$resource")" || { identity_query_failed=1; continue; }
        ;;
    esac
    IFS='|' read -r actual_run actual_scope component <<<"$identity"
    component_allowed=false
    for expected_component in "${expected_components[@]}"; do
      [[ "$component" == "$expected_component" ]] && component_allowed=true
    done
    if [[ "$actual_run" == "$child_run" && "$actual_scope" == "$expected_scope" &&
          "$component_allowed" == true ]]; then
      inventory_line="${child_run}"$'\t'"${expected_scope}"$'\t'"${component}"$'\t'"${resource}"
      rg -Fxq -- "$inventory_line" "$output" 2>/dev/null ||
        printf '%s\n' "$inventory_line" >>"$output"
      component_counts["$component"]=$(( ${component_counts[$component]:-0} + 1 ))
    else
      certification_failed=1
    fi
  done
  for expected_component in "${expected_components[@]}"; do
    if [[ "$outcome" == passed && "${component_counts[$expected_component]:-0}" != 1 ]] ||
       [[ "$outcome" == failed && "${component_counts[$expected_component]:-0}" -gt 1 ]]; then
      certification_failed=1
    fi
  done
  [[ "$identity_query_failed" == 0 ]] || return 2
  [[ "$certification_failed" == 0 ]] || return 1
}

reconcile_node_resource_inventories() {
  local bitcoin_outcome="$1"
  local lez_outcome="$2"
  local reconciliation_failed=0
  local bitcoin_scope="bitcoin-core-regtest-e2e"
  local lez_scope="lez-v0.2-local-devnet"
  local bitcoin_containers_partial="${bitcoin_container_ids}.partial"
  local lez_containers_partial="${lez_container_ids}.partial"
  local networks_partial="${network_resources}.partial"
  local volumes_partial="${volume_resources}.partial"
  local images_partial="${image_resources}.partial"
  [[ "$bitcoin_outcome" == passed || "$bitcoin_outcome" == failed ]] || return 1
  [[ "$lez_outcome" == passed || "$lez_outcome" == failed ]] || return 1
  for inventory_pair in \
    "$bitcoin_container_ids:$bitcoin_containers_partial" \
    "$lez_container_ids:$lez_containers_partial" \
    "$network_resources:$networks_partial" \
    "$volume_resources:$volumes_partial" \
    "$image_resources:$images_partial"; do
    inventory_source="${inventory_pair%%:*}"
    inventory_target="${inventory_pair#*:}"
    : >"$inventory_target"
    if [[ -f "$inventory_source" ]]; then
      while IFS= read -r inventory_line; do
        [[ -n "$inventory_line" ]] && printf '%s\n' "$inventory_line" >>"$inventory_target"
      done <"$inventory_source"
    fi
    chmod 0600 "$inventory_target"
  done
  collect_owned_containers "$bitcoin_run_id" "$bitcoin_scope" "$bitcoin_outcome" \
    "$bitcoin_containers_partial" bitcoin-core || reconciliation_failed=1
  collect_owned_containers "$lez_run_id" "$lez_scope" "$lez_outcome" \
    "$lez_containers_partial" bedrock indexer sequencer || reconciliation_failed=1
  collect_owned_resources network "$bitcoin_run_id" "$bitcoin_scope" "$bitcoin_outcome" \
    "$networks_partial" bitcoin-core-network || reconciliation_failed=1
  collect_owned_resources network "$lez_run_id" "$lez_scope" "$lez_outcome" \
    "$networks_partial" lez-v0.2-network || reconciliation_failed=1
  collect_owned_resources volume "$bitcoin_run_id" "$bitcoin_scope" "$bitcoin_outcome" \
    "$volumes_partial" bitcoin-core-data || reconciliation_failed=1
  collect_owned_resources volume "$lez_run_id" "$lez_scope" "$lez_outcome" \
    "$volumes_partial" || reconciliation_failed=1
  collect_owned_resources image "$bitcoin_run_id" "$bitcoin_scope" "$bitcoin_outcome" \
    "$images_partial" bitcoin-core-image || reconciliation_failed=1
  collect_owned_resources image "$lez_run_id" "$lez_scope" "$lez_outcome" \
    "$images_partial" lez-v0.2-image || reconciliation_failed=1
  mv "$bitcoin_containers_partial" "$bitcoin_container_ids"
  mv "$lez_containers_partial" "$lez_container_ids"
  mv "$networks_partial" "$network_resources"
  mv "$volumes_partial" "$volume_resources"
  mv "$images_partial" "$image_resources"
  return "$reconciliation_failed"
}

remove_exact_resource_file() {
  local kind="$1"
  local resources_file="$2"
  local expected_run expected_scope expected_component resource extra
  [[ -f "$resources_file" ]] || return 0
  while IFS=$'\t' read -r expected_run expected_scope expected_component resource extra; do
    [[ -n "$expected_run" && -n "$expected_scope" && -n "$expected_component" &&
       -n "$resource" && -z "$extra" ]] || return 1
    remove_exact_resource "$kind" "$resource" "$expected_run" "$expected_scope" \
      "$expected_component" || return 1
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
    --filter "label=org.logos-co.atomic-swaps.run=${bitcoin_run_id}")" || return 1
  bitcoin_networks="$(docker network ls --quiet \
    --filter "label=org.logos-co.atomic-swaps.run=${bitcoin_run_id}")" || return 1
  bitcoin_volumes="$(docker volume ls --quiet \
    --filter "label=org.logos-co.atomic-swaps.run=${bitcoin_run_id}")" || return 1
  bitcoin_images="$(docker image ls --quiet \
    --filter "label=org.logos-co.atomic-swaps.run=${bitcoin_run_id}")" || return 1
  lez_containers="$(docker container ls --all --quiet \
    --filter "label=org.logos-co.atomic-swaps.run=${lez_run_id}")" || return 1
  lez_networks="$(docker network ls --quiet \
    --filter "label=org.logos-co.atomic-swaps.run=${lez_run_id}")" || return 1
  lez_volumes="$(docker volume ls --quiet \
    --filter "label=org.logos-co.atomic-swaps.run=${lez_run_id}")" || return 1
  lez_images="$(docker image ls --quiet \
    --filter "label=org.logos-co.atomic-swaps.run=${lez_run_id}")" || return 1
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
  local attestation_result
  trap - EXIT
  set +e

  stop_owned_processes || cleanup_failed=1
  reconcile_node_resource_inventories failed failed || cleanup_failed=1
  remove_exact_container_file "$bitcoin_container_ids" "$bitcoin_run_id" \
    bitcoin-core-regtest-e2e || cleanup_failed=1
  remove_exact_container_file "$lez_container_ids" "$lez_run_id" \
    lez-v0.2-local-devnet || cleanup_failed=1

  remove_exact_resource_file volume "$volume_resources" || cleanup_failed=1
  remove_exact_resource_file network "$network_resources" || cleanup_failed=1
  remove_exact_resource_file image "$image_resources" || cleanup_failed=1
  remove_secure_state_root || cleanup_failed=1

  if [[ "$cleanup_failed" == 0 ]]; then
    attestation_result=passed
  else
    attestation_result=failed
  fi
  if ! write_cleanup_attestation "$attestation_result"; then
    echo "M3 actor local PoC could not publish cleanup attestation" >&2
    cleanup_failed=1
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
initialize_phase_timings || fail "phase timing initialization failed"
mkdir -m 0700 "$secure_state_root"
mkdir -m 0700 "$secure_state_root/directions"

verify_direction_driver_contract() {
  local contract driver_sha
  driver_sha="$(sha256sum "$direction_driver" | sed 's/ .*//')"
  [[ "$driver_sha" =~ ^[0-9a-f]{64}$ ]] || fail "direction-driver SHA-256 is invalid"
  contract="$(M5_BTC_APPLICATION_MODE="$m5_btc_application_mode" \
    M3_POC_ASSET_MODE="$asset_mode" M3_POC_JOURNEY="$journey" \
    "$direction_driver" contract)" ||
    fail "direction-driver contract is unavailable"
  jq -e --arg journey "$journey" --arg asset_mode "$asset_mode" \
    --arg m5_btc_application_mode "$m5_btc_application_mode" '
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
      (if $m5_btc_application_mode == "1" then 6
       elif $asset_mode == "custom_token" then 5 else 4 end)
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
  contract="$(M5_BTC_APPLICATION_MODE="$m5_btc_application_mode" \
    M5_LEZ_DEPLOYER_SHA256="$expected_lez_deployer_sha256" \
    "$lez_bootstrap_driver" contract)" || fail "LEZ bootstrap contract is unavailable"
  jq -e --arg guest "$expected_lez_guest_sha256" \
    --arg program "$expected_lez_program_id" \
    --arg deployment_profile "$expected_lez_deployment_profile" '
    .schema_version == 1
    and .kind == "m3_lez_bootstrap_contract"
    and .verified_artifact_target_required == true
    and .embedded_guest_sha256 == $guest
    and .escrow_program_id == $program
    and .deployment_profile == $deployment_profile
    and .deployment_submission_count == 1
    and .fresh_identity_vault_claims == ["maker", "taker"]
    and .vault_claim_submission_count_per_role == 1
    and .public_rpc_used == false
    and .faucet_used == false
  ' <<<"$contract" >/dev/null || fail "LEZ bootstrap contract is incomplete"
}

prebuild() {
  local cache_partial
  echo "Prebuilding every M3 actor binary before service startup"
  if ! cargo +"$toolchain" build --locked --offline \
      -p btc-local-poc-provision -p btc-reference-actor -p lez-adaptor-role-runner --bins; then
    fail "offline M3 prebuild failed; populate the pinned Cargo cache before certification"
  fi
  if [[ "$m5_btc_application_mode" == 1 ]] &&
     ! cargo +"$toolchain" build --locked --offline -p lez-maker-node --bins; then
    fail "offline M5 BTC application prebuild failed; populate the pinned Cargo cache"
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
    for variable in M3_OFFICIAL_WALLET_CACHE_TEST_MODE \
      M3_OFFICIAL_WALLET_TEST_EXPECTED_COMMIT \
      M3_OFFICIAL_WALLET_TEST_EXPECTED_ORIGIN \
      M3_OFFICIAL_WALLET_TEST_WALLET_SHA256 \
      M3_OFFICIAL_WALLET_TEST_LIBRAPIDSNARK_A_SHA256 \
      M3_OFFICIAL_WALLET_TEST_LIBGMP_A_SHA256 \
      M3_OFFICIAL_WALLET_TEST_LIBFQ_A_SHA256 \
      M3_OFFICIAL_WALLET_TEST_LIBFR_A_SHA256; do
      [[ ! -v "$variable" ]] ||
        fail "official-wallet cache test override is forbidden in an actor run: $variable"
    done
    if ! cargo +"$toolchain" build --manifest-path compat/lez-v0_2-sidecar/Cargo.toml \
        --locked --offline --example lez-v02-account-codec; then
      fail "offline LEZ account-codec prebuild failed; populate the pinned Cargo cache"
    fi
    [[ -x "$official_wallet_cache_helper" &&
       ! -L "$official_wallet_cache_helper" ]] ||
      fail "official-wallet cache helper is unavailable"
    cache_partial="${official_wallet_cache_evidence}.partial"
    [[ ! -e "$cache_partial" && ! -L "$cache_partial" ]] ||
      fail "official-wallet cache evidence partial already exists"
    M3_OFFICIAL_WALLET_CACHE_ROOT="$official_wallet_cache_root" \
      M3_OFFICIAL_WALLET_DESTINATION="$official_wallet_bin" \
      LEZ_V02_SOURCE_DIR="$lez_source_dir" \
      M3_RUST_TOOLCHAIN="$toolchain" \
      "$official_wallet_cache_helper" prepare >"$cache_partial" ||
      fail "verified official-wallet artifact preparation failed"
    chmod 0600 "$cache_partial"
    jq -e '
      .schema_version == 1
      and .kind == "m3_official_wallet_artifact_preparation"
      and .result == "prepared"
      and (.cache_hit | type == "boolean")
      and .test_mode == false
      and .validation_policy_revision == 2
      and (.publisher_helper_sha256 | test("^[0-9a-f]{64}$"))
      and .input.validation_policy_revision == 2
      and .input.publisher_helper_sha256 == .publisher_helper_sha256
      and .input.build.expected_wallet_sha256 == .wallet_sha256
      and (.input.toolchain.target_libdir_sha256 | test("^[0-9a-f]{64}$"))
      and (.input_key | strings | test("^[0-9a-f]{64}$"))
      and (.wallet_sha256 | strings | test("^[0-9a-f]{64}$"))
      and (.object_manifest_sha256 | strings | test("^[0-9a-f]{64}$"))
      and (.runtime_fingerprint_sha256 | strings | test("^[0-9a-f]{64}$"))
      and (.duration_ms | numbers) >= 0
      and (.artifact_bytes | numbers) > 0
      and .private_copy == true
      and .hardlink == false
      and .source_rehashed_after_copy == true
      and .destination_rehashed == true
      and .secrets_or_state_cached == false
    ' "$cache_partial" >/dev/null ||
      fail "official-wallet artifact preparation evidence is invalid"
    [[ "$(jq -er '.wallet_sha256' "$cache_partial")" == \
       "$(sha256sum "$official_wallet_bin" | sed 's/ .*//')" ]] ||
      fail "official-wallet artifact evidence does not bind the private copy"
    mv "$cache_partial" "$official_wallet_cache_evidence"
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
readonly maker_daemon_bin="${repo_root}/target/debug/lez-maker-daemon"
readonly maker_cli_bin="${repo_root}/target/debug/lez-maker"
readonly taker_cli_bin="${repo_root}/target/debug/lez-taker"

assert_prebuilt() {
  local binary
  for binary in "$actor_bin" "$provisioner_bin" "$role_runner_bin" \
    "$core_fixture_bin" "$lez_operator_bin" "$sidecar_bin" "$vault_claim_bin" \
    "$native_escrow_bin" "$identity_bin" "$nssa_mapping_bin" "$lez_deployer"; do
    [[ -x "$binary" && ! -L "$binary" ]] || fail "prebuilt binary is missing: ${binary}"
  done
  if [[ "$m5_btc_application_mode" == 1 ]]; then
    for binary in "$maker_daemon_bin" "$maker_cli_bin" "$taker_cli_bin"; do
      [[ -x "$binary" && ! -L "$binary" ]] || fail "M5 BTC binary is missing: ${binary}"
    done
  fi
  if [[ "$asset_mode" == "custom_token" ]]; then
    [[ -x "$account_codec_bin" && ! -L "$account_codec_bin" ]] ||
      fail "prebuilt LEZ account codec is missing"
    [[ -x "$official_wallet_bin" && -f "$official_wallet_bin" &&
       ! -L "$official_wallet_bin" ]] || fail "official LEZ v0.2 wallet binary is missing"
    [[ "$(stat -c %a "$official_wallet_bin")" == 500 ]] ||
      fail "official LEZ v0.2 wallet binary has an unsafe mode"
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

wait_for_node_child() {
  local pid="$1"
  local status_variable="$2"
  local child_status
  [[ "$pid" =~ ^[1-9][0-9]*$ && "$status_variable" =~ ^[a-zA-Z_][a-zA-Z0-9_]*$ ]] ||
    return 1
  if wait "$pid"; then
    child_status=0
  else
    child_status=$?
  fi
  printf -v "$status_variable" '%s' "$child_status"
  [[ "$child_status" == 0 ]]
}

wait_for_node_children() {
  local bitcoin_pid="$1"
  local bitcoin_status_variable="$2"
  local bitcoin_pgid="$3"
  local bitcoin_sid="$4"
  local lez_pid="$5"
  local lez_status_variable="$6"
  local lez_pgid="$7"
  local lez_sid="$8"
  local groups_absent_variable="$9"
  local bitcoin_wait_ok=true lez_wait_ok=true groups_absent=true
  [[ "$groups_absent_variable" =~ ^[a-zA-Z_][a-zA-Z0-9_]*$ ]] || return 1
  wait_for_node_child "$bitcoin_pid" "$bitcoin_status_variable" || bitcoin_wait_ok=false
  stop_trusted_process_group_members "$bitcoin_pgid" "$bitcoin_sid" ||
    groups_absent=false
  wait_for_node_child "$lez_pid" "$lez_status_variable" || lez_wait_ok=false
  stop_trusted_process_group_members "$lez_pgid" "$lez_sid" || groups_absent=false
  printf -v "$groups_absent_variable" '%s' "$groups_absent"
  [[ "$bitcoin_wait_ok" == true && "$lez_wait_ok" == true &&
     "$groups_absent" == true ]]
}

start_actual_nodes() {
  local maker_account maker_vault taker_account taker_vault
  local bitcoin_pid lez_pid bitcoin_registered=true lez_registered=true bitcoin_status lez_status
  local bitcoin_start="" bitcoin_ppid="" bitcoin_executable="" bitcoin_pgid="" bitcoin_sid=""
  local lez_start="" lez_ppid="" lez_executable="" lez_pgid="" lez_sid=""
  local bitcoin_outcome lez_outcome inventory_reconciled=true
  local process_groups_absent=true
  local bash_executable bitcoin_log lez_log
  local owning_shell_pid="$BASHPID"
  local node_start_pending_signal=0
  maker_account="$(jq -er '.account_id' "${evidence_dir}/maker-lez-identity.json")"
  maker_vault="$(jq -er '.vault_account_id' "${evidence_dir}/maker-lez-identity.json")"
  taker_account="$(jq -er '.account_id' "${evidence_dir}/taker-lez-identity.json")"
  taker_vault="$(jq -er '.vault_account_id' "${evidence_dir}/taker-lez-identity.json")"
  bitcoin_log="${evidence_dir}/bitcoin-service.log"
  lez_log="${evidence_dir}/lez-service.log"
  for log_file in "$bitcoin_log" "$lez_log"; do
    [[ ! -e "$log_file" && ! -L "$log_file" ]] ||
      fail "refusing to reuse node-service log: ${log_file}"
    : >"$log_file"
    chmod 0600 "$log_file"
  done
  bash_executable="$(readlink -f "$(command -v bash)")"

  trap 'node_start_pending_signal=130' INT
  trap 'node_start_pending_signal=143' TERM
  setsid env RUN_ID="$bitcoin_run_id" BITCOIN_CORE_E2E_MODE=service \
    BITCOIN_CORE_E2E_KEEP_RUNNING=1 "$bitcoin_service_driver" \
    >"$bitcoin_log" 2>&1 &
  bitcoin_pid=$!
  register_owned_process node-bitcoin startup "$bitcoin_pid" "$bash_executable" true true \
    bitcoin_start bitcoin_ppid bitcoin_executable bitcoin_pgid bitcoin_sid "$owning_shell_pid" ||
    bitcoin_registered=false
  if [[ "$bitcoin_registered" == false ]]; then
    stop_provisional_owned_process "$bitcoin_pid" "$bitcoin_start" "$bitcoin_ppid" \
      "$bitcoin_executable" "$bitcoin_pgid" "$bitcoin_sid" ||
      fail "unregistered Bitcoin service child could not be terminated exactly"
    trap 'exit 130' INT
    trap 'exit 143' TERM
    [[ "$node_start_pending_signal" == 0 ]] || exit "$node_start_pending_signal"
    fail "Bitcoin service child could not be registered exactly"
  fi

  setsid env RUN_ID="$lez_run_id" LEZ_V02_KEEP_RUNNING=1 LEZ_V02_SOURCE_DIR="$lez_source_dir" \
    LEZ_V02_SLOT_DURATION_SECONDS="$lez_slot_duration_seconds" \
    LEZ_V02_MAKER_ACCOUNT_ID="$maker_account" LEZ_V02_MAKER_VAULT_ACCOUNT_ID="$maker_vault" \
    LEZ_V02_TAKER_ACCOUNT_ID="$taker_account" LEZ_V02_TAKER_VAULT_ACCOUNT_ID="$taker_vault" \
    "$lez_service_driver" >"$lez_log" 2>&1 &
  lez_pid=$!
  register_owned_process node-lez startup "$lez_pid" "$bash_executable" true true \
    lez_start lez_ppid lez_executable lez_pgid lez_sid "$owning_shell_pid" ||
    lez_registered=false
  if [[ "$lez_registered" == false ]]; then
    stop_provisional_owned_process "$lez_pid" "$lez_start" "$lez_ppid" \
      "$lez_executable" "$lez_pgid" "$lez_sid" ||
      fail "unregistered LEZ service child could not be terminated exactly"
    trap 'exit 130' INT
    trap 'exit 143' TERM
    [[ "$node_start_pending_signal" == 0 ]] || exit "$node_start_pending_signal"
    fail "LEZ service child could not be registered exactly"
  fi
  trap 'exit 130' INT
  trap 'exit 143' TERM
  [[ "$node_start_pending_signal" == 0 ]] || exit "$node_start_pending_signal"
  if wait_for_node_children "$bitcoin_pid" bitcoin_status "$bitcoin_pgid" "$bitcoin_sid" \
      "$lez_pid" lez_status "$lez_pgid" "$lez_sid" process_groups_absent; then :; fi

  if [[ "$bitcoin_status" == 0 ]]; then bitcoin_outcome=passed; else bitcoin_outcome=failed; fi
  if [[ "$lez_status" == 0 ]]; then lez_outcome=passed; else lez_outcome=failed; fi
  reconcile_node_resource_inventories "$bitcoin_outcome" "$lez_outcome" ||
    inventory_reconciled=false
  jq -n --argjson bitcoin_status "$bitcoin_status" --argjson lez_status "$lez_status" \
    --argjson owning_shell_pid "$owning_shell_pid" \
    --argjson bitcoin_registered "$bitcoin_registered" \
    --argjson lez_registered "$lez_registered" \
    --argjson inventory_reconciled "$inventory_reconciled" \
    --argjson process_groups_absent "$process_groups_absent" '
    {schema_version:1,owning_shell_pid:$owning_shell_pid,
     bitcoin_status:$bitcoin_status,lez_status:$lez_status,
     bitcoin_registered:$bitcoin_registered,lez_registered:$lez_registered,
     both_children_waited_and_reaped:true,
     exact_process_groups_absent_after_wait:$process_groups_absent,
     inventory_reconciled:$inventory_reconciled}
  ' >"${evidence_dir}/node-startup-status.json.partial"
  chmod 0600 "${evidence_dir}/node-startup-status.json.partial"
  mv "${evidence_dir}/node-startup-status.json.partial" \
    "${evidence_dir}/node-startup-status.json"
  [[ "$bitcoin_registered" == true ]] ||
    fail "Bitcoin service child could not be registered exactly"
  [[ "$lez_registered" == true ]] || fail "LEZ service child could not be registered exactly"
  [[ "$process_groups_absent" == true ]] ||
    fail "node service process groups were not absent after launcher wait"
  [[ "$bitcoin_status" == 0 ]] ||
    fail "Bitcoin service provisioning failed with status ${bitcoin_status}"
  [[ "$lez_status" == 0 ]] || fail "LEZ service provisioning failed with status ${lez_status}"
  [[ "$inventory_reconciled" == true ]] || fail "node resource inventory reconciliation failed"

  [[ -f "$bitcoin_manifest" && ! -L "$bitcoin_manifest" &&
     -f "$lez_manifest" && ! -L "$lez_manifest" ]] ||
    fail "retained node manifests are unavailable"
  [[ "$(stat -c '%a' "$bitcoin_manifest")" == 600 &&
     "$(stat -c '%a' "$lez_manifest")" == 600 ]] ||
    fail "retained node manifests are not owner-private"
  [[ "$(manifest_value "$bitcoin_manifest" RUN_ID)" == "$bitcoin_run_id" &&
     "$(manifest_value "$bitcoin_manifest" BITCOIN_CORE_E2E_MODE)" == service ]] ||
    fail "Bitcoin child manifest does not attest the fixed service run"
  [[ "$(manifest_value "$lez_manifest" RUN_ID)" == "$lez_run_id" ]] ||
    fail "LEZ child manifest does not attest the fixed service run"
  [[ "$(manifest_value "$lez_manifest" LEZ_V02_SLOT_DURATION_SECONDS)" == "$lez_slot_duration_seconds" ]] ||
    fail "LEZ child manifest does not attest the journey-selected slot duration"
}

core_admin() {
  local container_id
  container_id="$(single_owned_container_id "$bitcoin_container_ids" bitcoin-core)" ||
    fail "Bitcoin container inventory is malformed"
  assert_exact_owned_resource container "$container_id" "$bitcoin_run_id" \
    bitcoin-core-regtest-e2e bitcoin-core ||
    fail "captured Bitcoin container ownership identity drifted"
  docker exec "$container_id" bitcoin-cli \
    -conf=/run-config/bitcoin.conf -datadir=/var/lib/bitcoin "$@"
}

provision_bitcoin_funding_sources() {
  local address before_height after_height mined block_hash block txid vout utxo
  local direction height source_file mempool allocation
  local source_count="${#directions[@]}"
  local -a source_files=()
  if [[ "$source_count" == 1 ]]; then
    allocation="one_mature_coinbase_outpoint"
  else
    allocation="two_distinct_mature_coinbase_outpoints"
  fi
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
      --argjson confirmations "$(jq -er '.confirmations | numbers' <<<"$utxo")" '
      {direction:$direction,source:{transaction_id:$txid,output_index:$vout,
       value_sat:5000000000,script_pubkey:$script,coinbase:true,
       containing_block_hash:$block_hash,containing_block_height:$block_height,
       confirmations:$confirmations},planned_bitcoin_funding_anchor_height:null}
    ' >"$source_file"
    chmod 0600 "$source_file"
    source_files+=("$source_file")
  done

  jq -s --arg allocation "$allocation" --argjson base_height "$after_height" \
    '{schema_version:1,network:"regtest",allocation:$allocation,
      shared_fixture_custody_key:true,base_height:$base_height,sources:.}' \
    "${source_files[@]}" >"${bitcoin_funding_sources}.partial"
  chmod 0600 "${bitcoin_funding_sources}.partial"
  mv "${bitcoin_funding_sources}.partial" "$bitcoin_funding_sources"
  jq -e --arg allocation "$allocation" --argjson source_count "$source_count" '
    .schema_version == 1 and .network == "regtest" and .base_height == 102
    and .allocation == $allocation
    and (.sources | length) == $source_count
    and ([.sources[].direction] | unique | length) == $source_count
    and ([.sources[].source.transaction_id] | unique | length) == $source_count
    and ([.sources[] | [.source.transaction_id,.source.output_index]] | unique | length) == $source_count
    and all(.sources[]; .planned_bitcoin_funding_anchor_height == null)
    and all(.sources[]; .source.confirmations >= 101)
  ' "$bitcoin_funding_sources" >/dev/null ||
    fail "independent Bitcoin funding-source manifest is inconsistent"
}

reserve_bitcoin_funding_anchors() {
  local reservation_mode="$1" direction="${2:-}"
  local tip_before tip_after mempool partial
  [[ "$reservation_mode" == "$schedule" ]] ||
    fail "Bitcoin anchor-reservation mode differs from the execution schedule"
  case "$reservation_mode" in
    sequential)
      [[ "$direction" == "taker_sells_foreign" || "$direction" == "taker_sells_lez" ]] ||
        fail "sequential Bitcoin anchor reservation requires one exact direction"
      [[ ! -e "${directions_dir}/${direction}/stage-two.json" &&
         ! -L "${directions_dir}/${direction}/stage-two.json" &&
         ! -e "${evidence_dir}/${direction}-stage-two.json" &&
         ! -L "${evidence_dir}/${direction}-stage-two.json" ]] ||
        fail "refusing to reserve or rebase an anchor after stage-two finalization"
      ;;
    overlap)
      [[ -z "$direction" ]] ||
        fail "overlap Bitcoin anchors must be reserved atomically"
      for direction in "${directions[@]}"; do
        [[ ! -e "${directions_dir}/${direction}/stage-two.json" &&
           ! -L "${directions_dir}/${direction}/stage-two.json" &&
           ! -e "${evidence_dir}/${direction}-stage-two.json" &&
           ! -L "${evidence_dir}/${direction}-stage-two.json" ]] ||
          fail "refusing to reserve overlap anchors after stage-two finalization"
      done
      direction=""
      ;;
    *) fail "invalid Bitcoin anchor-reservation mode" ;;
  esac
  [[ -f "$bitcoin_funding_sources" && ! -L "$bitcoin_funding_sources" &&
     "$(stat -c '%a' "$bitcoin_funding_sources")" == 600 ]] ||
    fail "Bitcoin funding-source manifest is unavailable or unsafe"
  [[ -f "$bitcoin_anchor_assignment_filter" &&
     ! -L "$bitcoin_anchor_assignment_filter" ]] ||
    fail "Bitcoin anchor-assignment filter is unavailable or unsafe"
  tip_before="$(core_admin getblockcount)"
  [[ "$tip_before" =~ ^[0-9]+$ ]] ||
    fail "Core tip before anchor reservation is malformed"
  mempool="$(core_admin getrawmempool)"
  jq -e 'type == "array" and length == 0' <<<"$mempool" >/dev/null ||
    fail "Bitcoin anchor reservation requires an empty run-owned mempool"
  tip_after="$(core_admin getblockcount)"
  [[ "$tip_after" == "$tip_before" ]] ||
    fail "Core tip moved while reserving Bitcoin funding anchors"
  partial="${bitcoin_funding_sources}.anchor-partial"
  [[ ! -e "$partial" && ! -L "$partial" ]] ||
    fail "refusing an existing Bitcoin anchor-reservation partial"
  jq -e --arg mode "$reservation_mode" --arg direction "$direction" \
    --argjson base_height "$tip_before" \
    -f "$bitcoin_anchor_assignment_filter" "$bitcoin_funding_sources" >"$partial" || {
      rm -f -- "$partial"
      fail "Bitcoin anchor reservation was rejected"
    }
  chmod 0600 "$partial"
  mv "$partial" "$bitcoin_funding_sources"
  [[ "$(core_admin getblockcount)" == "$tip_before" ]] ||
    fail "Core tip moved after Bitcoin anchor reservation"
}

bootstrap_lez_runtime() {
  M3_POC_RUN_ID="$run_id" \
  M5_BTC_APPLICATION_MODE="$m5_btc_application_mode" \
  M5_LEZ_DEPLOYER_SHA256="$expected_lez_deployer_sha256" \
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

prepare_m5_btc_delivery_plan() {
  local direction="$1"
  local direction_root="${directions_dir}/${direction}"
  local fixture_root="${direction_root}/fixture"
  local application_root="${direction_root}/application"
  local socket="${secure_state_root}/m5-btc-maker.sock"
  local ready_file="${secure_state_root}/m5-btc-maker.ready"
  local database="${application_root}/maker.sqlite3"
  local delivery="${application_root}/delivery"
  local delivery_key="${fixture_root}/private/maker-signing.key"
  local plan_file="${application_root}/btc-plan.json"
  local daemon_log="${application_root}/delivery-daemon.log"
  local offer_id="m5btc-offer-${run_id:0:24}"
  local reservation_id="m5btc-reservation-${run_id:0:24}"
  local maker_public_key now swap_id
  local daemon_pid daemon_start="" daemon_ppid="" daemon_executable=""
  local daemon_pgid="" daemon_sid=""

  [[ "$m5_btc_application_mode" == 1 && "$direction" == taker_sells_foreign ]] ||
    fail "M5 BTC planning is restricted to taker_sells_foreign"
  [[ ! -e "$application_root" && ! -L "$application_root" ]] ||
    fail "M5 BTC application root already exists"
  mkdir -m 0700 "$application_root"
  maker_public_key="$(jq -er '.maker.musig2_public_key' \
    "${fixture_root}/public-spec.json")"
  [[ "$maker_public_key" =~ ^0[23][0-9a-f]{64}$ ]] ||
    fail "stage-one Maker public key is invalid"

  setsid "$maker_daemon_bin" \
    --socket "$socket" --database "$database" --ready-file "$ready_file" \
    --delivery-directory "$delivery" --delivery-signing-key-file "$delivery_key" \
    >"$daemon_log" 2>&1 &
  daemon_pid=$!
  if ! register_owned_process m5-btc-delivery planning "$daemon_pid" \
      "$maker_daemon_bin" true true daemon_start daemon_ppid daemon_executable \
      daemon_pgid daemon_sid; then
    stop_provisional_owned_process "$daemon_pid" "$daemon_start" "$daemon_ppid" \
      "$daemon_executable" "$daemon_pgid" "$daemon_sid" || true
    fail "M5 BTC Delivery-only daemon registration failed"
  fi

  for _ in {1..200}; do
    if [[ -f "$ready_file" ]] &&
       [[ "$(cat "$ready_file")" == "$socket" ]] && [[ -S "$socket" ]]; then
      break
    fi
    kill -0 "$daemon_pid" 2>/dev/null ||
      fail "M5 BTC Delivery-only daemon exited before readiness"
    sleep 0.05
  done
  [[ -S "$socket" && -f "$ready_file" && "$(cat "$ready_file")" == "$socket" ]] ||
    fail "M5 BTC Delivery-only daemon readiness timed out"

  "$maker_cli_bin" --socket "$socket" configure-pair --request-id \
    "${run_id}-btc-pair-create" --pair bitcoin --direction taker-sells-foreign \
    --enabled false --minimum-foreign-units 1 --maximum-foreign-units 100000000 \
    --offer-ttl-seconds 7200 >"${application_root}/pair-create.json"
  "$maker_cli_bin" --socket "$socket" set-local-price --request-id \
    "${run_id}-btc-price" --pair bitcoin --direction taker-sells-foreign \
    --lez-units-per-lot 1 --foreign-units-per-lot 1000 \
    >"${application_root}/price.json"
  "$maker_cli_bin" --socket "$socket" configure-pair --request-id \
    "${run_id}-btc-pair-enable" --expected-revision 1 --pair bitcoin \
    --direction taker-sells-foreign --enabled true --minimum-foreign-units 1 \
    --maximum-foreign-units 100000000 --offer-ttl-seconds 7200 \
    >"${application_root}/pair-enable.json"
  "$maker_cli_bin" --socket "$socket" publish-offer --request-id \
    "${run_id}-btc-offer" --offer-id "$offer_id" --pair bitcoin \
    --direction taker-sells-foreign >"${application_root}/offer.json"

  now="$(date -u +%s)"
  "$taker_cli_bin" --delivery-directory "$delivery" \
    --maker-public-key "$maker_public_key" --now-unix-seconds "$now" \
    --pair bitcoin --direction taker-sells-foreign \
    --plan-btc-offer "$offer_id" --reservation-id "$reservation_id" \
    --foreign-units 1000000 >"$plan_file"
  chmod 0600 "$plan_file" "$daemon_log" "${application_root}"/*.json
  jq -e --arg offer "$offer_id" --arg reservation "$reservation_id" '
    .schema_version == 1 and .offer_id == $offer and
    .reservation_id == $reservation and
    (.signed_envelope_sha256 | test("^[0-9a-f]{64}$")) and
    (.swap_id | test("^[0-9a-f]{64}$")) and
    .foreign_units == 1000000 and .lez_units == 1000 and
    .private_material_disclosed == false
  ' "$plan_file" >/dev/null || fail "M5 BTC Taker planning output is invalid"
  swap_id="$(jq -er '.swap_id' "$plan_file")"
  m5_btc_swap_ids["$direction"]="$swap_id"

  stop_provisional_owned_process "$daemon_pid" "$daemon_start" "$daemon_ppid" \
    "$daemon_executable" "$daemon_pgid" "$daemon_sid" ||
    fail "M5 BTC Delivery-only daemon shutdown failed"
  [[ ! -e "$socket" && ! -e "$ready_file" ]] ||
    fail "M5 BTC Delivery-only daemon left live endpoints"
}

with_direction_environment() {
  local direction="$1"
  shift
  local direction_root="${directions_dir}/${direction}"
  local bitcoin_container_id m5_swap_id=""
  if [[ "$m5_btc_application_mode" == 1 ]]; then
    m5_swap_id="${m5_btc_swap_ids[$direction]:-}"
    [[ "$m5_swap_id" =~ ^[0-9a-f]{64}$ ]] || fail "M5 BTC planned swap ID is unavailable"
  fi
  bitcoin_container_id="$(single_owned_container_id \
    "$bitcoin_container_ids" bitcoin-core)" ||
    fail "Bitcoin container inventory is malformed at actor handoff"
  assert_exact_owned_resource container "$bitcoin_container_id" "$bitcoin_run_id" \
    bitcoin-core-regtest-e2e bitcoin-core ||
    fail "captured Bitcoin container ownership identity drifted at actor handoff"
  M3_POC_RUN_ID="$run_id" \
  M5_BTC_APPLICATION_MODE="$m5_btc_application_mode" \
  M3_POC_SWAP_ID="$m5_swap_id" \
  M3_POC_JOURNEY="$journey" \
  M3_POC_DIRECTION="$direction" \
  M3_POC_DIRECTION_ROOT="$direction_root" \
  M3_POC_SECURE_STATE_ROOT="${secure_state_root}/directions/${direction}" \
  M3_POC_EVIDENCE_DIR="$evidence_dir" \
  M3_POC_PROCESS_REGISTRY="$process_registry" \
  M3_POC_ACTOR_BIN="$actor_bin" \
  M3_POC_PROVISIONER_BIN="$provisioner_bin" \
  M3_POC_M5_APPLICATION_ROOT="${direction_root}/application" \
  M3_POC_MAKER_DAEMON_BIN="$maker_daemon_bin" \
  M3_POC_TAKER_CLI_BIN="$taker_cli_bin" \
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
  M3_POC_BITCOIN_CONTAINER_ID="$bitcoin_container_id" \
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
  local direction="$1" pid="$2" executable=""
  for _ in {1..100}; do
    if [[ -r "/proc/${pid}/stat" ]]; then
      executable="$(readlink -f "/proc/${pid}/exe" 2>/dev/null || true)"
      if [[ "$executable" == /* ]]; then break; fi
    fi
    sleep 0.01
  done
  [[ "$executable" == /* ]] ||
    fail "overlap ${direction} driver exited before registration"
  register_owned_process "controller-${direction}" overlap-driver "$pid" \
    "$executable" false true ||
    fail "overlap ${direction} driver could not be registered exactly"
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

OFFICIAL_WALLET_ATA_BALANCE=""
read_official_wallet_ata_balance() {
  local owner="$1" definition="$2" ata="$3" output="$4"
  local wallet_home="${f7_token_fixture_private}/wallets/maker"
  local wallet_output
  local -a balances=()
  [[ -d "$wallet_home" && ! -L "$wallet_home" && ! -e "$output" && ! -L "$output" ]] ||
    fail "official wallet terminal-balance inputs are unavailable or unsafe"
  wallet_output="$(
    printf '%s\n' 'local-poc-password-unused-upstream' |
      timeout --preserve-status 180s env LEE_WALLET_HOME_DIR="$wallet_home" \
        "$official_wallet_bin" ata list --owner "$owner" --token-definition "$definition"
  )" || fail "official wallet terminal ATA read failed"
  printf '%s\n' "$wallet_output" >"$output"
  chmod 0600 "$output"
  mapfile -t balances < <(
    sed -n "s/^ATA ${ata} (definition ${definition}): balance \([0-9][0-9]*\)$/\1/p" \
      "$output"
  )
  [[ "${#balances[@]}" == 1 ]] ||
    fail "official wallet did not return exactly one expected terminal ATA balance"
  OFFICIAL_WALLET_ATA_BALANCE="${balances[0]}"
}

write_custom_token_terminal_balance_evidence() {
  local direction="$1"
  local asset lez_owner evidence_kind transition claim_label actor_revision
  local expected_maker expected_taker definition maker_owner taker_owner maker_ata taker_ata
  local direction_root actor_db final_terms finality actor_submit actual_effects
  local actor_output output maker_log taker_log before_replay after_replay
  local maker_balance taker_balance custody_balance actor_payload claim_transaction
  local maker_codec taker_codec definition_codec custody_codec
  local maker_ata_hex taker_ata_hex definition_hex custody_hex custody_base58
  local actor_sha submit_sha finality_sha effects_sha terms_sha maker_log_sha taker_log_sha
  local before_replay_sha after_replay_sha
  local -a actor_rows=()
  [[ "$asset_mode" == "custom_token" && "$journey" == "claim" ]] ||
    fail "terminal custom-token balances require the custom-token claim journey"
  case "$direction" in
    taker_sells_foreign)
      asset=M3F7A
      lez_owner=taker
      evidence_kind=revealing_claim
      transition=revealing_claim
      claim_label=lez-revealing-claim
      actor_revision=3
      expected_maker=175
      expected_taker=75
      ;;
    taker_sells_lez)
      asset=M3F7B
      lez_owner=maker
      evidence_kind=followup_claim
      transition=followup_claim
      claim_label=lez-followup-claim
      actor_revision=4
      expected_maker=75
      expected_taker=175
      ;;
    *) fail "unsupported custom-token terminal-balance direction" ;;
  esac
  definition="$(jq -er --arg asset "$asset" '.assets[$asset].definition' \
    "$f7_token_fixture_evidence")"
  maker_owner="$(jq -er '.actors.maker' "$f7_token_fixture_evidence")"
  taker_owner="$(jq -er '.actors.taker' "$f7_token_fixture_evidence")"
  maker_ata="$(jq -er --arg asset "$asset" '.assets[$asset].atas.maker' \
    "$f7_token_fixture_evidence")"
  taker_ata="$(jq -er --arg asset "$asset" '.assets[$asset].atas.taker' \
    "$f7_token_fixture_evidence")"
  maker_log="${evidence_dir}/${direction}-custom-token-terminal-maker-ata.log"
  taker_log="${evidence_dir}/${direction}-custom-token-terminal-taker-ata.log"
  read_official_wallet_ata_balance "$maker_owner" "$definition" "$maker_ata" "$maker_log"
  maker_balance="$OFFICIAL_WALLET_ATA_BALANCE"
  read_official_wallet_ata_balance "$taker_owner" "$definition" "$taker_ata" "$taker_log"
  taker_balance="$OFFICIAL_WALLET_ATA_BALANCE"

  direction_root="${directions_dir}/${direction}"
  actor_db="${direction_root}/actors/${lez_owner}/actor-state.sqlite"
  final_terms="${direction_root}/final-asset-terms.json"
  finality="${evidence_dir}/${direction}-${claim_label}-finality.json"
  actor_submit="${evidence_dir}/${direction}-${claim_label}-submit-${lez_owner}.json"
  actual_effects="${evidence_dir}/${direction}-actual-effects.json"
  before_replay="${evidence_dir}/${direction}-submission-counts-before-replay.json"
  after_replay="${evidence_dir}/${direction}-submission-counts-after-replay.json"
  actor_output="${evidence_dir}/${direction}-custom-token-finalized-actor-claim.json"
  output="${evidence_dir}/${direction}-custom-token-terminal-balances.json"
  for input in "$actor_db" "$final_terms" "$finality" "$actor_submit" "$actual_effects" \
    "$before_replay" "$after_replay"; do
    [[ -f "$input" && ! -L "$input" ]] ||
      fail "terminal custom-token evidence input is unavailable or unsafe: ${input##*/}"
  done
  [[ ! -e "$actor_output" && ! -L "$actor_output" &&
     ! -e "$output" && ! -L "$output" ]] ||
    fail "refusing to overwrite terminal custom-token evidence"
  [[ "$(jq -S -c . "$before_replay")" == "$(jq -S -c . "$after_replay")" ]] ||
    fail "terminal balance sampling requires quiescent replay submission counts"
  mapfile -t actor_rows < <(
    sqlite3 -batch -noheader -readonly "$actor_db" \
      "SELECT payload_json FROM btc_actor_evidence WHERE local_role = '${lez_owner}' AND aggregate_revision = ${actor_revision} AND evidence_kind = '${evidence_kind}' ORDER BY aggregate_revision;"
  )
  [[ "${#actor_rows[@]}" == 1 ]] ||
    fail "actor state does not contain exactly one finalized LEZ claim evidence row"
  actor_payload="${actor_rows[0]}"
  claim_transaction="$(jq -er '.transaction_id' "$finality")"
  jq -e --arg kind "$evidence_kind" --arg tx "$claim_transaction" '
    .kind == $kind and .chain == "Lez" and .proof.transaction_id == $tx
    and (.chain_evidence | type) == "array" and (.chain_evidence | length) > 0
  ' <<<"$actor_payload" >/dev/null ||
    fail "actor lifecycle row does not bind the expected finalized LEZ claim"
  jq -e --arg role "$lez_owner" --argjson revision "$((actor_revision - 1))" '
    .schema_version == 1 and .role == $role and .command == "drive"
    and .outcome == "awaiting_observation" and .chain == "lez"
    and .revision == $revision
  ' "$actor_submit" >/dev/null ||
    fail "actor submit output does not prove the role-owned LEZ claim"
  jq -e --arg tx "$claim_transaction" '
    .actor_owned_claims.lez == $tx
    and .lez_effect_ids[-1] == $tx
  ' "$actual_effects" >/dev/null ||
    fail "actual-effect manifest does not bind the actor-owned LEZ claim"
  jq -c '.chain_evidence | implode | fromjson' <<<"$actor_payload" >"$actor_output"
  chmod 0600 "$actor_output"

  maker_codec="$("$account_codec_bin" "$maker_ata")" ||
    fail "official account codec rejected the Maker ATA"
  taker_codec="$("$account_codec_bin" "$taker_ata")" ||
    fail "official account codec rejected the Taker ATA"
  definition_codec="$("$account_codec_bin" "$definition")" ||
    fail "official account codec rejected the token definition"
  maker_ata_hex="$(jq -er '.account_id_hex' <<<"$maker_codec")"
  taker_ata_hex="$(jq -er '.account_id_hex' <<<"$taker_codec")"
  definition_hex="$(jq -er '.account_id_hex' <<<"$definition_codec")"
  custody_hex="$(jq -er '.asset.terms.custody_ata_account_id' "$final_terms")"
  custody_codec="$("$account_codec_bin" --from-hex "$custody_hex")" ||
    fail "official account codec rejected the custody ATA"
  custody_base58="$(jq -er '.account_id_base58' <<<"$custody_codec")"
  jq -e --arg direction "$direction" --arg maker "$maker_ata_hex" \
    --arg taker "$taker_ata_hex" --arg definition "$definition_hex" '
    .asset.terms.token_definition_account_id == $definition
    and if $direction == "taker_sells_foreign" then
      .asset.terms.depositor == "maker" and .asset.terms.claimant == "taker"
      and .asset.terms.depositor_ata_account_id == $maker
      and .asset.terms.claimant_ata_account_id == $taker
    else
      .asset.terms.depositor == "taker" and .asset.terms.claimant == "maker"
      and .asset.terms.depositor_ata_account_id == $taker
      and .asset.terms.claimant_ata_account_id == $maker
    end
  ' "$final_terms" >/dev/null ||
    fail "official account-codec ATA mappings differ from the signed asset terms"
  jq -e --arg transition "$transition" --arg tx "$claim_transaction" \
    --arg role "$lez_owner" \
    --arg asset_commitment "$(jq -er '.asset_commitment' \
      "${evidence_dir}/${direction}-asset-extension.json")" \
    --arg custody "$custody_hex" --arg definition "$definition_hex" \
    --arg claimant_ata "$(jq -er '.asset.terms.claimant_ata_account_id' "$final_terms")" \
    --arg amount "$(jq -er '.asset.terms.amount | strings' "$final_terms")" \
    --arg token_program "$(jq -er '.asset.terms.token_program_id' "$final_terms")" \
    --argjson block "$(jq -er '.containing_block_id | numbers' "$finality")" \
    --arg block_hash "$(jq -er '.containing_block_hash' "$finality")" '
    .schema_version == 2 and .transition == $transition
    and .runtime.sidecar_role == $role
    and .asset_commitment == $asset_commitment
    and .facts.transaction.transaction_id == $tx
    and .facts.containing_block.block_id == $block
    and .facts.containing_block.block_hash == $block_hash
    and .finalized_clock.height >= $block
    and .scanned_window.start_height <= $block
    and (.scanned_window.start_height + .scanned_window.max_blocks - 1) >= $block
    and .facts.metadata.status == "claimed"
    and .facts.metadata.claimant_asset_account_id == $claimant_ata
    and .facts.metadata.asset_definition == $definition
    and .facts.metadata.custody_account_id == $custody
    and (.facts.metadata.amount | tostring) == $amount and $amount == "75"
    and .facts.custody.kind == "custom_token"
    and .facts.custody.facts.account_id == $custody
    and .facts.custody.facts.token_definition_account_id == $definition
    and .facts.custody.facts.owner_program_id == $token_program
    and (.facts.custody.facts.balance | tostring) == "0"
  ' "$actor_output" >/dev/null ||
    fail "typed finalized actor evidence does not prove the exact empty custody ATA"
  custody_balance="$(jq -er '.facts.custody.facts.balance | tonumber' "$actor_output")"
  [[ "$maker_balance" == "$expected_maker" && "$taker_balance" == "$expected_taker" &&
     "$custody_balance" == 0 ]] ||
    fail "custom-token terminal balances differ from the exact direction outcome"

  actor_sha="$(sha256sum "$actor_output" | sed 's/ .*//')"
  submit_sha="$(sha256sum "$actor_submit" | sed 's/ .*//')"
  finality_sha="$(sha256sum "$finality" | sed 's/ .*//')"
  effects_sha="$(sha256sum "$actual_effects" | sed 's/ .*//')"
  terms_sha="$(sha256sum "$final_terms" | sed 's/ .*//')"
  maker_log_sha="$(sha256sum "$maker_log" | sed 's/ .*//')"
  taker_log_sha="$(sha256sum "$taker_log" | sed 's/ .*//')"
  before_replay_sha="$(sha256sum "$before_replay" | sed 's/ .*//')"
  after_replay_sha="$(sha256sum "$after_replay" | sed 's/ .*//')"
  jq -n --arg direction "$direction" --arg asset "$asset" --arg definition "$definition" \
    --arg maker_ata "$maker_ata" --arg taker_ata "$taker_ata" \
    --arg custody_ata "$custody_base58" --arg claim_transaction "$claim_transaction" \
    --arg actor_file "$(basename "$actor_output")" --arg actor_sha "$actor_sha" \
    --arg submit_file "$(basename "$actor_submit")" --arg submit_sha "$submit_sha" \
    --arg finality_file "$(basename "$finality")" --arg finality_sha "$finality_sha" \
    --arg effects_file "$(basename "$actual_effects")" --arg effects_sha "$effects_sha" \
    --arg terms_file "$(basename "$final_terms")" --arg terms_sha "$terms_sha" \
    --arg maker_log "$(basename "$maker_log")" --arg maker_log_sha "$maker_log_sha" \
    --arg taker_log "$(basename "$taker_log")" --arg taker_log_sha "$taker_log_sha" \
    --arg before_replay "$(basename "$before_replay")" --arg before_replay_sha "$before_replay_sha" \
    --arg after_replay "$(basename "$after_replay")" --arg after_replay_sha "$after_replay_sha" \
    --argjson maker_balance "$maker_balance" --argjson taker_balance "$taker_balance" \
    --argjson custody_balance "$custody_balance" '
    {schema_version:1,kind:"m3_f7_terminal_custom_token_balances",
     direction:$direction,asset:$asset,token_definition:$definition,
     claim_transaction_id:$claim_transaction,
     balances:{maker:$maker_balance,taker:$taker_balance,custody:$custody_balance},
     conservation_total:($maker_balance + $taker_balance + $custody_balance),
     expected_total:250,exact_direction_balances:true,
     accounts:{maker_ata:$maker_ata,taker_ata:$taker_ata,custody_ata:$custody_ata},
     owner_balance_source:{
       reader:"official_lez_v0_2_wallet",
       scope:"post_finality_quiescent_sequencer_account_read",
       finalized_claim_proved_before_read:true,
       same_atomic_snapshot_as_finalized_claim:false,
       maker_and_taker_reads_share_one_atomic_snapshot:false,
       wallet_operations_read_only:true,
       isolated_run_quiescent_after_terminal_replay:true,
       no_later_asset_mutation_command_executed:true,
       terminal_replay_resubmission_count:0,
       maker_log:{file:$maker_log,sha256:$maker_log_sha},
       taker_log:{file:$taker_log,sha256:$taker_log_sha}},
     custody_balance_source:{
       reader:"finalized_actor_chain_evidence",
       finalized:true,metadata_status:"claimed",
       claim_transfer_atomicity:"single_on_chain_claim_transaction",
       file:$actor_file,sha256:$actor_sha},
     bindings:{
       actor_submit:{file:$submit_file,sha256:$submit_sha},
       lez_claim_finality:{file:$finality_file,sha256:$finality_sha},
       actual_effects:{file:$effects_file,sha256:$effects_sha},
       signed_asset_terms:{file:$terms_file,sha256:$terms_sha},
       replay_before:{file:$before_replay,sha256:$before_replay_sha},
       replay_after:{file:$after_replay,sha256:$after_replay_sha}}}
  ' >"$output"
  chmod 0600 "$output"
  jq -e --arg direction "$direction" \
    --argjson maker "$expected_maker" --argjson taker "$expected_taker" '
    .schema_version == 1 and .direction == $direction
    and .balances == {maker:$maker,taker:$taker,custody:0}
    and .conservation_total == 250 and .expected_total == 250
    and .exact_direction_balances == true
    and .owner_balance_source.reader == "official_lez_v0_2_wallet"
    and .owner_balance_source.scope == "post_finality_quiescent_sequencer_account_read"
    and .owner_balance_source.same_atomic_snapshot_as_finalized_claim == false
    and .owner_balance_source.maker_and_taker_reads_share_one_atomic_snapshot == false
    and .owner_balance_source.wallet_operations_read_only == true
    and .owner_balance_source.isolated_run_quiescent_after_terminal_replay == true
    and .owner_balance_source.no_later_asset_mutation_command_executed == true
    and .custody_balance_source.reader == "finalized_actor_chain_evidence"
    and .custody_balance_source.finalized == true
    and .custody_balance_source.metadata_status == "claimed"
    and .custody_balance_source.claim_transfer_atomicity ==
      "single_on_chain_claim_transaction"
    and all([
      .owner_balance_source.maker_log.sha256,
      .owner_balance_source.taker_log.sha256,
      .custody_balance_source.sha256,
      .bindings.actor_submit.sha256,
      .bindings.lez_claim_finality.sha256,
      .bindings.actual_effects.sha256,
      .bindings.signed_asset_terms.sha256,
      .bindings.replay_before.sha256,
      .bindings.replay_after.sha256
    ][]; test("^[0-9a-f]{64}$"))
  ' "$output" >/dev/null ||
    fail "terminal custom-token balance evidence is incomplete"
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
  if (( ${#directions[@]} > 1 )); then
  jq -e --slurpfile foreign "${evidence_dir}/taker_sells_foreign-actual-effects.json" \
    --slurpfile lez "${evidence_dir}/taker_sells_lez-actual-effects.json" '
    (($foreign[0].bitcoin_effect_ids + $lez[0].bitcoin_effect_ids) as $ids |
      ($ids | unique | length) == ($ids | length))
    and (($foreign[0].lez_effect_ids + $lez[0].lez_effect_ids) as $ids |
      ($ids | unique | length) == ($ids | length))
  ' <<<null >/dev/null || fail "direction effect IDs overlap across independent swaps"
  fi
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

service_launcher_hashes_stable() {
  local bitcoin_sha lez_sha
  [[ -f "$bitcoin_service_driver" && ! -L "$bitcoin_service_driver" &&
     -f "$lez_service_driver" && ! -L "$lez_service_driver" ]] || return 1
  bitcoin_sha="$(sha256sum "$bitcoin_service_driver" | sed 's/ .*//')" || return 1
  lez_sha="$(sha256sum "$lez_service_driver" | sed 's/ .*//')" || return 1
  [[ "$bitcoin_service_driver_sha_at_start" =~ ^[0-9a-f]{64}$ &&
     "$lez_service_driver_sha_at_start" =~ ^[0-9a-f]{64}$ &&
     "$bitcoin_sha" == "$bitcoin_service_driver_sha_at_start" &&
     "$lez_sha" == "$lez_service_driver_sha_at_start" ]]
}

write_run_evidence() {
  local repository_commit origin_main completed_at outer_runner_sha direction_driver_sha lez_bootstrap_sha
  local bitcoin_service_driver_sha lez_service_driver_sha
  local repository_status
  local bedrock_log bedrock_ntp_timeout_count
  local foreign_survivor_summary="null" lez_survivor_summary="null"
  local overlap_summary="null"
  local f7_token_fixture_summary="null" f7_token_fixture_sha=""
  local f7_token_fixture_driver_sha=""
  local official_wallet_cache_summary="null" official_wallet_cache_evidence_sha=""
  local official_wallet_cache_helper_sha=""
  local foreign_terminal_balance_summary="null" lez_terminal_balance_summary="null"
  local foreign_stage2_sha lez_stage2_sha=""
  local terminal_file terminal_sha effects_sha
  local phase_timing_summary="" phase_timing_sha=""
  local foreign_actor_direction_timing_summary=""
  local lez_actor_direction_timing_summary=""
  local actor_direction_timing_summary=""
  validate_phase_timings_for_run_evidence phase_timing_summary ||
    fail "finalized phase timing evidence is invalid"
  validate_actor_direction_phase_timing_for_run_evidence \
    taker_sells_foreign foreign_actor_direction_timing_summary "$phase_timing_sha" ||
    fail "forward actor direction timing evidence is invalid"
  if [[ "$m5_btc_application_mode" == 1 ]]; then
    actor_direction_timing_summary="$(jq -cn \
      --argjson foreign "$foreign_actor_direction_timing_summary" \
      '{taker_sells_foreign:$foreign}')" ||
      fail "M5 actor direction timing summary construction failed"
  else
    validate_actor_direction_phase_timing_for_run_evidence \
      taker_sells_lez lez_actor_direction_timing_summary "$phase_timing_sha" ||
      fail "reverse actor direction timing evidence is invalid"
    actor_direction_timing_summary="$(jq -cn \
      --argjson foreign "$foreign_actor_direction_timing_summary" \
      --argjson lez "$lez_actor_direction_timing_summary" \
      '{taker_sells_foreign:$foreign,taker_sells_lez:$lez}')" ||
      fail "actor direction timing summary construction failed"
  fi
  actor_direction_phase_timings_hash_stable "$actor_direction_timing_summary" ||
    fail "actor direction timing evidence changed during validation"
  service_launcher_hashes_stable ||
    fail "service launcher changed before terminal evidence publication"
  repository_status="$(git status --porcelain --untracked-files=all)" ||
    fail "final repository status query failed"
  [[ -z "$repository_status" ]] || fail "repository changed during M3 actor execution"
  repository_commit="$(git rev-parse HEAD)"
  [[ "$repository_commit" == "$repository_commit_at_start" ]] ||
    fail "repository HEAD changed during M3 actor execution"
  origin_main="$(git rev-parse refs/remotes/origin/main)" ||
    fail "final origin/main remote-tracking commit query failed"
  [[ "$origin_main" == "$origin_main_at_start" &&
     "$origin_main" == "$repository_commit" ]] ||
    fail "origin/main or repository HEAD changed during M3 actor execution"
  completed_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  outer_runner_sha="$(sha256sum scripts/run-m3-actor-local-poc.sh | sed 's/ .*//')"
  direction_driver_sha="$(sha256sum "$direction_driver" | sed 's/ .*//')"
  lez_bootstrap_sha="$(sha256sum "$lez_bootstrap_driver" | sed 's/ .*//')"
  bitcoin_service_driver_sha="$(sha256sum "$bitcoin_service_driver" | sed 's/ .*//')"
  lez_service_driver_sha="$(sha256sum "$lez_service_driver" | sed 's/ .*//')"
  [[ "$outer_runner_sha" == "$outer_runner_sha_at_start" &&
     "$direction_driver_sha" == "$direction_driver_sha_at_start" &&
     "$lez_bootstrap_sha" == "$lez_bootstrap_sha_at_start" &&
     "$bitcoin_service_driver_sha" == "$bitcoin_service_driver_sha_at_start" &&
     "$lez_service_driver_sha" == "$lez_service_driver_sha_at_start" ]] ||
    fail "certified executable changed during M3 actor execution"
  if [[ "$asset_mode" == "custom_token" ]]; then
    [[ -f "$official_wallet_cache_evidence" &&
       ! -L "$official_wallet_cache_evidence" ]] ||
      fail "official-wallet cache evidence is unavailable"
    official_wallet_cache_evidence_sha="$(sha256sum "$official_wallet_cache_evidence" |
      sed 's/ .*//')"
    official_wallet_cache_helper_sha="$(sha256sum "$official_wallet_cache_helper" |
      sed 's/ .*//')"
    [[ "$official_wallet_cache_helper_sha" == \
       "$official_wallet_cache_helper_sha_at_start" &&
       "$official_wallet_cache_helper_sha" == \
       "$(jq -er '.publisher_helper_sha256' "$official_wallet_cache_evidence")" ]] ||
      fail "official-wallet helper changed after artifact preparation"
    official_wallet_cache_summary="$(jq -c \
      --arg evidence_path "${relative_run_root}/evidence/official-wallet-artifact.json" \
      --arg evidence_sha "$official_wallet_cache_evidence_sha" '
      {
        kind:.kind,
        result:.result,
        cache_hit:.cache_hit,
        test_mode:.test_mode,
        input:.input,
        validation_policy_revision:.validation_policy_revision,
        publisher_helper_sha256:.publisher_helper_sha256,
        input_key:.input_key,
        wallet_sha256:.wallet_sha256,
        object_manifest_sha256:.object_manifest_sha256,
        runtime_fingerprint_sha256:.runtime_fingerprint_sha256,
        duration_ms:.duration_ms,
        artifact_bytes:.artifact_bytes,
        private_copy:.private_copy,
        hardlink:.hardlink,
        source_rehashed_after_copy:.source_rehashed_after_copy,
        destination_rehashed:.destination_rehashed,
        secrets_or_state_cached:.secrets_or_state_cached,
        evidence_path:$evidence_path,
        evidence_sha256:$evidence_sha
      }
    ' "$official_wallet_cache_evidence")"
    f7_token_fixture_sha="$(sha256sum "$f7_token_fixture_evidence" | sed 's/ .*//')"
    f7_token_fixture_driver_sha="$(sha256sum "$f7_token_fixture_driver" | sed 's/ .*//')"
    [[ "$f7_token_fixture_driver_sha" == "$f7_token_fixture_driver_sha_at_start" ]] ||
      fail "F7 token-fixture driver changed during M3 actor execution"
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
  if [[ "$asset_mode" == "custom_token" ]]; then
    terminal_file="${evidence_dir}/taker_sells_foreign-custom-token-terminal-balances.json"
    [[ -f "$terminal_file" && ! -L "$terminal_file" ]] ||
      fail "forward custom-token terminal balance evidence is unavailable"
    terminal_sha="$(sha256sum "$terminal_file" | sed 's/ .*//')"
    effects_sha="$(sha256sum "${evidence_dir}/taker_sells_foreign-actual-effects.json" | sed 's/ .*//')"
    [[ "$(jq -er '.bindings.actual_effects.sha256' "$terminal_file")" == "$effects_sha" ]] ||
      fail "forward terminal balances do not bind the retained actual effects"
    foreign_terminal_balance_summary="$(jq -c --arg path "${relative_run_root}/evidence/taker_sells_foreign-custom-token-terminal-balances.json" --arg sha "$terminal_sha" '
      {evidence_path:$path,evidence_sha256:$sha,direction:.direction,asset:.asset,
       balances:.balances,conservation_total:.conservation_total,
       exact_direction_balances:.exact_direction_balances,
       owner_balance_source:.owner_balance_source,
       custody_balance_source:.custody_balance_source,
       bindings:.bindings,
       actual_effects_sha256:.bindings.actual_effects.sha256}
    ' "$terminal_file")"
    terminal_file="${evidence_dir}/taker_sells_lez-custom-token-terminal-balances.json"
    [[ -f "$terminal_file" && ! -L "$terminal_file" ]] ||
      fail "reverse custom-token terminal balance evidence is unavailable"
    terminal_sha="$(sha256sum "$terminal_file" | sed 's/ .*//')"
    effects_sha="$(sha256sum "${evidence_dir}/taker_sells_lez-actual-effects.json" | sed 's/ .*//')"
    [[ "$(jq -er '.bindings.actual_effects.sha256' "$terminal_file")" == "$effects_sha" ]] ||
      fail "reverse terminal balances do not bind the retained actual effects"
    lez_terminal_balance_summary="$(jq -c --arg path "${relative_run_root}/evidence/taker_sells_lez-custom-token-terminal-balances.json" --arg sha "$terminal_sha" '
      {evidence_path:$path,evidence_sha256:$sha,direction:.direction,asset:.asset,
       balances:.balances,conservation_total:.conservation_total,
       exact_direction_balances:.exact_direction_balances,
       owner_balance_source:.owner_balance_source,
       custody_balance_source:.custody_balance_source,
       bindings:.bindings,
       actual_effects_sha256:.bindings.actual_effects.sha256}
    ' "$terminal_file")"
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
  foreign_stage2_sha="$(sha256sum \
    "${evidence_dir}/taker_sells_foreign-stage-two.json" | sed 's/ .*//')"
  if [[ "$m5_btc_application_mode" != 1 ]]; then
    lez_stage2_sha="$(sha256sum \
      "${evidence_dir}/taker_sells_lez-stage-two.json" | sed 's/ .*//')"
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
    --arg m5_btc_application_mode "$m5_btc_application_mode" \
    --arg deployment_profile "$expected_lez_deployment_profile" \
    --arg lez_guest_sha256 "$expected_lez_guest_sha256" \
    --arg lez_program_id "$expected_lez_program_id" \
    --arg lez_deployer_sha256 "$expected_lez_deployer_sha256" \
    --arg direction_driver "scripts/run-m3-actor-direction.sh" \
    --arg direction_driver_sha "$direction_driver_sha" \
    --arg lez_bootstrap "scripts/run-m3-lez-bootstrap.sh" \
    --arg lez_bootstrap_sha "$lez_bootstrap_sha" \
    --arg bitcoin_service_driver "scripts/run-bitcoin-core-e2e.sh" \
    --arg bitcoin_service_driver_sha "$bitcoin_service_driver_sha" \
    --arg lez_service_driver "scripts/run-lez-v02-stack.sh" \
    --arg lez_service_driver_sha "$lez_service_driver_sha" \
    --arg f7_token_fixture "scripts/run-m3-f7-token-fixture.sh" \
    --arg f7_token_fixture_driver_sha "$f7_token_fixture_driver_sha" \
    --arg official_wallet_cache "scripts/prepare-m3-official-wallet-artifact.sh" \
    --arg official_wallet_cache_helper_sha "$official_wallet_cache_helper_sha" \
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
    --argjson official_wallet_cache_summary "$official_wallet_cache_summary" \
    --argjson phase_timing_summary "$phase_timing_summary" \
    --argjson actor_direction_timing_summary "$actor_direction_timing_summary" \
    --arg foreign_stage2_sha "$foreign_stage2_sha" \
    --arg lez_stage2_sha "$lez_stage2_sha" --argjson foreign_terminal_balance "$foreign_terminal_balance_summary" --argjson lez_terminal_balance "$lez_terminal_balance_summary" '
    {
      schema_version: 1,
      kind: $packet_kind,
      journey: $journey,
      schedule: $schedule,
      asset_mode: $asset_mode,
      m5_btc_application_mode: ($m5_btc_application_mode == "1"),
      application:
        (if $m5_btc_application_mode == "1" then {
          pair:"bitcoin",
          direction:"taker_sells_foreign",
          lez_deployment:{
            profile:$deployment_profile,
            guest_sha256:$lez_guest_sha256,
            program_id:$lez_program_id,
            deployer_sha256:$lez_deployer_sha256}
        } else null end),
      result: "passed",
      run_id: $run_id,
      repository_commit: $repository_commit,
      completed_at: $completed_at,
      execution_provenance:{repository_clean_exact_head:true,
        origin_main_equals_head:true,
        executable_hashes_stable_from_start_to_publication:true},
      certified_executable_scripts: ({
        outer_runner: {repository_path:$outer_runner,sha256:$outer_runner_sha},
        direction_driver: {repository_path:$direction_driver,sha256:$direction_driver_sha},
        lez_bootstrap: {repository_path:$lez_bootstrap,sha256:$lez_bootstrap_sha},
        bitcoin_service_driver: {
          repository_path:$bitcoin_service_driver,sha256:$bitcoin_service_driver_sha},
        lez_service_driver: {
          repository_path:$lez_service_driver,sha256:$lez_service_driver_sha},
        external_override_allowed: false
      } + (if $asset_mode == "custom_token" then {
        f7_token_fixture:{repository_path:$f7_token_fixture,
          sha256:$f7_token_fixture_driver_sha},
        official_wallet_cache:{repository_path:$official_wallet_cache,
          sha256:$official_wallet_cache_helper_sha}
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
          cargo_locked_offline:true,
          content_addressed_executable_only_cache:true,
          target_in_exact_cleaned_secure_root:true,
          artifact_cache:$official_wallet_cache_summary}
      } else {} end)),
      asset:
        (if $asset_mode == "custom_token" then {custom_token:$f7_token_fixture_summary}
         else {native:{base_agreement_terms:true}} end),
      services: {
        bitcoin_core: {run_id: $bitcoin_run, version: "31.1", network: "regtest"},
        lez: {run_id: $lez_run, version: "v0.2.0", network: "private_local",
              slot_duration_seconds:$lez_slot_duration_seconds,
              deployment_profile:$deployment_profile}
      },
      directions: ([
        ({direction: "taker_sells_foreign", terminal_revision: $terminal_revision,
         terminal_phase: $terminal_phase,
         expected_unique_effects:
           (if $asset_mode == "custom_token" then {bitcoin:2,lez:4}
            elif $journey == "first_lock_refund" then {bitcoin:2,lez:0}
            else {bitcoin:2,lez:3} end),
         maker_second_lock_effect_count:
           (if $journey == "first_lock_refund" then 0 else 1 end),
         stage_two_evidence_sha256: $foreign_stage2_sha}
         + (if $asset_mode == "custom_token" then
              {custom_token_terminal_balances:$foreign_terminal_balance}
            else {} end)),
        ({direction: "taker_sells_lez", terminal_revision: $terminal_revision,
         terminal_phase: $terminal_phase,
         expected_unique_effects:
           (if $asset_mode == "custom_token" then {bitcoin:2,lez:4}
            elif $journey == "first_lock_refund" then {bitcoin:0,lez:3}
            else {bitcoin:2,lez:3} end),
         maker_second_lock_effect_count:
           (if $journey == "first_lock_refund" then 0 else 1 end),
         stage_two_evidence_sha256: $lez_stage2_sha}
         + (if $asset_mode == "custom_token" then
              {custom_token_terminal_balances:$lez_terminal_balance}
            else {} end))
      ] | if $m5_btc_application_mode == "1" then .[0:1] else . end),
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
        (if $m5_btc_application_mode == "1" then
           {taker_sells_foreign:{bitcoin:2,lez:3}}
         elif $asset_mode == "custom_token" then
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
      performance:{phase_timings:$phase_timing_summary,
        actor_direction_timings:$actor_direction_timing_summary},
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
  phase_timings_hash_stable "$phase_timing_sha" ||
    fail "phase timing evidence changed before main packet publication"
  actor_direction_phase_timings_hash_stable "$actor_direction_timing_summary" ||
    fail "actor direction timing evidence changed before main packet publication"
  chmod 0600 "${run_evidence}.partial"
  mv "${run_evidence}.partial" "$run_evidence"
  jq -e --arg journey "$journey" --arg schedule "$schedule" \
    --arg asset_mode "$asset_mode" \
    --arg m5_btc_application_mode "$m5_btc_application_mode" \
    --arg deployment_profile "$expected_lez_deployment_profile" \
    --arg lez_guest_sha256 "$expected_lez_guest_sha256" \
    --arg lez_program_id "$expected_lez_program_id" \
    --arg lez_deployer_sha256 "$expected_lez_deployer_sha256" \
    --arg packet_kind "$packet_kind" \
    --arg repository_commit "$repository_commit_at_start" \
    --arg outer_runner_sha "$outer_runner_sha_at_start" \
    --arg direction_driver_sha "$direction_driver_sha_at_start" \
    --arg lez_bootstrap_sha "$lez_bootstrap_sha_at_start" \
    --arg bitcoin_service_driver_sha "$bitcoin_service_driver_sha_at_start" \
    --arg lez_service_driver_sha "$lez_service_driver_sha_at_start" \
    --argjson phase_timing_summary "$phase_timing_summary" \
    --argjson actor_direction_timing_summary "$actor_direction_timing_summary" \
    --argjson terminal_revision "$terminal_revision" \
    --arg terminal_phase "$terminal_phase" --arg replay_command "$replay_command" \
    --arg actor_owned_effect_semantics "$actor_owned_effect_semantics" '
    .schema_version == 1
    and .kind == $packet_kind
    and .journey == $journey
    and .schedule == $schedule
    and .asset_mode == $asset_mode
    and .m5_btc_application_mode == ($m5_btc_application_mode == "1")
    and .services.lez.deployment_profile == $deployment_profile
    and (if $m5_btc_application_mode == "1" then
      .application == {
        pair:"bitcoin",
        direction:"taker_sells_foreign",
        lez_deployment:{
          profile:$deployment_profile,
          guest_sha256:$lez_guest_sha256,
          program_id:$lez_program_id,
          deployer_sha256:$lez_deployer_sha256}}
      and .directions[0].direction == "taker_sells_foreign"
    else .application == null end)
    and .result == "passed"
    and .repository_commit == $repository_commit
    and .execution_provenance == {repository_clean_exact_head:true,
      origin_main_equals_head:true,
      executable_hashes_stable_from_start_to_publication:true}
    and .performance == {phase_timings:$phase_timing_summary,
      actor_direction_timings:$actor_direction_timing_summary}
    and .certified_executable_scripts.outer_runner.sha256 == $outer_runner_sha
    and .certified_executable_scripts.direction_driver.sha256 == $direction_driver_sha
    and .certified_executable_scripts.lez_bootstrap.sha256 == $lez_bootstrap_sha
    and .certified_executable_scripts.bitcoin_service_driver.sha256 ==
      $bitcoin_service_driver_sha
    and .certified_executable_scripts.lez_service_driver.sha256 == $lez_service_driver_sha
    and (.directions | length) ==
      (if $m5_btc_application_mode == "1" then 1 else 2 end)
    and all(.directions[];
      .terminal_revision == $terminal_revision and .terminal_phase == $terminal_phase)
    and (if $asset_mode == "custom_token" then
      .directions[0].direction == "taker_sells_foreign"
      and .directions[0].custom_token_terminal_balances.direction == "taker_sells_foreign"
      and .directions[0].custom_token_terminal_balances.asset == "M3F7A"
      and .directions[0].custom_token_terminal_balances.balances ==
        {maker:175,taker:75,custody:0}
      and .directions[1].direction == "taker_sells_lez"
      and .directions[1].custom_token_terminal_balances.direction == "taker_sells_lez"
      and .directions[1].custom_token_terminal_balances.asset == "M3F7B"
      and .directions[1].custom_token_terminal_balances.balances ==
        {maker:75,taker:175,custody:0}
      and all(.directions[].custom_token_terminal_balances;
        .conservation_total == 250 and .exact_direction_balances == true
        and .owner_balance_source.reader == "official_lez_v0_2_wallet"
        and .owner_balance_source.same_atomic_snapshot_as_finalized_claim == false
        and .owner_balance_source.isolated_run_quiescent_after_terminal_replay == true
        and .custody_balance_source.reader == "finalized_actor_chain_evidence"
        and .custody_balance_source.finalized == true
        and .custody_balance_source.claim_transfer_atomicity ==
          "single_on_chain_claim_transaction"
        and (.evidence_path | startswith(".e2e/"))
        and (.evidence_sha256 | test("^[0-9a-f]{64}$"))
        and (.bindings.actor_submit.sha256 | test("^[0-9a-f]{64}$"))
        and (.bindings.lez_claim_finality.sha256 | test("^[0-9a-f]{64}$"))
        and (.bindings.actual_effects.sha256 | test("^[0-9a-f]{64}$"))
        and .actual_effects_sha256 == .bindings.actual_effects.sha256)
    else all(.directions[]; has("custom_token_terminal_balances") | not) end)
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
      (if $m5_btc_application_mode == "1" then
         {taker_sells_foreign:{bitcoin:2,lez:3}}
       elif $asset_mode == "custom_token" then
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
      and .native_build_prerequisites.official_wallet.content_addressed_executable_only_cache == true
      and .native_build_prerequisites.official_wallet.target_in_exact_cleaned_secure_root == true
      and .native_build_prerequisites.official_wallet.artifact_cache.result == "prepared"
      and (.native_build_prerequisites.official_wallet.artifact_cache.cache_hit |
        type == "boolean")
      and .native_build_prerequisites.official_wallet.artifact_cache.test_mode == false
      and .native_build_prerequisites.official_wallet.artifact_cache.validation_policy_revision == 2
      and .native_build_prerequisites.official_wallet.artifact_cache.publisher_helper_sha256 ==
        .certified_executable_scripts.official_wallet_cache.sha256
      and .native_build_prerequisites.official_wallet.artifact_cache.input.validation_policy_revision == 2
      and .native_build_prerequisites.official_wallet.artifact_cache.input.publisher_helper_sha256 ==
        .native_build_prerequisites.official_wallet.artifact_cache.publisher_helper_sha256
      and .native_build_prerequisites.official_wallet.artifact_cache.input.build.expected_wallet_sha256 ==
        .native_build_prerequisites.official_wallet.artifact_cache.wallet_sha256
      and (.native_build_prerequisites.official_wallet.artifact_cache.input.toolchain.target_libdir_sha256 |
        test("^[0-9a-f]{64}$"))
      and (.native_build_prerequisites.official_wallet.artifact_cache.input_key |
        test("^[0-9a-f]{64}$"))
      and (.native_build_prerequisites.official_wallet.artifact_cache.wallet_sha256 |
        test("^[0-9a-f]{64}$"))
      and (.native_build_prerequisites.official_wallet.artifact_cache.object_manifest_sha256 |
        test("^[0-9a-f]{64}$"))
      and (.native_build_prerequisites.official_wallet.artifact_cache.runtime_fingerprint_sha256 |
        test("^[0-9a-f]{64}$"))
      and (.native_build_prerequisites.official_wallet.artifact_cache.duration_ms | numbers) >= 0
      and (.native_build_prerequisites.official_wallet.artifact_cache.artifact_bytes | numbers) > 0
      and .native_build_prerequisites.official_wallet.artifact_cache.private_copy == true
      and .native_build_prerequisites.official_wallet.artifact_cache.hardlink == false
      and .native_build_prerequisites.official_wallet.artifact_cache.source_rehashed_after_copy == true
      and .native_build_prerequisites.official_wallet.artifact_cache.destination_rehashed == true
      and .native_build_prerequisites.official_wallet.artifact_cache.secrets_or_state_cached == false
      and (.native_build_prerequisites.official_wallet.artifact_cache.evidence_sha256 |
        test("^[0-9a-f]{64}$"))
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
  phase_timings_hash_stable "$phase_timing_sha" ||
    fail "phase timing evidence changed after main packet publication"
  actor_direction_phase_timings_hash_stable "$actor_direction_timing_summary" ||
    fail "actor direction timing evidence changed after main packet publication"
}

phase_timing_begin contract_validation || fail "contract-validation timing start failed"
verify_direction_driver_contract
verify_lez_bootstrap_contract
phase_timing_end contract_validation || fail "contract-validation timing end failed"

phase_timing_begin prebuild || fail "prebuild timing start failed"
prebuild
assert_prebuilt
phase_timing_end prebuild || fail "prebuild timing end failed"

phase_timing_begin identities_stage_one || fail "identity timing start failed"
provision_actor_identities

# Both agreements receive independent stage-one private material before any
# node fact or endpoint exists. The exact pinned official NSSA mapping is then
# recorded before either agreement can be finalized.
for direction in "${directions[@]}"; do
  run_stage_one "$direction"
  run_official_nssa_mapping "$direction"
done
phase_timing_end identities_stage_one || fail "identity timing end failed"

phase_timing_begin node_startup || fail "node-startup timing start failed"
start_actual_nodes
phase_timing_end node_startup || fail "node-startup timing end failed"
phase_timing_begin bitcoin_funding || fail "Bitcoin-funding timing start failed"
provision_bitcoin_funding_sources
phase_timing_end bitcoin_funding || fail "Bitcoin-funding timing end failed"
phase_timing_begin lez_bootstrap || fail "LEZ-bootstrap timing start failed"
bootstrap_lez_runtime
phase_timing_end lez_bootstrap || fail "LEZ-bootstrap timing end failed"
if [[ "$asset_mode" == "custom_token" ]]; then
  phase_timing_begin f7_fixture || fail "F7-fixture timing start failed"
fi
provision_f7_token_fixture
if [[ "$asset_mode" == "custom_token" ]]; then
  phase_timing_end f7_fixture || fail "F7-fixture timing end failed"
fi

# Directions share only the actual local nodes. The sequential schedule retains
# the historical one-direction-at-a-time proof. The overlap schedule keeps both
# direction controllers alive and withholds settlement until both independent
# swaps have durably reached revision two.
if [[ "$schedule" == "overlap" ]]; then
  phase_timing_begin directions_overlap || fail "overlap timing start failed"
  reserve_bitcoin_funding_anchors overlap
  run_overlapping_actor_flows
  phase_timing_end directions_overlap || fail "overlap timing end failed"
else
  for direction in "${directions[@]}"; do
    phase_timing_begin "direction_${direction}_reserve_funding" "$direction" ||
      fail "${direction} funding-reservation timing start failed"
    reserve_bitcoin_funding_anchors sequential "$direction"
    phase_timing_end "direction_${direction}_reserve_funding" ||
      fail "${direction} funding-reservation timing end failed"
    if [[ "$m5_btc_application_mode" == 1 ]]; then
      prepare_m5_btc_delivery_plan "$direction"
    fi
    phase_timing_begin "direction_${direction}_stage_two" "$direction" ||
      fail "${direction} stage-two timing start failed"
    run_stage_two "$direction"
    phase_timing_end "direction_${direction}_stage_two" ||
      fail "${direction} stage-two timing end failed"
    phase_timing_begin "direction_${direction}_actor_flow" "$direction" ||
      fail "${direction} actor-flow timing start failed"
    run_direction_actor_flow "$direction"
    phase_timing_end "direction_${direction}_actor_flow" ||
      fail "${direction} actor-flow timing end failed"
    phase_timing_begin "direction_${direction}_terminal_replay" "$direction" ||
      fail "${direction} terminal-replay timing start failed"
    assert_terminal_and_replay "$direction"
    phase_timing_end "direction_${direction}_terminal_replay" ||
      fail "${direction} terminal-replay timing end failed"
    if [[ "$asset_mode" == "custom_token" ]]; then
      phase_timing_begin "direction_${direction}_terminal_balances" "$direction" ||
        fail "${direction} terminal-balance timing start failed"
      write_custom_token_terminal_balance_evidence "$direction"
      phase_timing_end "direction_${direction}_terminal_balances" ||
        fail "${direction} terminal-balance timing end failed"
    fi
  done
fi

phase_timing_begin effect_validation || fail "effect-validation timing start failed"
validate_actual_effect_manifests
phase_timing_end effect_validation || fail "effect-validation timing end failed"
finalize_phase_timings || fail "phase timing evidence publication failed"
write_run_evidence
echo "${success_label} passed: ${run_evidence}"
