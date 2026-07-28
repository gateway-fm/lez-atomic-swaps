use std::{
    ffi::CString,
    fs,
    fs::File,
    io::Write as _,
    os::fd::OwnedFd,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
};

use btc_reference_actor::{
    ActorCli, ActorCommand, ActorCommandError, ActorConfig, ActorConfigError, ActorRole,
    execute_actor_command,
};
use clap::Parser as _;
use command_fds::{CommandFdExt as _, FdMapping};
use lez_bridge_protocol::{
    ExactMessageBytes, Hex32, MessageContext, Participant as BridgeParticipant,
    PrepareWitnessedClaimResult, PreparedWitnessedClaim, RequestId, RunId, RuntimeCompatibility,
    RuntimeDescriptor,
};
use lez_btc_swap_sdk::{
    BtcAdaptorSessionDomain, BtcAgreementV1, FreshAdaptorNonce, PersistedAdaptorSigningMaterial,
    SigningRole, aggregate_adaptor_presignature, sign_persisted_adaptor_partial,
    verify_adaptor_partial_signature, verify_nonce_commitment,
};
use lez_swap_store::{
    AdaptorNonceCommitment, AdaptorPartialSignature, AdaptorPresignature, AdaptorPublicNonce,
    AdaptorSessionIdentity, AdaptorSessionReservation, AdaptorSessionRole, MAKER_ACTOR_CONFIG_FD,
    SecretNonceBytes, SqliteAdaptorSessionJournal,
};
use rustix::fs::{MemfdFlags, Mode, SealFlags, fchmod, fcntl_add_seals, memfd_create};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;

#[allow(dead_code)]
#[path = "../../btc-core-adapter/tests/support.rs"]
mod support;

struct ActorFixture {
    _directory: TempDir,
    config_path: std::path::PathBuf,
    config: ActorConfig,
}

impl ActorFixture {
    fn new(role: BridgeParticipant, runtime_role: BridgeParticipant) -> Self {
        Self::try_new(role, runtime_role).expect("private actor config")
    }

    fn try_new(
        role: BridgeParticipant,
        runtime_role: BridgeParticipant,
    ) -> Result<Self, ActorConfigError> {
        let directory = tempfile::tempdir().expect("actor tempdir");
        let swap = support::swap_fixture();
        let agreement_wire = swap.agreement.encode_wire().expect("agreement wire");
        let agreement_path = directory.path().join("agreement.json");
        let state_path = directory.path().join("actor.sqlite3");
        let cookie_path = directory.path().join("bitcoin.cookie");
        let capability_path = directory.path().join("lez.capability");
        let bitcoin_journal_path = directory.path().join("bitcoin-adaptor.sqlite3");
        let lez_journal_path = directory.path().join("lez-adaptor.sqlite3");
        let prepared_claim_path = directory.path().join("prepared-witnessed-claim.json");
        let adaptor_secret_path = directory.path().join("adaptor-secret.key");
        let bitcoin_refund_path = directory.path().join("bitcoin-refund.key");
        let config_path = directory.path().join("actor-private.json");
        fs::write(&agreement_path, &agreement_wire).expect("write agreement");
        let run_id = RunId::new("m3-actor-test-run").expect("run id");
        seed_activation_material(
            &swap.agreement,
            role,
            &run_id,
            &bitcoin_journal_path,
            &lez_journal_path,
            &prepared_claim_path,
        );

        let runtime = RuntimeDescriptor::new(
            runtime_role,
            RuntimeCompatibility::LeeV0_2_0,
            Hex32::from_bytes([99; 32]),
            Hex32::from_bytes([17; 32]),
            Hex32::from_bytes([18; 32]),
            Hex32::from_bytes([15; 32]),
            Hex32::from_bytes(match runtime_role {
                BridgeParticipant::Maker => [10; 32],
                BridgeParticipant::Taker => [11; 32],
            }),
        );
        let mut signing = json!({
            "bitcoin": {
                "session_id": hex::encode([41; 32]),
                "journal_db": bitcoin_journal_path
            },
            "lez": {
                "session_id": hex::encode([42; 32]),
                "journal_db": lez_journal_path
            },
            "prepared_witnessed_claim_result_file": prepared_claim_path
        });
        if role == BridgeParticipant::Taker {
            write_private_scalar(&adaptor_secret_path, support::ADAPTOR_SECRET);
            write_private_scalar(&bitcoin_refund_path, support::REFUND_SECRET);
            signing["adaptor_secret_file"] =
                Value::from(adaptor_secret_path.to_string_lossy().into_owned());
        }
        write_private_json(
            &config_path,
            &json!({
                "schema_version": 3,
                "role": match role {
                    BridgeParticipant::Maker => "maker",
                    BridgeParticipant::Taker => "taker",
                },
                "agreement_file": agreement_path,
                "state_db": state_path,
                "accepted_at_unix_seconds": 1_700_000_000,
                "bitcoin_core": {
                    "endpoint": "http://127.0.0.1:1",
                    "cookie_file": cookie_path,
                    "connectivity": "isolated_local"
                },
                "lez_bridge": {
                    "endpoint": "http://127.0.0.1:2",
                    "capability_file": capability_path,
                    "run_id": run_id,
                    "runtime": runtime,
                    "request_timeout_millis": 1_000,
                    "discovery_start_height": 1,
                    "discovery_max_blocks": 10
                },
                "signing": signing,
                "refund": if role == BridgeParticipant::Taker {
                    json!({ "bitcoin_refund_key_file": bitcoin_refund_path })
                } else {
                    json!({})
                }
            }),
        );
        let config = ActorConfig::load_private(&config_path)?;
        Ok(Self {
            _directory: directory,
            config_path,
            config,
        })
    }
}

