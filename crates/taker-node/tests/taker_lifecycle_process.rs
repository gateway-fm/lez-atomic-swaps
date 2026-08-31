//! Black-box contract for post-lock Taker lifecycle commands.

use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    process::{Command, Output},
};

use lez_swap_store::MakerActorHeldLock;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;
use zec_reference_actor::ActorConfig;

const AGREEMENT: &[u8] = b"bounded signed agreement fixture";
const CLAIM_KEY: &[u8] = &[0x71; 32];
const PRIVATE_MARKER: &str = "taker-private-capability-marker";

#[test]
fn monitor_runs_offline_for_taker_and_returns_only_actor_status() {
    let fixture = LifecycleFixture::new();

    let output = taker_command("monitor", &fixture.taker_config);

    assert!(
        output.status.success(),
        "offline Taker monitor failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).expect("one JSON status response"),
        json!({
            "schema_version": 1,
            "role": "taker",
            "state": "not_activated"
        })
    );
    assert!(
        !fixture.taker_state.exists(),
        "offline monitor must not create role state"
    );
    assert_secret_free(&output, fixture.root.path());
}

#[test]
fn monitor_runs_offline_from_an_acceptance_receipt() {
    let fixture = LifecycleFixture::new();

    let output = taker_receipt_command("monitor", &fixture.taker_receipt);

    assert!(
        output.status.success(),
        "receipt-bound monitor failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).expect("one JSON status response"),
        json!({
            "schema_version": 1,
            "role": "taker",
            "state": "not_activated"
        })
    );
    assert!(!fixture.taker_state.exists());
    assert_secret_free(&output, fixture.root.path());
}

#[test]
fn receipt_rejects_changed_config_bytes_and_wrong_agreement_identity() {
    let changed_config = LifecycleFixture::new();
    OpenOptions::new()
        .append(true)
        .open(&changed_config.taker_config)
        .unwrap()
        .write_all(b"\n")
        .unwrap();
    let output = taker_receipt_command("monitor", &changed_config.taker_receipt);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_secret_free(&output, changed_config.root.path());

    let wrong_agreement = LifecycleFixture::new();
    let mut receipt: Value =
        serde_json::from_slice(&fs::read(&wrong_agreement.taker_receipt).unwrap()).unwrap();
    receipt["agreement_sha256"] = json!("00".repeat(32));
    fs::write(
        &wrong_agreement.taker_receipt,
        serde_json::to_vec(&receipt).unwrap(),
    )
    .unwrap();
    let output = taker_receipt_command("monitor", &wrong_agreement.taker_receipt);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_secret_free(&output, wrong_agreement.root.path());
}

#[test]
fn receipt_rejects_unknown_fields_and_cli_rejects_ambiguous_sources() {
    let fixture = LifecycleFixture::new();
    let mut receipt: Value =
        serde_json::from_slice(&fs::read(&fixture.taker_receipt).unwrap()).unwrap();
    receipt["unexpected"] = json!(true);
    fs::write(
        &fixture.taker_receipt,
        serde_json::to_vec(&receipt).unwrap(),
    )
    .unwrap();
    let output = taker_receipt_command("monitor", &fixture.taker_receipt);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_secret_free(&output, fixture.root.path());

    let output = Command::new(env!("CARGO_BIN_EXE_lez-taker-cli"))
        .arg("monitor")
        .arg("--actor-config")
        .arg(&fixture.taker_config)
        .arg("--receipt")
        .arg(&fixture.taker_receipt)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_secret_free(&output, fixture.root.path());
}

#[test]
fn monitor_rejects_a_maker_role_config_without_exposing_private_material() {
    let fixture = LifecycleFixture::new();

    let output = taker_command("monitor", &fixture.maker_config);

    assert!(
        !output.status.success(),
        "Taker lifecycle command accepted a Maker-role config"
    );
    assert!(output.stdout.is_empty());
    assert!(
        !fixture.maker_state.exists(),
        "role rejection must happen before state access"
    );
    assert_secret_free(&output, fixture.root.path());
}

#[test]
fn monitor_fails_closed_while_the_same_role_actor_is_running() {
    let fixture = LifecycleFixture::new();
    let config = ActorConfig::load_private(&fixture.taker_config).expect("valid Taker config");
    let held = MakerActorHeldLock::acquire_for(config.swap_id(), config.role_state_db())
        .expect("hold exact Taker actor lock");

    let contended = taker_command("monitor", &fixture.taker_config);
    assert!(!contended.status.success());
    assert!(contended.stdout.is_empty());
    assert_secret_free(&contended, fixture.root.path());

    drop(held);
    let recovered = taker_command("monitor", &fixture.taker_config);
    assert!(
        recovered.status.success(),
        "monitor did not recover after lock release: {}",
        String::from_utf8_lossy(&recovered.stderr)
    );
}

