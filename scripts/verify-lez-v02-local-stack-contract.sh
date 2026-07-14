#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

contract="compat/lez-v0.2-provisional/local-stack.toml"
lockfile="compat/lez-v0.2-provisional/Cargo.lock"
lez_source="${LEZ_V02_SOURCE_DIR:?set LEZ_V02_SOURCE_DIR to an exact LEZ v0.2.0 checkout}"
r0vm_bin="${LEZ_V02_R0VM:?set LEZ_V02_R0VM to the verified r0vm 3.0.5 binary}"
sequencer_binary="${LEZ_V02_SEQUENCER_BINARY:?set LEZ_V02_SEQUENCER_BINARY to the verified locked-build service binary}"
indexer_binary="${LEZ_V02_INDEXER_BINARY:?set LEZ_V02_INDEXER_BINARY to the verified locked-build service binary}"
rapidsnark_archive="${LEZ_V02_RAPIDSNARK_ARCHIVE:?set LEZ_V02_RAPIDSNARK_ARCHIVE to the verified v0.0.8 release zip}"
rapidsnark_lib_dir="${RAPIDSNARK_LIB_DIR:?set RAPIDSNARK_LIB_DIR to the verified extracted v0.0.8 library directory}"
bindgen_extra_clang_args="${BINDGEN_EXTRA_CLANG_ARGS:?set BINDGEN_EXTRA_CLANG_ARGS to the contracted GCC include path}"
run_id="${RUN_ID:-source-contract}"
lez_commit="a58fbce2ff48c58b7bb5001b1a27e64b9596ee3a"
lez_tag="v0.2.0"
logos_commit="d8711bbc3d43d3ef9755ef9b73af32fd0f703160"
rust_toolchain="1.94.0"
bedrock_image="ghcr.io/logos-blockchain/logos-blockchain@sha256:91d6c5bf07e07fcfba5e7cf07d21ee686a6bc4b9f6210f2d28bffbcad9a3729f"
runtime_base="gcr.io/distroless/cc-debian13:nonroot@sha256:aded2458d026e046cb68199db0e5793e1028ffa143f7258f3c4278253e20add7"
r0vm_sha256="36c016a5bb2ded5bd1f8f92cc487e6ffaeb1e95ec05850c983081a0f716b515b"
rapidsnark_revision="e91187f8ccb5bbfc7bb00dac88169112428da78f"
rapidsnark_archive_name="rapidsnark-linux-x86_64-pic-v0.0.8.zip"
rapidsnark_archive_sha256="59bdd709eed96235de061f352893f4650c923b54b591052118593012bb1cd831"

if [[ ! "$run_id" =~ ^[a-z0-9][a-z0-9_-]{0,63}$ ]]; then
  echo "RUN_ID must match the local-stack contract" >&2
  exit 1
fi
project_name="lez-atomic-swaps-lez-v02-${run_id}"

for command in cargo cut docker git rg rustup sha256sum; do
  command -v "$command" >/dev/null || {
    echo "${command} is required by the LEZ v0.2 source-contract verifier" >&2
    exit 1
  }
done

[[ -f "$contract" ]] || {
  echo "missing tracked LEZ v0.2 local-stack contract" >&2
  exit 1
}
[[ -d "$lez_source/.git" || -f "$lez_source/.git" ]] || {
  echo "LEZ_V02_SOURCE_DIR must be a git checkout" >&2
  exit 1
}
[[ -x "$r0vm_bin" ]] || {
  echo "LEZ_V02_R0VM must be an executable verified artifact" >&2
  exit 1
}
[[ -x "$sequencer_binary" && -x "$indexer_binary" ]] || {
  echo "LEZ v0.2 service binaries must be executable verified artifacts" >&2
  exit 1
}
[[ -f "$rapidsnark_archive" ]] || {
  echo "LEZ_V02_RAPIDSNARK_ARCHIVE must be a verified release zip" >&2
  exit 1
}
[[ -d "$rapidsnark_lib_dir" ]] || {
  echo "RAPIDSNARK_LIB_DIR must be a verified extracted library directory" >&2
  exit 1
}
if [[ "${rapidsnark_archive##*/}" != "$rapidsnark_archive_name" ]]; then
  echo "rapidsnark archive must retain its contracted release asset name" >&2
  exit 1