fn seed_activation_material(
    agreement: &BtcAgreementV1,
    role: BridgeParticipant,
    run_id: &RunId,
    bitcoin_journal: &Path,
    lez_journal: &Path,
    prepared_claim: &Path,
) {
    let request_id = RequestId::new("prepared-claim-001").expect("request ID");
    let claimant = match agreement.lez_claimant() {
        lez_swap_core::Participant::Maker => BridgeParticipant::Maker,
        lez_swap_core::Participant::Taker => BridgeParticipant::Taker,
    };
    let result = PrepareWitnessedClaimResult::new(
        MessageContext::new(run_id.clone(), request_id.clone(), claimant),
        PreparedWitnessedClaim::new(
            request_id,
            Hex32::from_bytes(support::lez_claim_message_hash()),
            ExactMessageBytes::new(support::LEZ_PREPARED_MESSAGE_BYTES.to_vec())
                .expect("prepared message"),
        ),
    );
    fs::write(
        prepared_claim,
        serde_json::to_vec(&result).expect("prepared result JSON"),
    )
    .expect("write prepared result");
    seed_signing_journal(
        agreement,
        role,
        BtcAdaptorSessionDomain::Bitcoin,
        [41; 32],
        bitcoin_journal,
    );
    seed_signing_journal(
        agreement,
        role,
        BtcAdaptorSessionDomain::Lez,
        [42; 32],
        lez_journal,
    );
}

