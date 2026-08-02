use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _, symlink},
    path::{Path, PathBuf},
};

use serde::Serialize;
use sha2::{Digest as _, Sha256};
use xmr_reference_actor::{ActorRole, load_validated_xmr_effect_authority_bytes};

const SWAP: [u8; 32] = [0x81; 32];
const AGREEMENT: [u8; 32] = [0x82; 32];
const ACTIVATION: [u8; 32] = [0x83; 32];
const RUN: &str = "m5-xmr-taker-effect-run-1";

#[derive(Clone, Serialize)]
struct Tool {
    program: PathBuf,
    program_sha256: String,
    abi: &'static str,
}

#[derive(Clone, Serialize)]
struct TakerTools {
    tag14_authorize: Tool,
    finalized_classifier: Tool,
    monero_claim: Tool,
    monero_verify: Tool,
    tag16_refund: Tool,
}

#[derive(Clone, Serialize)]
struct LezRpc {
    sidecar_url: String,
    runtime_file: PathBuf,
    runtime_sha256: String,
    capability_file: PathBuf,
}

#[derive(Clone, Serialize)]
struct AuthenticatedRpc {
    url: String,
    username_file: PathBuf,
    password_file: PathBuf,
}

#[derive(Clone, Serialize)]
struct MoneroRpc {
    daemon: AuthenticatedRpc,
    funding_wallet: AuthenticatedRpc,
    shared_wallet: AuthenticatedRpc,
    role_wallet: AuthenticatedRpc,
}

#[derive(Clone, Serialize)]
struct TakerEffectAuthority {
    schema_version: u16,
    pair: &'static str,
    role: ActorRole,
    swap_id: String,
    agreement_commitment: String,
    activation_commitment: String,
    run_id: &'static str,
    workflow_journal: PathBuf,
    adaptor_journal: PathBuf,
    evidence_root: PathBuf,
    lez: LezRpc,
    monero: MoneroRpc,
    taker_tools: TakerTools,
}

fn tool(name: &str, byte: u8, abi: &'static str) -> Tool {
    Tool {
        program: PathBuf::from(format!("/opt/lez/bin/{name}")),
        program_sha256: format!("{byte:02x}").repeat(32),
        abi,
    }
}

fn rpc(port: u16, name: &str) -> AuthenticatedRpc {
    AuthenticatedRpc {
        url: format!("http://127.0.0.1:{port}/"),
        username_file: PathBuf::from(format!("/run/monero/{name}.username")),
        password_file: PathBuf::from(format!("/run/monero/{name}.password")),
    }
}

fn manifest() -> TakerEffectAuthority {
    TakerEffectAuthority {
        schema_version: 1,
        pair: "monero",
        role: ActorRole::Taker,
        swap_id: hex::encode(SWAP),
        agreement_commitment: hex::encode(AGREEMENT),
        activation_commitment: hex::encode(ACTIVATION),
        run_id: RUN,
        workflow_journal: PathBuf::from("/var/lib/lez/taker/xmr-workflow.sqlite"),
        adaptor_journal: PathBuf::from("/var/lib/lez/taker/adaptor.sqlite"),
        evidence_root: PathBuf::from("/var/lib/lez/taker/evidence"),
        lez: LezRpc {
            sidecar_url: "http://127.0.0.1:32972/".to_owned(),
            runtime_file: PathBuf::from("/run/lez/taker-runtime.json"),
            runtime_sha256: "84".repeat(32),
            capability_file: PathBuf::from("/run/lez/taker.capability"),
        },
        monero: MoneroRpc {
            daemon: rpc(32974, "daemon"),
            funding_wallet: rpc(32975, "funding"),
            shared_wallet: rpc(32976, "shared"),
            role_wallet: rpc(32977, "taker"),
        },
        taker_tools: TakerTools {
            tag14_authorize: tool("xmr-tag14-authorize", 0x85, "lez_xmr_tag14_authorize_v1"),
            finalized_classifier: tool("xmr-classifier", 0x86, "lez_xmr_finalized_classifier_v1"),
            monero_claim: tool("xmr-claim-sweep", 0x87, "lez_xmr_monero_claim_sweep_v2"),
            monero_verify: tool("xmr-verify", 0x88, "lez_xmr_monero_verify_v2"),
            tag16_refund: tool("xmr-reference-tag16", 0x89, "lez_xmr_tag16_refund_v1"),
        },
    }
}

fn canonical(value: &TakerEffectAuthority) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(value).expect("serialize Taker authority");
    bytes.push(b'\n');
    bytes
}

#[test]
fn taker_profile_is_fixed_and_cannot_cross_role_or_tool_authority() {
    let valid = manifest();
    let authority = load_validated_xmr_effect_authority_bytes(
        &canonical(&valid),
        ActorRole::Taker,
        SWAP,
        AGREEMENT,
        ACTIVATION,
        RUN,
    )
    .expect("canonical Taker effect authority");
    assert_eq!(authority.role(), ActorRole::Taker);
    assert_eq!(
        authority.workflow_journal(),
        Path::new("/var/lib/lez/taker/xmr-workflow.sqlite")
    );

    let mut crossed = manifest();
    crossed.role = ActorRole::Maker;
    assert!(
        load_validated_xmr_effect_authority_bytes(
            &canonical(&crossed),
            ActorRole::Maker,
            SWAP,
            AGREEMENT,
            ACTIVATION,
            RUN,
        )
        .is_err()
    );

    let mut drifted = manifest();
    drifted.taker_tools.tag16_refund.abi = "lez_xmr_tag15_claim_v1";
    assert!(
        load_validated_xmr_effect_authority_bytes(
            &canonical(&drifted),
            ActorRole::Taker,
            SWAP,
            AGREEMENT,
            ACTIVATION,
            RUN,
        )
        .is_err()
    );

    let mut bad_hash = manifest();
    bad_hash.taker_tools.monero_claim.program_sha256 = "AA".repeat(32);
    assert!(
        load_validated_xmr_effect_authority_bytes(
            &canonical(&bad_hash),
            ActorRole::Taker,
            SWAP,
            AGREEMENT,
            ACTIVATION,
            RUN,
        )
        .is_err()
    );
}