fi
if [[ "$bindgen_extra_clang_args" != "-I/usr/lib/gcc/x86_64-linux-gnu/13/include" ]]; then
  echo "BINDGEN_EXTRA_CLANG_ARGS does not match the contracted build include" >&2
  exit 1
fi

actual_commit="$(git -C "$lez_source" rev-parse HEAD)"
if [[ "$actual_commit" != "$lez_commit" ]]; then
  echo "expected LEZ ${lez_tag} commit ${lez_commit}, got ${actual_commit}" >&2
  exit 1
fi
source_status="$(git -C "$lez_source" status --porcelain=v1 --untracked-files=all)"
if [[ -n "$source_status" ]]; then
  echo "LEZ source checkout must be clean, including no untracked files:" >&2
  printf '%s\n' "$source_status" >&2
  exit 1
fi

if git -C "$lez_source" show-ref --verify --quiet "refs/tags/${lez_tag}"; then
  tag_commit="$(git -C "$lez_source" rev-parse "${lez_tag}^{commit}")"
  if [[ "$tag_commit" != "$lez_commit" ]]; then
    echo "local ${lez_tag} resolves to ${tag_commit}, expected ${lez_commit}" >&2
    exit 1
  fi
  local_tag_state="present_and_verified"
else
  local_tag_state="absent_and_reported"
fi

