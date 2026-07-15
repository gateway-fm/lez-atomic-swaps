use std::{fs, os::unix::fs::PermissionsExt as _, path::Path, process::Command as ProcessCommand};

use btc_reference_actor::{
    ActorCli, ActorCommand, ActorCommandError, ActorConfig, ActorConfigError, execute_actor_command,
};
use clap::Parser as _;
use lez_bridge_protocol::{
    Hex32, Participant as BridgeParticipant, RunId, RuntimeCompatibility, RuntimeDescriptor,
};
use serde_json::{Value, json};
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
        let config_path = directory.path().join("actor-private.json");
        fs::write(&agreement_path, &agreement_wire).expect("write agreement");

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
        write_private_json(
            &config_path,
            &json!({
                "schema_version": 1,
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
                    "run_id": RunId::new("m3-actor-test-run").expect("run id"),
                    "runtime": runtime,
                    "request_timeout_millis": 1_000,
                    "discovery_start_height": 1,
                    "discovery_max_blocks": 10
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

fn write_private_json(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec(value).expect("config JSON")).expect("write config");
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).expect("private config mode");
}

fn output_json(output: impl serde::Serialize) -> Value {
    serde_json::to_value(output).expect("secret-free actor output")
}

#[test]
fn cli_exposes_repeatable_activate_drive_and_status_commands() {
    for (command, expected) in [
        ("activate", ActorCommand::Activate),
        ("drive", ActorCommand::Drive),
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
        serde_json::from_slice::<Value>(&output.stdout).expect("one JSON actor response")
    };

    assert_eq!(invoke("status")["state"], "not_activated");
    assert_eq!(invoke("activate")["was_replay"], false);
    assert_eq!(invoke("activate")["was_replay"], true);
    let status = invoke("status");
    assert_eq!(status["state"], "active");
    assert_eq!(status["revision"], 0);
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