#[test]
fn validated_taker_authority_exposes_only_typed_role_fixed_execution_inputs() {
    let valid = manifest();
    let authority = load_validated_xmr_effect_authority_bytes(
        &canonical(&valid),
        ActorRole::Taker,
        SWAP,
        AGREEMENT,
        ACTIVATION,
        RUN,
    )
    .expect("canonical Taker effect authority");
    assert!(authority.maker_tools().is_none());

    assert_eq!(
        authority.evidence_root(),
        Path::new("/var/lib/lez/taker/evidence")
    );
    let lez = authority.lez();
    assert_eq!(lez.sidecar_url().as_str(), "http://127.0.0.1:32972/");
    assert_eq!(lez.runtime_file(), Path::new("/run/lez/taker-runtime.json"));
    assert_eq!(lez.runtime_sha256(), [0x84; 32]);
    assert_eq!(
        lez.capability_file(),
        Path::new("/run/lez/taker.capability")
    );

    let monero = authority.monero();
    for (rpc, url, username, password) in [
        (
            monero.daemon(),
            "http://127.0.0.1:32974/",
            "/run/monero/daemon.username",
            "/run/monero/daemon.password",
        ),
        (
            monero.funding_wallet(),
            "http://127.0.0.1:32975/",
            "/run/monero/funding.username",
            "/run/monero/funding.password",
        ),
        (
            monero.shared_wallet(),
            "http://127.0.0.1:32976/",
            "/run/monero/shared.username",
            "/run/monero/shared.password",
        ),
        (
            monero.role_wallet(),
            "http://127.0.0.1:32977/",
            "/run/monero/taker.username",
            "/run/monero/taker.password",
        ),
    ] {
        assert_eq!(rpc.url().as_str(), url);
        assert_eq!(rpc.username_file(), Path::new(username));
        assert_eq!(rpc.password_file(), Path::new(password));
    }

    let tools = authority
        .taker_tools()
        .expect("Taker authority retains shared role-fixed tool views");
    for (tool, program, digest, abi) in [
        (
            tools.tag14_authorize(),
            "/opt/lez/bin/xmr-tag14-authorize",
            [0x85; 32],
            "lez_xmr_tag14_authorize_v1",
        ),
        (
            tools.finalized_classifier(),
            "/opt/lez/bin/xmr-classifier",
            [0x86; 32],
            "lez_xmr_finalized_classifier_v1",
        ),
        (
            tools.monero_claim(),
            "/opt/lez/bin/xmr-claim-sweep",
            [0x87; 32],
            "lez_xmr_monero_claim_sweep_v2",
        ),
        (
            tools.monero_verify(),
            "/opt/lez/bin/xmr-verify",
            [0x88; 32],
            "lez_xmr_monero_verify_v2",
        ),
        (
            tools.tag16_refund(),
            "/opt/lez/bin/xmr-reference-tag16",
            [0x89; 32],
            "lez_xmr_tag16_refund_v1",
        ),
    ] {
        assert_eq!(tool.program(), Path::new(program));
        assert_eq!(tool.program_sha256(), digest);
        assert_eq!(tool.abi(), abi);
    }

    // The returned type is Taker-specific: the executable surface above has
    // no Maker fund/claim/refund slots and exposes no serde/raw-JSON handle.
}

#[test]
fn pinned_taker_tool_is_reverified_at_use_against_storage_and_bytes() {
    const PROGRAM: &[u8] = b"#!/bin/sh\nexit 0\n";
    let root = tempfile::tempdir().expect("isolated executable root");
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700))
        .expect("make executable root owner-private");
    let program = root.path().join("tag14-authorize");
    write_executable(&program, PROGRAM, 0o700);

    let mut valid = manifest();
    valid.taker_tools.tag14_authorize.program = program.clone();
    valid.taker_tools.tag14_authorize.program_sha256 = hex::encode(Sha256::digest(PROGRAM));
    let authority = load_validated_xmr_effect_authority_bytes(
        &canonical(&valid),
        ActorRole::Taker,
        SWAP,
        AGREEMENT,
        ACTIVATION,
        RUN,
    )
    .expect("canonical Taker effect authority");
    let tool = authority
        .taker_tools()
        .expect("Taker tool plan")
        .tag14_authorize();
    let pinned = tool
        .verify_program_at_use()
        .expect("exact owner executable matches pinned bytes");

    fs::write(&program, b"changed").expect("replace executable bytes");
    let output = pinned
        .into_command()
        .expect("construct descriptor-addressed command")
        .output()
        .expect("execute the sealed pre-replacement bytes");
    assert!(output.status.success());
    assert!(tool.verify_program_at_use().is_err());

    fs::remove_file(&program).expect("remove changed executable");
    let target = root.path().join("symlink-target");
    write_executable(&target, PROGRAM, 0o700);
    symlink(&target, &program).expect("replace executable with symlink");
    assert!(tool.verify_program_at_use().is_err());

    fs::remove_file(&program).expect("remove executable symlink");
    write_executable(&program, PROGRAM, 0o777);
    assert!(tool.verify_program_at_use().is_err());
}

fn write_executable(path: &Path, bytes: &[u8], mode: u32) {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)
        .expect("create executable");
    file.write_all(bytes).expect("write executable");
    file.sync_all().expect("sync executable");
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("set executable mode");
}