rg -Fqx "tag = \"${lez_tag}\"" "$contract"
rg -Fqx "commit = \"${lez_commit}\"" "$contract"
rg -Fqx 'source_verification = "clean_git_commit_plus_file_sha256"' "$contract"
rg -Fqx 'local_tag_policy = "verify_commit_when_present_report_absence"' "$contract"
rg -Fqx 'rust_toolchain_channel = "1.94.0"' "$contract"
rg -Fqx 'cargo_toolchain_channel = "1.94.0"' "$contract"
rg -Fqx 'cargo_build_command = "cargo +1.94.0 build --locked --release --jobs 2 --package sequencer_service --package indexer_service"' "$contract"
rg -Fqx 'contract_status = "isolated_service_readiness_green_full_runtime_tuple_pending"' "$contract"
rg -Fqx "stack_kind = \"bedrock_settled_non_standalone\"" "$contract"
rg -Fqx 'compose_project_template = "configuration_validation_only_lez-atomic-swaps-lez-v02-{run_id}"' "$contract"
rg -Fqx 'container_group_template = "lez-atomic-swaps-lez-v02-{run_id}"' "$contract"
rg -Fqx 'network_template = "lez-atomic-swaps-lez-v02-{run_id}-private"' "$contract"
rg -Fqx 'state_root_template = ".e2e/{run_id}/lez-v02"' "$contract"
rg -Fqx "host_port_policy = \"dynamic_zero_only\"" "$contract"
rg -Fqx "container_name_policy = \"no_fixed_or_global_names_exact_run_scoped_names_required\"" "$contract"
rg -Fqx 'cleanup_scope = "captured_exact_container_ids_then_exact_network_and_image"' "$contract"
rg -Fqx "source_commit = \"${logos_commit}\"" "$contract"
rg -Fqx "image_revision_label = \"${logos_commit}\"" "$contract"
rg -Fqx 'source_mapping_status = "oci_label_verified_matches_lez_locked_revision"' "$contract"
rg -Fqx 'image_metadata_verification = "requires_local_docker_image_inspect"' "$contract"
rg -Fqx 'image_source_label = "https://github.com/logos-blockchain/logos-blockchain"' "$contract"
rg -Fqx 'image_version_label = "master"' "$contract"
rg -Fqx 'image_licenses_label = "Apache-2.0"' "$contract"
rg -Fqx 'public_runtime_parity_status = "unverified_upstream_tagged_readme_calls_bundled_node_outdated"' "$contract"
rg -Fqx "runtime_base_reference = \"${runtime_base}\"" "$contract"
rg -Fqx "r0vm_sha256 = \"${r0vm_sha256}\"" "$contract"
rg -Fqx 'binary_sha256_status = "locked_source_build_hashes_bound_and_executed_in_isolated_service_stack"' "$contract"
rg -Fqx 'sequencer_binary_sha256 = "3727e9aa10600d04d0cdfda6eb39df146ef4cc14f5b09ad33bcf076a8f2c412f"' "$contract"
rg -Fqx 'indexer_binary_sha256 = "6ed54f04ae018f3554898a9f0aef6decd6930c4e8609326d146ca164e48d7442"' "$contract"
rg -Fqx 'runtime_smoke_status = "distroless_services_executed_isolated_service_readiness_green"' "$contract"
rg -Fqx '  "network_none",' "$contract"
rg -Fqx '  "unique_private_bridge",' "$contract"
rg -Fqx '  "ip_masquerade_disabled",' "$contract"
rg -Fqx '  "dynamic_loopback_publication",' "$contract"
rg -Fqx '  "resource_limits",' "$contract"
rg -Fqx 'mode = "runner_generated_minimal_not_upstream_example_copy"' "$contract"
rg -Fqx 'bedrock_node_url = "http://bedrock:18080"' "$contract"
rg -Fqx 'channel_id = "b6adb2d238911395adde0b2f40b880ec03ffd1a3a8d97e7df8cacadf08873748"' "$contract"
rg -Fqx 'channel_id_source = "ed25519_public_key_of_deterministic_local_bedrock_signing_seed"' "$contract"
rg -Fqx 'bedrock_genesis_channel_id = "0000000000000000000000000000000000000000000000000000000000000000"' "$contract"
rg -Fqx 'upstream_example_channel_id = "0101010101010101010101010101010101010101010101010101010101010101"' "$contract"
rg -Fqx 'bedrock_signing_key_file_sha256 = "8fd0d8a6423536c14b5d3979e5135bf37253f5dfbc8485b52202bbf963b8f02e"' "$contract"
rg -Fqx 'fresh_state_policy = "refuse_preexisting_run_state"' "$contract"
rg -Fqx 'filesystem_policy = "private_run_scoped_paths"' "$contract"
rg -Fqx 'genesis_policy = "exact_two_public_supply_accounts_fresh_state_only"' "$contract"
rg -Fqx 'actor_genesis_status = "runtime_green_sequencer_and_exact_finalized_indexer_preclaim_state"' "$contract"
rg -Fqx 'indexer_preclaim_binding = "getAccountAtBlock_exact_last_finalized_block_id"' "$contract"
rg -Fqx '[actors.maker]' "$contract"
rg -Fqx 'account_id = "B1UN3hPgxacgHKBRoThcAmsPajGcUf6YXUhgB36x4DAd"' "$contract"
rg -Fqx 'vault_account_id = "7Mzr43PK9VxpcvwdjgL8PeE4nb2aG9FqBKLfkoH8RBmQ"' "$contract"
rg -Fqx 'genesis_allocation = 100000' "$contract"
rg -Fqx '[actors.taker]' "$contract"
rg -Fqx 'account_id = "34Kqgek6R7N1zU5FSJz8ziXwSPEPCuWGcn1T7GCVrfib"' "$contract"
rg -Fqx 'vault_account_id = "AXLjVw4tKTgieQoGRgXMVLVVaB4c5YnL1YTogZdX1cpH"' "$contract"
rg -Fqx 'genesis_allocation = 200000' "$contract"
rg -Fqx 'config_sha256 = "3ddeb4d9159cdd584dc9423deaac0897896edfd4cd27d2a509bec08077e1b49d"' "$contract"
rg -Fqx 'builder_sha256 = "7c72530e5ccdb72dda636511dd237b913e5865b18430f5920b50ffb4ade97df3"' "$contract"
rg -Fqx 'faucet_program_sha256 = "4cc6e9fbb404ea03468ccdd886c1d6426de736a5b7ac3564d39d04f58ed33936"' "$contract"
rg -Fqx 'vault_core_sha256 = "36bdae7c0c2dafeea98f97d1964388f0a21203f312b230e603923760c5073846"' "$contract"
rg -Fqx 'runtime_fixture_policy = "exact_mounts_direct_bedrock_binary_invocation"' "$contract"
rg -Fqx 'backoff_field_policy = "omitted_unsupported_upstream_example_field"' "$contract"
if rg -Fq 'backoff_policy =' "$contract"; then
  echo "unsupported upstream backoff field policy must be omitted" >&2
  exit 1