#[allow(clippy::too_many_lines)] // The fixture spells out every durable ceremony transition.
fn seed_signing_journal(
    agreement: &BtcAgreementV1,
    role: BridgeParticipant,
    domain: BtcAdaptorSessionDomain,
    session_id: [u8; 32],
    journal_path: &Path,
) {
    let context = agreement
        .adaptor_session_context(domain, session_id)
        .expect("agreement signing context");
    let maker_nonce =
        FreshAdaptorNonce::generate(&context, SigningRole::Maker, support::MAKER_SECRET)
            .expect("maker nonce");
    let taker_nonce =
        FreshAdaptorNonce::generate(&context, SigningRole::Taker, support::TAKER_SECRET)
            .expect("taker nonce");
    let (local_role, local_store_role, local_secret, local_nonce) = match role {
        BridgeParticipant::Maker => (
            SigningRole::Maker,
            AdaptorSessionRole::Maker,
            support::MAKER_SECRET,
            &maker_nonce,
        ),
        BridgeParticipant::Taker => (
            SigningRole::Taker,
            AdaptorSessionRole::Taker,
            support::TAKER_SECRET,
            &taker_nonce,
        ),
    };
    let (peer_role, peer_secret, peer_nonce) = match role {
        BridgeParticipant::Maker => (SigningRole::Taker, support::TAKER_SECRET, &taker_nonce),
        BridgeParticipant::Taker => (SigningRole::Maker, support::MAKER_SECRET, &maker_nonce),
    };
    let identity = AdaptorSessionIdentity::new(
        session_id,
        local_store_role,
        context.durable_context_binding(),
        context.message(),
        context.adaptor_point(),
        context.ordered_public_keys(),
    );
    let mut journal =
        SqliteAdaptorSessionJournal::open(journal_path).expect("create signing journal");
    let _ = journal
        .reserve(AdaptorSessionReservation::new(
            identity.clone(),
            SecretNonceBytes::new(*local_nonce.secret_nonce()),
            AdaptorPublicNonce::new(local_nonce.public_nonce()),
            AdaptorNonceCommitment::new(local_nonce.commitment()),
        ))
        .expect("reserve nonce");
    let _ = journal
        .record_peer_commitment(
            &identity,
            AdaptorNonceCommitment::new(peer_nonce.commitment()),
        )
        .expect("peer commitment");
    verify_nonce_commitment(
        &context,
        peer_role,
        peer_nonce.commitment(),
        peer_nonce.public_nonce(),
    )
    .expect("verify peer nonce");
    let _ = journal
        .record_verified_peer_public_nonce(
            &identity,
            AdaptorPublicNonce::new(peer_nonce.public_nonce()),
        )
        .expect("peer nonce");
    let own_partial = journal
        .sign_and_persist_partial(&identity, |material| {
            sign_persisted_adaptor_partial(
                &context,
                local_role,
                local_secret,
                PersistedAdaptorSigningMaterial::new(
                    *material.identity().signing_domain(),
                    material.secret_nonce(),
                    *material.own_public_nonce().bytes(),
                    local_nonce.commitment(),
                    peer_nonce.commitment(),
                    *material.peer_public_nonce().bytes(),
                ),
            )
            .map(AdaptorPartialSignature::new)
            .map_err(|_| ())
        })
        .expect("local partial")
        .partial();
    let peer_partial = sign_persisted_adaptor_partial(
        &context,
        peer_role,
        peer_secret,
        PersistedAdaptorSigningMaterial::new(
            context.durable_context_binding(),
            peer_nonce.secret_nonce(),
            peer_nonce.public_nonce(),
            peer_nonce.commitment(),
            local_nonce.commitment(),
            local_nonce.public_nonce(),
        ),
    )
    .expect("peer partial");
    let (maker_nonce_bytes, taker_nonce_bytes, maker_partial, taker_partial) = match role {
        BridgeParticipant::Maker => (
            local_nonce.public_nonce(),
            peer_nonce.public_nonce(),
            *own_partial.bytes(),
            peer_partial,
        ),
        BridgeParticipant::Taker => (
            peer_nonce.public_nonce(),
            local_nonce.public_nonce(),
            peer_partial,
            *own_partial.bytes(),
        ),
    };
    verify_adaptor_partial_signature(
        &context,
        peer_role,
        maker_nonce_bytes,
        taker_nonce_bytes,
        peer_partial,
    )
    .expect("verify peer partial");
    let _ = journal
        .record_verified_peer_partial(&identity, AdaptorPartialSignature::new(peer_partial))
        .expect("record peer partial");
    let presignature = aggregate_adaptor_presignature(
        &context,
        maker_nonce_bytes,
        taker_nonce_bytes,
        maker_partial,
        taker_partial,
    )
    .expect("aggregate presignature");
    let _ = journal
        .record_verified_presignature(&identity, AdaptorPresignature::new(presignature))
        .expect("record presignature");
}

fn write_private_json(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec(value).expect("config JSON")).expect("write config");
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).expect("private config mode");
}

fn write_private_scalar(path: &Path, scalar: [u8; 32]) {
    fs::write(path, format!("{}\n", hex::encode(scalar))).expect("write private scalar");
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).expect("private scalar mode");
}

