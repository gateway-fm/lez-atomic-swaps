use std::{collections::BTreeMap, fs, path::Path};

use serde::Deserialize;

const CONTRACT_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/local-stack.toml");
const LEZ_TAG: &str = "v0.2.0";
const LEZ_COMMIT: &str = "a58fbce2ff48c58b7bb5001b1a27e64b9596ee3a";
const BEDROCK_IMAGE_DIGEST: &str =
    "sha256:91d6c5bf07e07fcfba5e7cf07d21ee686a6bc4b9f6210f2d28bffbcad9a3729f";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalStackContract {
    schema_version: u16,
    contract_status: String,
    stack_kind: String,
    public_transition: String,
    lez: LezSource,
    bedrock: BedrockSource,
    packaging: Packaging,
    generated_configuration: GeneratedConfiguration,
    isolation: Isolation,
    services: BTreeMap<String, Service>,
    flows: Vec<Flow>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LezSource {
    repository: String,
    tag: String,
    commit: String,
    cargo_source: String,
    source_verification: String,
    local_tag_policy: String,
    rust_toolchain_path: String,
    rust_toolchain_sha256: String,
    rust_toolchain_channel: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BedrockSource {
    repository: String,
    image_repository: String,
    image_digest: String,
    image_reference: String,
    source_commit: String,
    source_mapping_status: String,
    image_metadata_verification: String,
    image_source_label: String,
    image_revision_label: String,
    image_version_label: String,
    image_licenses_label: String,
    public_runtime_parity_status: String,
    upstream_compose_path: String,
    upstream_compose_sha256: String,
    runtime_fixture_policy: String,
    runner_script_status: String,
    runtime_fixtures: BTreeMap<String, SourceIdentity>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Isolation {
    run_id_pattern: String,
    compose_project_template: String,
    state_root_template: String,
    host_port_policy: String,
    container_name_policy: String,
    cleanup_scope: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GeneratedConfiguration {
    mode: String,
    bedrock_node_url: String,
    channel_id: String,
    channel_id_source: String,
    sequencer_state_dir: String,
    indexer_state_dir: String,
    backoff_field_policy: String,
    fresh_state_policy: String,
    filesystem_policy: String,
    source_examples_status: String,
    source_examples: BTreeMap<String, SourceIdentity>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Packaging {
    services: Vec<String>,
    cargo_locked: bool,
    cargo_build_jobs: u8,
    cargo_profile: String,
    cargo_toolchain_channel: String,
    cargo_build_command: String,
    rapidsnark: RapidsnarkInput,
    runtime_base_reference: String,
    r0vm_version: String,
    r0vm_sha256: String,
    binary_sha256_status: String,
    sequencer_binary_sha256: String,
    indexer_binary_sha256: String,
    sequencer_binary_version: String,
    indexer_binary_version: String,
    binary_runtime_dependency_status: String,
    runtime_smoke_status: String,
    runtime_smoke_security: Vec<String>,
    upstream_dockerfiles_status: String,
    upstream_dockerfiles: BTreeMap<String, SourceIdentity>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RapidsnarkInput {
    dependency_repository: String,
    dependency_revision: String,
    crate_version: String,
    version: String,
    archive_name: String,
    archive_url: String,
    archive_sha256: String,
    library_directory_environment: String,
    bindgen_extra_clang_args: String,
    library_sha256: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceIdentity {
    path: String,
    sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Service {
    upstream_name: String,
    runtime: String,
    package: String,
    cargo_features: Vec<String>,
    forbidden_cargo_features: Vec<String>,
    transport: String,
    bind: String,
    host_publish: String,
    container_port: u16,
    host_port: u16,
    readiness: Vec<String>,
    upstream_source_path: String,
    upstream_source_sha256: String,
    upstream_bind_source_path: String,
    upstream_bind_source_sha256: String,
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
struct Flow {
    name: String,
    source: String,
    destination: String,
    direction: String,
    protocol: String,
    semantics: String,
    upstream_source_path: String,
    upstream_source_sha256: String,
}

#[test]
fn immutable_contract_selects_exact_non_standalone_v02_stack() {
    let contract = load_contract();

    assert_eq!(contract.schema_version, 1);
    assert_eq!(
        contract.contract_status,
        "binaries_built_distroless_cli_smoked_stack_not_executed"
    );
    assert_eq!(contract.stack_kind, "bedrock_settled_non_standalone");
    assert_eq!(
        contract.public_transition,
        "configuration_plus_escrow_deployment"
    );

    assert_eq!(
        contract.lez.repository,
        "https://github.com/logos-blockchain/logos-execution-zone.git"
    );
    assert_eq!(contract.lez.tag, LEZ_TAG);
    assert_eq!(contract.lez.commit, LEZ_COMMIT);
    assert_eq!(
        contract.lez.cargo_source,
        format!(
            "git+https://github.com/logos-blockchain/logos-execution-zone.git?tag={LEZ_TAG}#{LEZ_COMMIT}"
        )
    );
    assert_eq!(
        contract.lez.source_verification,
        "clean_git_commit_plus_file_sha256"
    );
    assert_eq!(
        contract.lez.local_tag_policy,
        "verify_commit_when_present_report_absence"
    );
    assert_eq!(contract.lez.rust_toolchain_path, "rust-toolchain.toml");
    assert_eq!(
        contract.lez.rust_toolchain_sha256,
        "0dab14fcb283227f21263b19b22f4895e37b79ab95a4c700f4b2ac94b2ea1fce"
    );
    assert_eq!(contract.lez.rust_toolchain_channel, "1.94.0");

    let sequencer = service(&contract, "sequencer");
    assert_eq!(sequencer.upstream_name, "sequencer_service");
    assert_eq!(sequencer.runtime, "locked_source_packaged_container");
    assert_eq!(sequencer.package, "sequencer_service");
    assert!(
        sequencer.cargo_features.is_empty(),
        "the non-standalone sequencer uses the default real ZoneSdkPublisher"
    );
    assert_eq!(sequencer.forbidden_cargo_features, ["standalone"]);
    assert_eq!(sequencer.container_port, 3040);
    assert_eq!(sequencer.host_port, 0);
    assert_eq!(sequencer.bind, "0.0.0.0:3040_inside_private_run_network");
    assert_eq!(sequencer.host_publish, "127.0.0.1:0:3040");
    assert_eq!(
        sequencer.readiness,
        [
            "rpc:checkHealth",
            "rpc:getChannelId_matches_configured_bedrock_channel",
            "rpc:getProgramIds_contains_required_builtins",
            "rpc:getBlock(1)_returns_genesis",
            "rpc:getLastBlockId_at_or_after_genesis",
            "rpc:getLastBlockId_advances_after_probe_transaction",
        ]
    );

    let indexer = service(&contract, "indexer");
    assert_eq!(indexer.upstream_name, "indexer_service");
    assert_eq!(indexer.runtime, "locked_source_packaged_container");
    assert_eq!(indexer.package, "indexer_service");
    assert_eq!(indexer.forbidden_cargo_features, ["mock-responses"]);
    assert_eq!(indexer.container_port, 8779);
    assert_eq!(indexer.host_port, 0);
    assert_eq!(indexer.bind, "0.0.0.0:8779_inside_private_run_network");
    assert_eq!(indexer.host_publish, "127.0.0.1:0:8779");
    assert_eq!(
        indexer.readiness,
        [
            "rpc:getLastFinalizedBlockId_returns_some",
            "rpc:getBlockById(last_finalized)_returns_same_block_id",
            "cross_check:indexer_finalized_block_matches_sequencer_block_at_same_id",
        ]
    );

    let bedrock = service(&contract, "bedrock");
    assert_eq!(bedrock.upstream_name, "logos-blockchain-node-0");
    assert_eq!(bedrock.runtime, "digest_pinned_container");
    assert_eq!(bedrock.container_port, 18080);
    assert_eq!(bedrock.host_port, 0);
    assert_eq!(bedrock.bind, "0.0.0.0:18080_inside_private_run_network");
    assert_eq!(bedrock.host_publish, "127.0.0.1:0:18080");
    assert_eq!(
        bedrock.readiness,
        [
            "http:GET_/cryptarchia/info_tip_and_lib_present",
            "http:GET_/cryptarchia/info_tip_or_slot_advances",
            "http:GET_/channel/0101010101010101010101010101010101010101010101010101010101010101_returns_200",
        ]
    );

    assert!(
        !indexer
            .readiness
            .iter()
            .any(|probe| probe == "rpc:checkHealth"),
        "indexer checkHealth only exercises local DB recalculation"
    );

    for service in contract.services.values() {
        assert_eq!(
            service.transport,
            "private_run_network_plus_dynamic_host_loopback"
        );
        assert!(is_lower_hex(&service.upstream_source_sha256, 64));
        assert!(is_lower_hex(&service.upstream_bind_source_sha256, 64));
        assert!(
            Path::new(&service.upstream_source_path).is_relative(),
            "upstream source paths must be revision-relative"
        );
        assert!(Path::new(&service.upstream_bind_source_path).is_relative());
    }
}

#[test]
fn service_packaging_uses_locked_sources_and_a_digest_pinned_runtime() {
    let contract = load_contract();
    let packaging = contract.packaging;

    assert_eq!(packaging.services, ["sequencer", "indexer"]);
    assert!(packaging.cargo_locked);
    assert_eq!(packaging.cargo_build_jobs, 2);
    assert_eq!(packaging.cargo_profile, "release");
    assert_eq!(packaging.cargo_toolchain_channel, "1.94.0");
    assert_eq!(
        packaging.cargo_build_command,
        "cargo +1.94.0 build --locked --release --jobs 2 --package sequencer_service --package indexer_service"
    );
    assert_eq!(
        packaging.rapidsnark.dependency_repository,
        "https://github.com/logos-blockchain/logos-blockchain-rust-rapidsnark.git"
    );
    assert_eq!(
        packaging.rapidsnark.dependency_revision,
        "e91187f8ccb5bbfc7bb00dac88169112428da78f"
    );
    assert_eq!(packaging.rapidsnark.crate_version, "0.1.3");
    assert_eq!(packaging.rapidsnark.version, "0.0.8");
    assert_eq!(
        packaging.rapidsnark.archive_name,
        "rapidsnark-linux-x86_64-pic-v0.0.8.zip"
    );
    assert_eq!(
        packaging.rapidsnark.archive_url,
        "https://github.com/logos-blockchain/logos-blockchain-rust-rapidsnark/releases/download/rapidsnark-pic-v0.0.8/rapidsnark-linux-x86_64-pic-v0.0.8.zip"
    );
    assert_eq!(
        packaging.rapidsnark.archive_sha256,
        "59bdd709eed96235de061f352893f4650c923b54b591052118593012bb1cd831"
    );
    assert_eq!(
        packaging.rapidsnark.library_directory_environment,
        "RAPIDSNARK_LIB_DIR"
    );
    assert_eq!(
        packaging.rapidsnark.bindgen_extra_clang_args,
        "-I/usr/lib/gcc/x86_64-linux-gnu/13/include"
    );
    assert_eq!(packaging.rapidsnark.library_sha256.len(), 4);
    for digest in packaging.rapidsnark.library_sha256.values() {
        assert!(is_lower_hex(digest, 64));
    }
    assert_eq!(
        packaging.rapidsnark.library_sha256["librapidsnark.a"],
        "d4133227f845ff5bfa3672eb5b9c018a6a086bfa164b176bdaf76949c7d1f423"
    );
    assert_eq!(
        packaging.rapidsnark.library_sha256["libgmp.a"],
        "0a910b420c3ad603c83c9dc2818c7ae05394c231ca23135c7b873e8e680ea41b"
    );
    assert_eq!(
        packaging.rapidsnark.library_sha256["libfq.a"],
        "797b5d24bb8e8b088f811bddfff35f33973af9c797fb3812489cd42ba6a957d0"
    );
    assert_eq!(
        packaging.rapidsnark.library_sha256["libfr.a"],
        "40f809394904682cb5517845cd3c2f936a5eb4609712534b573f552f2811fb82"
    );
    assert_eq!(
        packaging.runtime_base_reference,
        "gcr.io/distroless/cc-debian13:nonroot@sha256:aded2458d026e046cb68199db0e5793e1028ffa143f7258f3c4278253e20add7"
    );
    assert_eq!(packaging.r0vm_version, "3.0.5");
    assert_eq!(
        packaging.r0vm_sha256,
        "36c016a5bb2ded5bd1f8f92cc487e6ffaeb1e95ec05850c983081a0f716b515b"
    );
    assert_eq!(
        packaging.binary_sha256_status,
        "locked_source_build_hashes_bound_warm_rerun_stable_not_container_executed"
    );
    assert_eq!(
        packaging.sequencer_binary_sha256,
        "3727e9aa10600d04d0cdfda6eb39df146ef4cc14f5b09ad33bcf076a8f2c412f"
    );
    assert_eq!(
        packaging.indexer_binary_sha256,
        "6ed54f04ae018f3554898a9f0aef6decd6930c4e8609326d146ca164e48d7442"
    );
    assert_eq!(
        packaging.sequencer_binary_version,
        "sequencer_service 0.1.0"
    );
    assert_eq!(packaging.indexer_binary_version, "indexer_service 0.1.0");
    assert_eq!(
        packaging.binary_runtime_dependency_status,
        "ldd_standard_cc_runtime_only"
    );
    assert_eq!(
        packaging.runtime_smoke_status,
        "distroless_cli_smoked_not_service_started"
    );
    assert_eq!(
        packaging.runtime_smoke_security,
        [
            "uid_65532",
            "network_none",
            "read_only_root",
            "cap_drop_all",
            "no_new_privileges",
        ]
    );
    assert_eq!(
        packaging.upstream_dockerfiles_status,
        "source_observation_only_not_trusted_reproducible_builds"
    );
    assert_eq!(packaging.upstream_dockerfiles.len(), 2);
    for identity in packaging.upstream_dockerfiles.values() {
        assert!(Path::new(&identity.path).is_relative());
        assert!(is_lower_hex(&identity.sha256, 64));
    }
    let sequencer = packaging
        .upstream_dockerfiles
        .get("sequencer")
        .expect("sequencer Dockerfile observation");
    assert_eq!(sequencer.path, "lez/sequencer/service/Dockerfile");
    assert_eq!(
        sequencer.sha256,
        "4d83e7302ec7fe887336a6f1090c57bc1e425c366d10dd7db45dcfe1e95010bf"
    );
    let indexer = packaging
        .upstream_dockerfiles
        .get("indexer")
        .expect("indexer Dockerfile observation");
    assert_eq!(indexer.path, "lez/indexer/service/Dockerfile");
    assert_eq!(
        indexer.sha256,
        "7690d9f1a726462d6c5dbd86f7ff452f818d12c6399767106a330c6a2b75f0c7"
    );
}

#[test]
fn bedrock_digest_and_oci_source_mapping_are_pinned_while_public_parity_stays_explicit() {
    let contract = load_contract();
    let bedrock = contract.bedrock;

    assert_eq!(
        bedrock.repository,
        "https://github.com/logos-blockchain/logos-blockchain"
    );
    assert_eq!(
        bedrock.image_repository,
        "ghcr.io/logos-blockchain/logos-blockchain"
    );
    assert_eq!(bedrock.image_digest, BEDROCK_IMAGE_DIGEST);
    assert_eq!(
        bedrock.image_reference,
        format!("ghcr.io/logos-blockchain/logos-blockchain@{BEDROCK_IMAGE_DIGEST}")
    );
    assert_eq!(
        bedrock.source_commit,
        "d8711bbc3d43d3ef9755ef9b73af32fd0f703160"
    );
    assert_eq!(
        bedrock.source_mapping_status,
        "oci_label_verified_matches_lez_locked_revision"
    );
    assert_eq!(
        bedrock.image_metadata_verification,
        "requires_local_docker_image_inspect"
    );
    assert_eq!(
        bedrock.image_source_label,
        "https://github.com/logos-blockchain/logos-blockchain"
    );
    assert_eq!(
        bedrock.image_revision_label,
        "d8711bbc3d43d3ef9755ef9b73af32fd0f703160"
    );
    assert_eq!(bedrock.image_version_label, "master");
    assert_eq!(bedrock.image_licenses_label, "Apache-2.0");
    assert_eq!(
        bedrock.public_runtime_parity_status,
        "unverified_upstream_tagged_readme_calls_bundled_node_outdated"
    );
    assert_eq!(bedrock.upstream_compose_path, "bedrock/docker-compose.yml");
    assert!(is_lower_hex(&bedrock.upstream_compose_sha256, 64));
    assert_eq!(
        bedrock.runtime_fixture_policy,
        "exact_mounts_direct_bedrock_binary_invocation"
    );
    assert_eq!(
        bedrock.runner_script_status,
        "source_observation_only_runner_bypasses_timestamp_substitution"
    );
    assert_eq!(bedrock.runtime_fixtures.len(), 3);
    assert_source_identity(
        &bedrock.runtime_fixtures["deployment_settings"],
        "bedrock/deployment-settings.yaml",
        "a253c641c515957d1480a87f4b098a5cd9cbf9ac34bbc097dbe480d8869adbd3",
    );
    assert_source_identity(
        &bedrock.runtime_fixtures["kzgrs_test_params"],
        "bedrock/kzgrs_test_params",
        "45cce45664e7ac60c54d298ffd8f7d96ba6cad741cfebe5ea4e4d2820a667d53",
    );
    assert_source_identity(
        &bedrock.runtime_fixtures["upstream_runner_script"],
        "bedrock/scripts/run_logos_blockchain_node.sh",
        "261585229656475f908c64984a59f6105b0dffd961b69104c2b22eedc050a061",
    );
}

#[test]
fn local_stack_names_ports_and_cleanup_are_run_isolated() {
    let contract = load_contract();
    let generated = &contract.generated_configuration;
    assert_eq!(
        generated.mode,
        "runner_generated_minimal_not_upstream_example_copy"
    );
    assert_eq!(generated.bedrock_node_url, "http://bedrock:18080");
    assert_eq!(
        generated.channel_id,
        "0101010101010101010101010101010101010101010101010101010101010101"
    );
    assert_eq!(
        generated.channel_id_source,
        "contracted_lez_zone_channel_01_repeated_32"
    );
    assert_eq!(
        generated.sequencer_state_dir,
        ".e2e/{run_id}/lez-v02/sequencer"
    );
    assert_eq!(generated.indexer_state_dir, ".e2e/{run_id}/lez-v02/indexer");
    assert_eq!(
        generated.backoff_field_policy,
        "omitted_unsupported_upstream_example_field"
    );
    assert_eq!(generated.fresh_state_policy, "refuse_preexisting_run_state");
    assert_eq!(generated.filesystem_policy, "private_run_scoped_paths");
    assert_eq!(
        generated.source_examples_status,
        "hashed_observations_only_not_copied_backoff_or_addresses"
    );
    assert_eq!(generated.source_examples.len(), 4);
    assert_source_identity(
        &generated.source_examples["all_in_one_sequencer"],
        "lez/configs/docker-all-in-one/sequencer_config.json",
        "542c06f58ebecfec443d5e8e201d6e5009b613c3d7bc7a17e6de04ca52c58331",
    );
    assert_source_identity(
        &generated.source_examples["all_in_one_indexer"],
        "lez/configs/docker-all-in-one/indexer_config.json",
        "4f4ca8a1bd94aaf6bd65d6c7315d99ac378e664822425cca6f9289647c7e9e40",
    );
    assert_source_identity(
        &generated.source_examples["service_sequencer"],
        "lez/sequencer/service/configs/docker/sequencer_config.json",
        "101e136a2ef5071fdacfbc973184dcbc43b8e09690af0a30e5592c0fd1d2887a",
    );
    assert_source_identity(
        &generated.source_examples["service_indexer"],
        "lez/indexer/service/configs/docker/indexer_config.json",
        "428320c0e9db6490e0949c354fdeada8b55e47eb3a037d05677714601f3e620d",
    );

    let isolation = contract.isolation;

    assert_eq!(isolation.run_id_pattern, "^[a-z0-9][a-z0-9_-]{0,63}$");
    assert_eq!(
        isolation.compose_project_template,
        "lez-atomic-swaps-lez-v02-{run_id}"
    );
    assert_eq!(isolation.state_root_template, ".e2e/{run_id}/lez-v02");
    assert_eq!(isolation.host_port_policy, "dynamic_zero_only");
    assert_eq!(isolation.container_name_policy, "forbidden");
    assert_eq!(isolation.cleanup_scope, "matching_run_id_only");

    let upstream_names = contract
        .services
        .values()
        .map(|service| service.upstream_name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(upstream_names.len(), 3);
    assert!(
        upstream_names.iter().all(|name| !name.contains("{run_id}")),
        "upstream names describe source components, not reusable container names"
    );
    assert!(
        contract
            .services
            .values()
            .all(|service| service.host_port == 0)
    );
}

#[test]
fn event_directions_cover_submission_publication_and_finality() {
    let contract = load_contract();

    assert_eq!(
        contract.flows,
        [
            Flow {
                name: "actor_submission".to_owned(),
                source: "maker_or_taker_sidecar".to_owned(),
                destination: "sequencer".to_owned(),
                direction: "request_response".to_owned(),
                protocol: "official_lez_json_rpc".to_owned(),
                semantics: "submit_and_query_LeeTransaction".to_owned(),
                upstream_source_path: "lez/sequencer/service/rpc/src/lib.rs".to_owned(),
                upstream_source_sha256:
                    "db50ad06b17a86086d84c8a7642def3af21beda4ffed4660cbc219255c06ffb4".to_owned(),
            },
            Flow {
                name: "block_publication".to_owned(),
                source: "sequencer".to_owned(),
                destination: "bedrock".to_owned(),
                direction: "outbound".to_owned(),
                protocol: "logos_zone_sdk_http".to_owned(),
                semantics: "publish_lez_block_inscription_and_withdrawals".to_owned(),
                upstream_source_path: "lez/sequencer/core/src/block_publisher.rs".to_owned(),
                upstream_source_sha256:
                    "57d4cf0e94755d19daa8af13f6c5b27cb349fe2c176436f79af5b871cb6971fb".to_owned(),
            },
            Flow {
                name: "finality_ingestion".to_owned(),
                source: "indexer".to_owned(),
                destination: "bedrock".to_owned(),
                direction: "request_response".to_owned(),
                protocol: "logos_zone_sdk_http".to_owned(),
                semantics:
                    "poll_finalized_zone_messages_from_bedrock_into_indexer_skip_deposit_withdraw"
                        .to_owned(),
                upstream_source_path: "lez/indexer/core/src/lib.rs".to_owned(),
                upstream_source_sha256:
                    "cdb8238dc9bbb0f83b616b63d407f08505fa5a5dfa20343486fee31dcab1e7b4".to_owned(),
            },
            Flow {
                name: "certification_observation".to_owned(),
                source: "indexer".to_owned(),
                destination: "local_stack_orchestrator".to_owned(),
                direction: "request_response".to_owned(),
                protocol: "official_indexer_json_rpc".to_owned(),
                semantics: "retrieve_finalized_block_and_match_sequencer_block_at_same_id"
                    .to_owned(),
                upstream_source_path: "lez/indexer/service/rpc/src/lib.rs".to_owned(),
                upstream_source_sha256:
                    "a7a2a3114e0b65a9f33decff79bb5ffe5c75c54507f5054560c4e1fef1e0af3e".to_owned(),
            },
        ]
    );
    for flow in contract.flows {
        assert!(Path::new(&flow.upstream_source_path).is_relative());
        assert!(is_lower_hex(&flow.upstream_source_sha256, 64));
    }
}

fn load_contract() -> LocalStackContract {
    let source = fs::read_to_string(CONTRACT_PATH)
        .expect("tracked LEZ v0.2 local-stack contract must exist");
    toml::from_str(&source).expect("local-stack contract must be strict valid TOML")
}

fn service<'a>(contract: &'a LocalStackContract, name: &str) -> &'a Service {
    contract
        .services
        .get(name)
        .unwrap_or_else(|| panic!("missing {name} service contract"))
}

fn is_lower_hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn assert_source_identity(identity: &SourceIdentity, path: &str, sha256: &str) {
    assert_eq!(identity.path, path);
    assert_eq!(identity.sha256, sha256);
}