fi
rg -Fq '"http:GET_runtime_channel_before_sequencer_returns_404_or_500_with_exact_17_byte_channel_not_found_body",' "$contract"
rg -Fq '"http:GET_runtime_channel_after_sequencer_returns_200_with_accredited_key_equal_channel_id",' "$contract"
rg -Fq '"http:GET_runtime_channel_after_finality_tip_message_or_slot_advances",' "$contract"
rg -Fq '"rpc:getBlock(1)_returns_genesis",' "$contract"
rg -Fq '"rpc:getProgramIds_contains_required_builtins",' "$contract"
rg -Fq '"rpc:getLastFinalizedBlockId_returns_some_id_greater_than_or_equal_to_2",' "$contract"
rg -Fq '"rpc:getBlockById(last_finalized)_returns_decoded_block",' "$contract"
rg -Fq '"rpc:getBlockByHash(decoded_hash)_equals_getBlockById_semantically",' "$contract"
rg -Fq '"cross_check:indexer_header_id_prev_hash_hash_signature_match_sequencer_borsh_header_offsets",' "$contract"
rg -Fqx 'full_runtime_tuple_green = false' "$contract"
rg -Fqx 'pending = ["vault_claims", "checked_escrow_deployment", "maker_actor", "taker_actor", "swap_effects", "restart_recovery"]' "$contract"
if sed -n '/\[services.indexer\]/,/\[services.sequencer\]/p' "$contract" | rg -Fq '"rpc:checkHealth"'; then
  echo "indexer local-DB health must not be used as chain readiness" >&2
  exit 1
fi
rg -Fqx "dependency_revision = \"${rapidsnark_revision}\"" "$contract"
rg -Fqx 'crate_version = "0.1.3"' "$contract"
rg -Fqx 'version = "0.0.8"' "$contract"
rg -Fqx "archive_name = \"${rapidsnark_archive_name}\"" "$contract"
rg -Fqx 'archive_url = "https://github.com/logos-blockchain/logos-blockchain-rust-rapidsnark/releases/download/rapidsnark-pic-v0.0.8/rapidsnark-linux-x86_64-pic-v0.0.8.zip"' "$contract"
rg -Fqx "archive_sha256 = \"${rapidsnark_archive_sha256}\"" "$contract"
rg -Fqx 'bindgen_extra_clang_args = "-I/usr/lib/gcc/x86_64-linux-gnu/13/include"' "$contract"
rg -Fqx '"librapidsnark.a" = "d4133227f845ff5bfa3672eb5b9c018a6a086bfa164b176bdaf76949c7d1f423"' "$contract"
rg -Fqx '"libgmp.a" = "0a910b420c3ad603c83c9dc2818c7ae05394c231ca23135c7b873e8e680ea41b"' "$contract"
rg -Fqx '"libfq.a" = "797b5d24bb8e8b088f811bddfff35f33973af9c797fb3812489cd42ba6a957d0"' "$contract"
rg -Fqx '"libfr.a" = "40f809394904682cb5517845cd3c2f936a5eb4609712534b573f552f2811fb82"' "$contract"

bedrock_source_label="$(docker image inspect --format '{{ index .Config.Labels "org.opencontainers.image.source" }}' "$bedrock_image")"
bedrock_revision_label="$(docker image inspect --format '{{ index .Config.Labels "org.opencontainers.image.revision" }}' "$bedrock_image")"
bedrock_version_label="$(docker image inspect --format '{{ index .Config.Labels "org.opencontainers.image.version" }}' "$bedrock_image")"
bedrock_licenses_label="$(docker image inspect --format '{{ index .Config.Labels "org.opencontainers.image.licenses" }}' "$bedrock_image")"
[[ "$bedrock_source_label" == "https://github.com/logos-blockchain/logos-blockchain" ]] || {
  echo "Bedrock OCI source label drift: ${bedrock_source_label}" >&2
  exit 1
}
[[ "$bedrock_revision_label" == "$logos_commit" ]] || {
  echo "Bedrock OCI revision label drift: ${bedrock_revision_label}" >&2
  exit 1
}
[[ "$bedrock_version_label" == "master" ]] || {
  echo "Bedrock OCI version label drift: ${bedrock_version_label}" >&2
  exit 1
}
[[ "$bedrock_licenses_label" == "Apache-2.0" ]] || {
  echo "Bedrock OCI licenses label drift: ${bedrock_licenses_label}" >&2
  exit 1
}