fn output_json(output: impl serde::Serialize) -> Value {
    serde_json::to_value(output).expect("secret-free actor output")
}

#[test]
fn cli_exposes_repeatable_activate_drive_and_status_commands() {
    for (command, expected) in [
        ("activate", ActorCommand::Activate),
        ("drive", ActorCommand::Drive),
        ("recover", ActorCommand::Recover),
        ("status", ActorCommand::Status),
    ] {
        let cli = ActorCli::try_parse_from([
            "btc-reference-actor",
            "--config",
            "/tmp/private-actor.json",
            command,
        ])
        .expect("parse actor command");
        assert_eq!(cli.command, expected);
    }
    assert!(
        ActorCli::try_parse_from(["btc-reference-actor", "--config-fd", "196", "status"]).is_ok()
    );
    assert!(
        ActorCli::try_parse_from([
            "btc-reference-actor",
            "--config",
            "/tmp/private-actor.json",
            "--config-fd",
            "196",
            "status"
        ])
        .is_err()
    );
    assert!(
        ActorCli::try_parse_from(["btc-reference-actor", "--config-fd", "195", "status"]).is_err()
    );
}

#[test]
fn binary_repeats_offline_status_and_idempotent_activation_from_disk() {
    let fixture = ActorFixture::new(BridgeParticipant::Taker, BridgeParticipant::Taker);
    let invoke = |command: &str| {
        let output = ProcessCommand::new(env!("CARGO_BIN_EXE_btc-reference-actor"))
            .args([
                "--config",
                fixture.config_path.to_str().expect("UTF-8 test path"),
                command,
            ])
            .output()
            .expect("invoke actor binary");
        assert!(
            output.status.success(),
            "actor command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        let secret_hex = hex::encode(support::ADAPTOR_SECRET);
        assert!(
            !output
                .stdout
                .windows(secret_hex.len())
                .any(|window| window == secret_hex.as_bytes())
        );
        serde_json::from_slice::<Value>(&output.stdout).expect("one JSON actor response")
    };

    assert_eq!(invoke("status")["state"], "not_activated");
    assert_eq!(invoke("activate")["was_replay"], false);
    assert_eq!(invoke("activate")["was_replay"], true);
    let status = invoke("status");
    assert_eq!(status["state"], "active");
    assert_eq!(status["revision"], 0);

    let config: Value =
        serde_json::from_slice(&fs::read(&fixture.config_path).expect("config bytes"))
            .expect("config JSON");
    let state = config["state_db"].as_str().expect("state path");
    let secret_hex = hex::encode(support::ADAPTOR_SECRET);
    for suffix in ["", "-wal", "-shm"] {
        let path = PathBuf::from(format!("{state}{suffix}"));
        if path.exists() {
            let bytes = fs::read(path).expect("read actor state artifact");
            assert!(
                !bytes
                    .windows(support::ADAPTOR_SECRET.len())
                    .any(|window| { window == support::ADAPTOR_SECRET })
            );
            assert!(
                !bytes
                    .windows(secret_hex.len())
                    .any(|window| window == secret_hex.as_bytes())
            );
        }
    }
}

#[test]
fn real_binary_reads_only_commitment_bound_fully_sealed_config() {
    let fixture = ActorFixture::new(BridgeParticipant::Taker, BridgeParticipant::Taker);
    let mut config: Value =
        serde_json::from_slice(&fs::read(&fixture.config_path).expect("config bytes"))
            .expect("config JSON");
    let agreement_path = PathBuf::from(config["agreement_file"].as_str().expect("agreement path"));
    config["schema_version"] = Value::from(6);
    config["agreement_sha256"] = Value::from(hex::encode(Sha256::digest(
        fs::read(agreement_path).expect("agreement bytes"),
    )));
    write_private_json(&fixture.config_path, &config);
    let supervised_bytes = fs::read(&fixture.config_path).expect("supervised config bytes");
    let sealed = config_memfd(
        &supervised_bytes,
        SealFlags::SEAL | SealFlags::SHRINK | SealFlags::GROW | SealFlags::WRITE,
    );
    write_private_json(&fixture.config_path, &json!({}));

    let output = run_with_config_fd(sealed, "status");
    assert!(
        output.status.success(),
        "sealed actor failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let status: Value = serde_json::from_slice(&output.stdout).expect("one actor response");
    assert_eq!(status["role"], "taker");
    assert_eq!(status["state"], "not_activated");

    let incomplete = config_memfd(
        &supervised_bytes,
        SealFlags::SEAL | SealFlags::SHRINK | SealFlags::GROW,
    );
    assert_config_fd_rejected(&run_with_config_fd(incomplete, "status"));

    let ordinary = File::open(&fixture.config_path).expect("ordinary config file");
    assert_config_fd_rejected(&run_with_config_fd(ordinary.into(), "status"));

    let mut legacy = config;
    legacy["schema_version"] = Value::from(3);
    legacy.as_object_mut().unwrap().remove("agreement_sha256");
    let legacy_bytes = serde_json::to_vec(&legacy).expect("legacy config JSON");
    let legacy = config_memfd(
        &legacy_bytes,
        SealFlags::SEAL | SealFlags::SHRINK | SealFlags::GROW | SealFlags::WRITE,
    );
    assert_config_fd_rejected(&run_with_config_fd(legacy, "status"));
}

fn run_with_config_fd(config: OwnedFd, command_name: &str) -> std::process::Output {
    let mut command = ProcessCommand::new(env!("CARGO_BIN_EXE_btc-reference-actor"));
    command.args(["--config-fd", "196", command_name]);
    command
        .fd_mappings(vec![FdMapping {
            parent_fd: config,
            child_fd: MAKER_ACTOR_CONFIG_FD,
        }])
        .expect("map config descriptor");
    command.output().expect("run config-FD actor")
}

fn assert_config_fd_rejected(output: &std::process::Output) {
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(output.stderr, b"actor configuration is unavailable\n");
}

fn config_memfd(bytes: &[u8], seals: SealFlags) -> OwnedFd {
    let name = CString::new("lez-btc-actor-test-config").expect("memfd name");
    let descriptor = memfd_create(
        name.as_c_str(),
        MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
    )
    .expect("create config memfd");
    let mut file = File::from(descriptor);
    fchmod(&file, Mode::RUSR | Mode::WUSR).expect("set config memfd mode");
    file.write_all(bytes).expect("write config memfd");
    fcntl_add_seals(&file, seals).expect("seal config memfd");
    file.into()
}

#[test]
fn private_config_rejects_world_readability_and_role_runtime_drift() {
    let directory = tempfile::tempdir().expect("actor tempdir");
    let config_path = directory.path().join("actor-private.json");
    fs::write(&config_path, b"{}").expect("write config");
    fs::set_permissions(&config_path, fs::Permissions::from_mode(0o644))
        .expect("world-readable mode");
    assert!(ActorConfig::load_private(&config_path).is_err());

    assert!(
        ActorFixture::try_new(BridgeParticipant::Maker, BridgeParticipant::Taker).is_err(),
        "private configuration must reject role/runtime drift"
    );
}

#[test]
fn old_config_schema_is_rejected_after_refund_authority_migration() {
    let fixture = ActorFixture::new(BridgeParticipant::Taker, BridgeParticipant::Taker);
    let mut config: Value =
        serde_json::from_slice(&fs::read(&fixture.config_path).expect("config bytes"))
            .expect("config JSON");
    config["schema_version"] = Value::from(2);
    write_private_json(&fixture.config_path, &config);
    assert_eq!(
        ActorConfig::load_private(&fixture.config_path),
        Err(ActorConfigError::Invalid)
    );
}

#[test]
fn supervised_schema_requires_and_enforces_the_exact_agreement_digest() {
    let fixture = ActorFixture::new(BridgeParticipant::Taker, BridgeParticipant::Taker);
    let mut config: Value =
        serde_json::from_slice(&fs::read(&fixture.config_path).expect("config bytes"))
            .expect("config JSON");
    let agreement_path = PathBuf::from(config["agreement_file"].as_str().expect("agreement path"));
    let agreement_bytes = fs::read(agreement_path).expect("agreement bytes");
    config["schema_version"] = Value::from(6);
    config["agreement_sha256"] = Value::from(hex::encode(Sha256::digest(&agreement_bytes)));
    write_private_json(&fixture.config_path, &config);
    let exact = ActorConfig::load_private(&fixture.config_path)
        .expect("commitment-bound supervised config");
    assert!(execute_sync(&exact, ActorCommand::Status).is_ok());
    assert_eq!(exact.role(), ActorRole::Taker);
    assert_eq!(
        exact.state_db(),
        Path::new(config["state_db"].as_str().expect("state path"))
    );
    assert_eq!(
        exact.agreement_sha256(),
        Some(Sha256::digest(&agreement_bytes).into())
    );
    assert_eq!(
        exact.supervised_swap_id().expect("supervised swap ID"),
        support::swap_fixture().agreement.coordinator().id().clone()
    );

    config["agreement_sha256"] = Value::from(hex::encode([0_u8; 32]));
    write_private_json(&fixture.config_path, &config);
    let mismatched = ActorConfig::load_private(&fixture.config_path)
        .expect("well-shaped config with mismatched external commitment");
    assert_eq!(
        execute_sync(&mismatched, ActorCommand::Activate),
        Err(ActorCommandError::AgreementBindingInvalid)
    );

    config.as_object_mut().unwrap().remove("agreement_sha256");
    write_private_json(&fixture.config_path, &config);
    assert_eq!(
        ActorConfig::load_private(&fixture.config_path),
        Err(ActorConfigError::Invalid)
    );
}

#[test]
fn maker_config_rejects_an_explicit_null_adaptor_secret_field() {
    let fixture = ActorFixture::new(BridgeParticipant::Maker, BridgeParticipant::Maker);
    let mut config: Value =
        serde_json::from_slice(&fs::read(&fixture.config_path).expect("config bytes"))
            .expect("config JSON");
    config["signing"]["adaptor_secret_file"] = Value::Null;
    write_private_json(&fixture.config_path, &config);
    assert_eq!(
        ActorConfig::load_private(&fixture.config_path),
        Err(ActorConfigError::Invalid)
    );
}

#[test]
fn maker_and_taker_activate_only_with_their_role_bound_runtime() {
    for (role, expected) in [
        (BridgeParticipant::Maker, "maker"),
        (BridgeParticipant::Taker, "taker"),
    ] {
        let fixture = ActorFixture::new(role, role);
        let activated = output_json(
            execute_sync(&fixture.config, ActorCommand::Activate).expect("role activation"),
        );
        assert_eq!(activated["role"], expected);
        assert_eq!(activated["revision"], 0);
    }
}

#[test]
fn status_is_offline_and_activation_is_idempotent() {
    let fixture = ActorFixture::new(BridgeParticipant::Taker, BridgeParticipant::Taker);

    let before = output_json(
        execute_sync(&fixture.config, ActorCommand::Status).expect("offline pre-activation status"),
    );
    assert_eq!(before["state"], "not_activated");

    let first = output_json(
        execute_sync(&fixture.config, ActorCommand::Activate).expect("first activation"),
    );
    assert_eq!(first["outcome"], "activated");
    assert_eq!(first["was_replay"], false);
    assert_eq!(first["revision"], 0);

    let replay = output_json(
        execute_sync(&fixture.config, ActorCommand::Activate).expect("activation replay"),
    );
    assert_eq!(replay["outcome"], "activated");
    assert_eq!(replay["was_replay"], true);
    assert_eq!(replay["revision"], 0);

    let after = output_json(
        execute_sync(&fixture.config, ActorCommand::Status).expect("offline active status"),
    );
    assert_eq!(after["state"], "active");
    assert_eq!(after["revision"], 0);
    assert_eq!(after["next_action"], "observe_taker_first_lock");
}

fn execute_sync(
    config: &ActorConfig,
    command: ActorCommand,
) -> Result<btc_reference_actor::ActorCommandOutputV1, ActorCommandError> {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("current-thread runtime")
        .block_on(execute_actor_command(config, command))
}