#[test]
fn claim_and_refund_subcommands_expose_the_private_actor_config_boundary() {
    for command in ["claim", "refund"] {
        let output = Command::new(env!("CARGO_BIN_EXE_lez-taker-cli"))
            .args([command, "--help"])
            .output()
            .expect("run real Taker CLI help");
        assert!(
            output.status.success(),
            "{command} help failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).expect("UTF-8 help");
        assert!(
            stdout.contains("--actor-config") && stdout.contains("--receipt"),
            "{command} must require a private actor config"
        );
        assert!(
            !stdout.contains("--delivery-directory")
                && !stdout.contains("--maker-public-key")
                && !stdout.contains("--chat-socket"),
            "post-lock {command} must not depend on discovery or Chat arguments"
        );
    }
}

fn taker_command(command: &str, actor_config: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lez-taker-cli"))
        .arg(command)
        .arg("--actor-config")
        .arg(actor_config)
        .output()
        .expect("run real Taker CLI")
}

fn taker_receipt_command(command: &str, receipt: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lez-taker-cli"))
        .arg(command)
        .arg("--receipt")
        .arg(receipt)
        .output()
        .expect("run receipt-bound real Taker CLI")
}

fn assert_secret_free(output: &Output, root: &Path) {
    let diagnostics = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    for forbidden in [
        PRIVATE_MARKER,
        "taker-claim-recovery-v1",
        root.to_string_lossy().as_ref(),
    ] {
        assert!(
            !diagnostics.contains(forbidden),
            "Taker lifecycle output exposed private material"
        );
    }
}

struct LifecycleFixture {
    root: TempDir,
    maker_config: PathBuf,
    taker_config: PathBuf,
    taker_receipt: PathBuf,
    maker_state: PathBuf,
    taker_state: PathBuf,
}

impl LifecycleFixture {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("isolated Taker lifecycle root");
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700))
            .expect("make Taker lifecycle root owner-private");
        private_file(&root.path().join("agreement"), AGREEMENT);
        for role in ["maker", "taker"] {
            private_file(&root.path().join(format!("{role}-claim-key")), CLAIM_KEY);
            private_file(&root.path().join(format!("{role}-zcash-key")), &[0x72; 32]);
            private_file(
                &root.path().join(format!("{role}-capability")),
                format!("{PRIVATE_MARKER}-{role}").as_bytes(),
            );
            private_file(
                &root.path().join(format!("{role}-cookie")),
                b"user:password\n",
            );
        }
        private_file(&root.path().join("maker-preimage"), &[0x73; 32]);

        let maker_config = root.path().join("maker-config.json");
        let taker_config = root.path().join("taker-config.json");
        private_file(
            &maker_config,
            &serde_json::to_vec_pretty(&actor_config(root.path(), "maker", true))
                .expect("serialize Maker config"),
        );
        private_file(
            &taker_config,
            &serde_json::to_vec_pretty(&actor_config(root.path(), "taker", false))
                .expect("serialize Taker config"),
        );
        let taker_config_bytes = fs::read(&taker_config).unwrap();
        let taker_receipt = root.path().join("taker-acceptance-receipt.json");
        private_file(
            &taker_receipt,
            &serde_json::to_vec(&json!({
                "schema_version": 1,
                "swap_id": "m5-taker-lifecycle-001",
                "role": "taker",
                "agreement_sha256": hex::encode(Sha256::digest(AGREEMENT)),
                "actor_config_file": taker_config,
                "actor_config_sha256": hex::encode(Sha256::digest(&taker_config_bytes)),
                "actor_state_database": root.path().join("taker-state.sqlite3")
            }))
            .unwrap(),
        );
        Self {
            maker_state: root.path().join("maker-state.sqlite3"),
            taker_state: root.path().join("taker-state.sqlite3"),
            root,
            maker_config,
            taker_config,
            taker_receipt,
        }
    }
}

fn actor_config(root: &Path, role: &str, funder: bool) -> Value {
    let bridge_port = if role == "maker" { 19_101 } else { 19_102 };
    let signer_account_id = if role == "maker" {
        "55".repeat(32)
    } else {
        "66".repeat(32)
    };
    json!({
        "schema_version": 3,
        "role": role,
        "run_id": "m5-taker-lifecycle-process",
        "swap_id": "m5-taker-lifecycle-001",
        "signed_agreement_file": root.join("agreement"),
        "signed_agreement_sha256": hex::encode(Sha256::digest(AGREEMENT)),
        "role_state_db": root.join(format!("{role}-state.sqlite3")),
        "claim_recovery": {
            "key_id": format!("{role}-claim-recovery-v1"),
            "key_file": root.join(format!("{role}-claim-key"))
        },
        "claim_preimage_file": funder.then(|| root.join("maker-preimage")),
        "zcash_key_file": root.join(format!("{role}-zcash-key")),
        "bridge": {
            "endpoint": format!("http://127.0.0.1:{bridge_port}"),
            "journal_db": root.join(format!("{role}-journal.sqlite3")),
            "capability_file": root.join(format!("{role}-capability")),
            "runtime": {
                "sidecar_role": role,
                "compatibility": "nssa_v0_1_2",
                "chain_id": "11".repeat(32),
                "channel_id": "22".repeat(32),
                "genesis_block_hash": "33".repeat(32),
                "escrow_program_id": "44".repeat(32),
                "signer_account_id": signer_account_id
            },
            "request_timeout_millis": 5_000
        },
        "zebra": {
            "route": {
                "kind": "deterministic_local",
                "endpoint": "http://127.0.0.1:19201",
                "cookie_file": root.join(format!("{role}-cookie"))
            },
            "identity": {
                "network": "regtest",
                "rpc_chain": "test",
                "consensus_branch_id": "c8e71055",
                "genesis_hash": "77".repeat(32)
            },
            "counterparty_scan_blocks": 1_000
        },
        "lez_discovery_window": {
            "start_height": 1,
            "max_blocks": 256
        },
        "zcash_funding_outpoints": if funder {
            json!([{"transaction_id": "aa".repeat(32), "output_index": 0}])
        } else {
            json!([])
        }
    })
}

fn private_file(path: &Path, contents: &[u8]) {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .expect("create private fixture file");
    file.write_all(contents)
        .expect("write private fixture file");
}