rg -Fq "?tag=${lez_tag}#${lez_commit}" "$lockfile" || {
  echo "v0.2 lockfile does not map the tag to the contracted commit" >&2
  exit 1
}
logos_lock_identity="git+https://github.com/logos-blockchain/logos-blockchain.git?rev=${logos_commit}#${logos_commit}"
rg -Fq "$logos_lock_identity" "$lez_source/Cargo.lock"
rg -Fq "$logos_lock_identity" "$lockfile"
rapidsnark_lock_identity="git+https://github.com/logos-blockchain/logos-blockchain-rust-rapidsnark.git?rev=${rapidsnark_revision}#${rapidsnark_revision}"
rg -Fq "$rapidsnark_lock_identity" "$lez_source/Cargo.lock" || {
  echo "upstream LEZ lockfile does not pin the contracted rapidsnark revision" >&2
  exit 1
}
rg -Fq "$rapidsnark_lock_identity" "$lockfile" || {
  echo "v0.2 compatibility lockfile does not pin the contracted rapidsnark revision" >&2
  exit 1
}

actual_rustc_version="$(rustup run "$rust_toolchain" rustc --version)"
if [[ "$actual_rustc_version" != "rustc 1.94.0 "* ]]; then
  echo "contracted Rust toolchain is not rustc 1.94.0: ${actual_rustc_version}" >&2
  exit 1
fi
actual_cargo_version="$(cargo +"$rust_toolchain" --version)"
if [[ "$actual_cargo_version" != "cargo 1.94.0 "* ]]; then
  echo "contracted build command is not backed by cargo 1.94.0: ${actual_cargo_version}" >&2
  exit 1
fi

actual_r0vm_sha256="$(sha256sum "$r0vm_bin" | cut -d ' ' -f 1)"
if [[ "$actual_r0vm_sha256" != "$r0vm_sha256" ]]; then
  echo "r0vm 3.0.5 artifact identity drift" >&2
  exit 1
fi
if [[ "$("$r0vm_bin" --version)" != "risc0-r0vm 3.0.5" ]]; then
  echo "contracted r0vm does not report version 3.0.5" >&2
  exit 1
fi

verify_source_file() {
  local relative_path="$1"
  local expected_sha256="$2"
  local source_file="${lez_source}/${relative_path}"
  [[ -f "$source_file" ]] || {
    echo "missing contracted upstream source file ${relative_path}" >&2
    exit 1
  }
  local actual_sha256
  actual_sha256="$(sha256sum "$source_file" | cut -d ' ' -f 1)"
  if [[ "$actual_sha256" != "$expected_sha256" ]]; then
    echo "source identity drift for ${relative_path}: expected ${expected_sha256}, got ${actual_sha256}" >&2
    exit 1
  fi
}

verify_source_file "bedrock/docker-compose.yml" "c2d09752a4d2f994308efae88b6cd65c1a21e0159cf3343692a5fbd90b060b13"
verify_source_file "rust-toolchain.toml" "0dab14fcb283227f21263b19b22f4895e37b79ab95a4c700f4b2ac94b2ea1fce"
verify_source_file "bedrock/node-config.yaml" "93fd090eeef25e33137ca6d9437b03dc903f721459d7fb60d8b6464ae301354f"
verify_source_file "bedrock/deployment-settings.yaml" "a253c641c515957d1480a87f4b098a5cd9cbf9ac34bbc097dbe480d8869adbd3"
verify_source_file "bedrock/kzgrs_test_params" "45cce45664e7ac60c54d298ffd8f7d96ba6cad741cfebe5ea4e4d2820a667d53"
verify_source_file "bedrock/scripts/run_logos_blockchain_node.sh" "261585229656475f908c64984a59f6105b0dffd961b69104c2b22eedc050a061"
verify_source_file "lez/sequencer/service/Cargo.toml" "3166bb45eae3aeb44876c7d85938337eae730b0c2bf36c0748d18a553c6d0011"
verify_source_file "lez/sequencer/core/src/config.rs" "3ddeb4d9159cdd584dc9423deaac0897896edfd4cd27d2a509bec08077e1b49d"
verify_source_file "lez/sequencer/core/src/lib.rs" "7c72530e5ccdb72dda636511dd237b913e5865b18430f5920b50ffb4ade97df3"
verify_source_file "lez/programs/faucet/src/main.rs" "4cc6e9fbb404ea03468ccdd886c1d6426de736a5b7ac3564d39d04f58ed33936"
verify_source_file "lez/programs/vault/core/src/lib.rs" "36bdae7c0c2dafeea98f97d1964388f0a21203f312b230e603923760c5073846"
verify_source_file "lez/sequencer/service/Dockerfile" "4d83e7302ec7fe887336a6f1090c57bc1e425c366d10dd7db45dcfe1e95010bf"
verify_source_file "lez/sequencer/service/src/lib.rs" "e9472ea5a15061c617257de849d19e90af213565c765c302515f97e265e357c3"
verify_source_file "lez/indexer/service/Cargo.toml" "2e8af720e109d7ea40580e4c1d5db71d47897c71c76405279b2c5c4f51d20b37"
verify_source_file "lez/indexer/service/Dockerfile" "7690d9f1a726462d6c5dbd86f7ff452f818d12c6399767106a330c6a2b75f0c7"
verify_source_file "lez/indexer/service/src/lib.rs" "a0ce328d707e318e7a8653ec65968949efc772ec08d08d6755f021d52e9be514"
verify_source_file "lez/sequencer/service/rpc/src/lib.rs" "db50ad06b17a86086d84c8a7642def3af21beda4ffed4660cbc219255c06ffb4"
verify_source_file "lez/sequencer/core/src/block_publisher.rs" "57d4cf0e94755d19daa8af13f6c5b27cb349fe2c176436f79af5b871cb6971fb"
verify_source_file "lez/indexer/core/src/lib.rs" "cdb8238dc9bbb0f83b616b63d407f08505fa5a5dfa20343486fee31dcab1e7b4"
verify_source_file "lez/indexer/service/rpc/src/lib.rs" "a7a2a3114e0b65a9f33decff79bb5ffe5c75c54507f5054560c4e1fef1e0af3e"
verify_source_file "lez/configs/docker-all-in-one/sequencer_config.json" "542c06f58ebecfec443d5e8e201d6e5009b613c3d7bc7a17e6de04ca52c58331"
verify_source_file "lez/configs/docker-all-in-one/indexer_config.json" "4f4ca8a1bd94aaf6bd65d6c7315d99ac378e664822425cca6f9289647c7e9e40"
verify_source_file "lez/sequencer/service/configs/docker/sequencer_config.json" "101e136a2ef5071fdacfbc973184dcbc43b8e09690af0a30e5592c0fd1d2887a"
verify_source_file "lez/indexer/service/configs/docker/indexer_config.json" "428320c0e9db6490e0949c354fdeada8b55e47eb3a037d05677714601f3e620d"

rg -Fqx 'channel = "1.94.0"' "$lez_source/rust-toolchain.toml"

verify_artifact() {
  local artifact="$1"
  local expected_sha256="$2"
  [[ -f "$artifact" ]] || {
    echo "missing contracted build artifact ${artifact}" >&2
    exit 1
  }
  local actual_sha256
  actual_sha256="$(sha256sum "$artifact" | cut -d ' ' -f 1)"
  if [[ "$actual_sha256" != "$expected_sha256" ]]; then
    echo "build input identity drift for ${artifact}: expected ${expected_sha256}, got ${actual_sha256}" >&2
    exit 1
  fi
}

verify_artifact "$rapidsnark_archive" "$rapidsnark_archive_sha256"
verify_artifact "$rapidsnark_lib_dir/librapidsnark.a" "d4133227f845ff5bfa3672eb5b9c018a6a086bfa164b176bdaf76949c7d1f423"
verify_artifact "$rapidsnark_lib_dir/libgmp.a" "0a910b420c3ad603c83c9dc2818c7ae05394c231ca23135c7b873e8e680ea41b"
verify_artifact "$rapidsnark_lib_dir/libfq.a" "797b5d24bb8e8b088f811bddfff35f33973af9c797fb3812489cd42ba6a957d0"
verify_artifact "$rapidsnark_lib_dir/libfr.a" "40f809394904682cb5517845cd3c2f936a5eb4609712534b573f552f2811fb82"
verify_artifact "$sequencer_binary" "3727e9aa10600d04d0cdfda6eb39df146ef4cc14f5b09ad33bcf076a8f2c412f"
verify_artifact "$indexer_binary" "6ed54f04ae018f3554898a9f0aef6decd6930c4e8609326d146ca164e48d7442"
[[ "$("$sequencer_binary" --version)" == "sequencer_service 0.1.0" ]]
[[ "$("$indexer_binary" --version)" == "indexer_service 0.1.0" ]]

rg -Fq "image: ${bedrock_image}" "$lez_source/bedrock/docker-compose.yml"
rg -Fq 'standalone = ["sequencer_core/mock"]' "$lez_source/lez/sequencer/service/Cargo.toml"
rg -Fq 'mock-responses = []' "$lez_source/lez/indexer/service/Cargo.toml"
rg -Fq '#[cfg(not(feature = "standalone"))]' "$lez_source/lez/sequencer/service/src/lib.rs"
rg -Fq 'use sequencer_core::SequencerCore;' "$lez_source/lez/sequencer/service/src/lib.rs"
rg -Fq '.build(SocketAddr::from(([0, 0, 0, 0], port)))' "$lez_source/lez/sequencer/service/src/lib.rs"
rg -Fq '.build(SocketAddr::from(([0, 0, 0, 0], port)))' "$lez_source/lez/indexer/service/src/lib.rs"
rg -Fq 'pub struct ZoneSdkPublisher' "$lez_source/lez/sequencer/core/src/block_publisher.rs"
rg -Fq 'ZoneIndexer::new(config.channel_id, node)' "$lez_source/lez/indexer/core/src/lib.rs"
rg -Fq 'pub const GENESIS_BLOCK_ID: BlockId = 1;' "$lez_source/lee/state_machine/core/src/lib.rs"
rg -Fq 'pub enum GenesisAction' "$lez_source/lez/sequencer/core/src/config.rs"
rg -Fq 'GenesisAction::SupplyAccount {' "$lez_source/lez/sequencer/core/src/lib.rs"
rg -Fq 'vault_core::compute_vault_account_id(vault_program_id, *account_id)' "$lez_source/lez/sequencer/core/src/lib.rs"
rg -Fq 'faucet_core::Instruction::GenesisTransferVault {' "$lez_source/lez/sequencer/core/src/lib.rs"
rg -Fq 'Instruction::GenesisTransferVault {' "$lez_source/lez/programs/faucet/src/main.rs"
rg -Fq '&vault_core::Instruction::Transfer {' "$lez_source/lez/programs/faucet/src/main.rs"
rg -Fq '#[method(name = "getProgramIds")]' "$lez_source/lez/sequencer/service/rpc/src/lib.rs"
rg -Fq '#[method(name = "getAccount")]' "$lez_source/lez/sequencer/service/rpc/src/lib.rs"
rg -Fq '#[method(name = "getAccountsNonces")]' "$lez_source/lez/sequencer/service/rpc/src/lib.rs"
rg -Fq '#[method(name = "getLastFinalizedBlockId")]' "$lez_source/lez/indexer/service/rpc/src/lib.rs"
rg -Fq '#[method(name = "getBlockById")]' "$lez_source/lez/indexer/service/rpc/src/lib.rs"
rg -Fq '#[method(name = "getAccount")]' "$lez_source/lez/indexer/service/rpc/src/lib.rs"
rg -Fq '#[method(name = "getAccountAtBlock")]' "$lez_source/lez/indexer/service/rpc/src/lib.rs"
rg -Fq '.recalculate_final_state()' "$lez_source/lez/indexer/service/src/service.rs"

tests/e2e/lez-v02/test-actor-genesis-contract.sh

printf 'LEZ v0.2 local-stack contract verified: commit=%s tag=%s toolchain=%s project=%s verification_scope=source-contract-only\n' "$actual_commit" "$local_tag_state" "$rust_toolchain" "$project_name"
printf 'Bedrock source mapping is OCI-label attested to %s; exact cached image labels inspected without starting a container: %s\n' "$logos_commit" "$bedrock_image"
