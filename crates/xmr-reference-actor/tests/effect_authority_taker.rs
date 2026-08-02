use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _, symlink},
    path::{Path, PathBuf},
    process::Stdio,
    thread,
    time::{Duration, Instant},
};

use lez_swap_core::SwapId;
use lez_swap_store::MakerActorHeldLock;
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
    shared_wallet_file_password_file: PathBuf,
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
            shared_wallet_file_password_file: PathBuf::from(
                "/run/monero/shared-wallet-file.password",
            ),
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
    assert_eq!(
        monero.shared_wallet_file_password_file(),
        Path::new("/run/monero/shared-wallet-file.password")
    );
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

#[test]
fn composed_effect_command_hands_off_exact_fds_and_locks() {
    const PROGRAM: &[u8] = br#"#!/bin/sh
set -eu
for fd in 197 198 199 200 201 202 203 204 205 206 207 208 209 210; do
    test -e "/proc/self/fd/$fd"
done
test ! -e /proc/self/fd/211
for fd in 200 201 202 203 204 205 206 207 208 209 210; do
    sha256sum "/proc/self/fd/$fd"
done
printf '%s\n' fd211=absent
printf ready > "$READY_FILE"
while [ ! -e "$GATE_FILE" ]; do
    sleep 0.02
done
"#;

    let root = tempfile::tempdir().expect("isolated composed-command root");
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let ready = root.path().join("child.ready");
    let gate = root.path().join("child.gate");
    let program = root.path().join("tag14-authorize");
    write_executable(&program, PROGRAM, 0o700);

    let (mut spec, runtime, secret_paths, secret_values) = custody_manifest(root.path());
    spec.taker_tools
        .tag14_authorize
        .program
        .clone_from(&program);
    spec.taker_tools.tag14_authorize.program_sha256 = hex::encode(Sha256::digest(PROGRAM));
    let authority = validated(&spec);
    let inputs = authority
        .pin_effect_inputs_at_use()
        .expect("pin every effect input before source replacement");
    let executable = authority
        .taker_tools()
        .expect("Taker tool plan")
        .tag14_authorize()
        .verify_program_at_use()
        .expect("pin the exact effect program before source replacement");

    let swap_id = SwapId::new(hex::encode(SWAP)).expect("canonical swap ID");
    let actor_state = root.path().join("actor-state.sqlite3");
    let workflow_state = root.path().join("workflow-state.sqlite3");
    let actor_lock =
        MakerActorHeldLock::acquire_for(&swap_id, &actor_state).expect("acquire actor lock");
    let workflow_lock =
        MakerActorHeldLock::acquire_for(&swap_id, &workflow_state).expect("acquire workflow lock");

    fs::write(&program, b"#!/bin/sh\nexit 99\n").expect("replace named program bytes");
    fs::write(&runtime, b"{\"generation\":2}\n").expect("replace named runtime bytes");
    for (index, path) in secret_paths.iter().enumerate() {
        fs::write(path, format!("replacement-{index}\n")).expect("replace named secret bytes");
    }

    let mut command = inputs
        .into_command(executable, &actor_lock, &workflow_lock)
        .expect("compose one collision-free executable, lock, runtime, and secret FD plan");
    command
        .env("READY_FILE", &ready)
        .env("GATE_FILE", &gate)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn composed effect command");

    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready.exists() {
        assert!(
            child.try_wait().expect("poll child before ready").is_none(),
            "composed child exited before proving its inherited descriptors"
        );
        assert!(
            Instant::now() < deadline,
            "composed child did not report ready before deadline"
        );
        thread::sleep(Duration::from_millis(20));
    }

    drop(command);
    drop(actor_lock);
    drop(workflow_lock);
    assert!(
        MakerActorHeldLock::acquire_for(&swap_id, &actor_state).is_err(),
        "the live child alone retains the actor lock"
    );
    assert!(
        MakerActorHeldLock::acquire_for(&swap_id, &workflow_state).is_err(),
        "the live child alone retains the workflow lock"
    );

    write_private_source(&gate, b"continue\n");
    let output = child
        .wait_with_output()
        .expect("collect composed child output");
    assert_effect_child_output(output, secret_values);

    drop(MakerActorHeldLock::acquire_for(&swap_id, &actor_state).unwrap());
    drop(MakerActorHeldLock::acquire_for(&swap_id, &workflow_state).unwrap());
}

fn assert_effect_child_output(output: std::process::Output, secret_values: Vec<Vec<u8>>) {
    assert!(
        output.status.success(),
        "composed effect command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());

    let mut expected_bytes = vec![b"{\"generation\":1}\n".to_vec()];
    expected_bytes.extend(secret_values);
    let expected_hashes = expected_bytes
        .iter()
        .map(|bytes| hex::encode(Sha256::digest(bytes)))
        .collect::<Vec<_>>();
    let stdout = String::from_utf8(output.stdout).expect("ASCII digest output");
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 12);
    let actual_hashes = lines[..11]
        .iter()
        .map(|line| {
            line.split_whitespace()
                .next()
                .expect("sha256sum digest")
                .to_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(actual_hashes, expected_hashes);
    assert_eq!(lines[11], "fd211=absent");
}

#[test]
fn effect_inputs_pin_runtime_and_all_trailing_newline_secrets_before_name_replacement() {
    let root = tempfile::tempdir().expect("isolated effect-input root");
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let (spec, runtime, secret_paths, secret_values) = custody_manifest(root.path());
    let authority = validated(&spec);
    let pinned = authority
        .pin_effect_inputs_at_use()
        .expect("securely pin schema-v3 effect inputs");

    assert_eq!(pinned.runtime_bytes(), b"{\"generation\":1}\n");
    let monero = pinned.monero();
    let secrets = [
        pinned.capability(),
        monero.daemon().username(),
        monero.daemon().password(),
        monero.funding_wallet().username(),
        monero.funding_wallet().password(),
        monero.shared_wallet().username(),
        monero.shared_wallet().password(),
        monero.role_wallet().username(),
        monero.role_wallet().password(),
        monero.shared_wallet_file_password(),
    ];
    let child_paths = secrets
        .iter()
        .map(|secret| secret.child_path().to_path_buf())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        child_paths.len(),
        10,
        "every secret has one distinct descriptor"
    );
    for (secret, expected) in secrets.iter().zip(&secret_values) {
        assert!(secret.child_path().starts_with("/proc/self/fd"));
        assert_eq!(secret.redacted_len(), expected.len());
        assert_eq!(secret.sha256(), <[u8; 32]>::from(Sha256::digest(expected)));
        assert_eq!(
            fs::read(secret.child_path()).expect("read descriptor snapshot"),
            *expected
        );
    }

    replace_named(&runtime, b"{\"generation\":2}\n");
    for (index, path) in secret_paths.iter().enumerate() {
        replace_named(path, format!("replacement-{index}\n").as_bytes());
    }
    assert_eq!(pinned.runtime_bytes(), b"{\"generation\":1}\n");
    for (secret, expected) in secrets.iter().zip(&secret_values) {
        assert_eq!(fs::read(secret.child_path()).unwrap(), *expected);
    }
    assert!(
        authority.pin_effect_inputs_at_use().is_err(),
        "fresh runtime pin must reject named digest drift"
    );

    fs::write(&runtime, b"{\"generation\":1}\n").unwrap();
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o600)).unwrap();
    fs::set_permissions(&secret_paths[0], fs::Permissions::from_mode(0o644)).unwrap();
    assert!(
        authority.pin_effect_inputs_at_use().is_err(),
        "fresh secret snapshot rejects unsafe named-file mode"
    );
}

#[test]
fn effect_input_custody_rejects_invalid_content_storage_and_aliases() {
    let invalid: &[&[u8]] = &[
        b"",
        b"\n",
        b"\r\n",
        b"embedded\nnewline",
        b"multiple\n\n",
        b"nul\0secret",
    ];
    for bytes in invalid {
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let (spec, _, secrets, _) = custody_manifest(root.path());
        fs::write(&secrets[0], bytes).unwrap();
        assert!(validated(&spec).pin_effect_inputs_at_use().is_err());
    }

    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let (spec, _, secrets, _) = custody_manifest(root.path());
    fs::write(&secrets[0], vec![b'x'; 257]).unwrap();
    assert!(validated(&spec).pin_effect_inputs_at_use().is_err());

    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let (spec, _, secrets, _) = custody_manifest(root.path());
    fs::remove_file(&secrets[1]).unwrap();
    symlink(&secrets[2], &secrets[1]).unwrap();
    assert!(validated(&spec).pin_effect_inputs_at_use().is_err());

    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let (spec, _, secrets, _) = custody_manifest(root.path());
    fs::remove_file(&secrets[4]).unwrap();
    fs::hard_link(&secrets[5], &secrets[4]).unwrap();
    assert!(validated(&spec).pin_effect_inputs_at_use().is_err());

    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let (mut spec, runtime, _, _) = custody_manifest(root.path());
    let oversized = vec![b'r'; 16 * 1024 + 1];
    fs::write(&runtime, &oversized).unwrap();
    spec.lez.runtime_sha256 = hex::encode(Sha256::digest(&oversized));
    assert!(validated(&spec).pin_effect_inputs_at_use().is_err());

    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let (spec, _, _, _) = custody_manifest(root.path());
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(validated(&spec).pin_effect_inputs_at_use().is_err());

    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let (mut spec, _, _, _) = custody_manifest(root.path());
    spec.monero.role_wallet.username_file = spec.monero.daemon.username_file.clone();
    assert!(
        authority_or_input_pin_rejects(&spec),
        "cross-RPC path aliases fail before any secret is exposed"
    );
    spec.monero.daemon.password_file = spec.monero.daemon.username_file.clone();
    assert!(
        load_validated_xmr_effect_authority_bytes(
            &canonical(&spec),
            ActorRole::Taker,
            SWAP,
            AGREEMENT,
            ACTIVATION,
            RUN,
        )
        .is_err(),
        "username/password overlap is invalid authority"
    );

    let root = tempfile::tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let (base, runtime, secrets, _) = custody_manifest(root.path());
    let aliases = std::iter::once(runtime)
        .chain(secrets.into_iter().take(9))
        .collect::<Vec<_>>();
    for alias in aliases {
        let mut spec = base.clone();
        spec.monero.shared_wallet_file_password_file = alias;
        assert!(
            authority_or_input_pin_rejects(&spec),
            "shared-wallet file password must not alias any other effect input"
        );
    }

    let unsafe_spec = base;
    fs::set_permissions(
        &unsafe_spec.monero.shared_wallet_file_password_file,
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert!(validated(&unsafe_spec).pin_effect_inputs_at_use().is_err());
}

#[test]
fn shared_wallet_file_password_is_required_and_normalized() {
    let canonical_bytes = canonical(&manifest());
    let mut missing_bytes =
        String::from_utf8(canonical_bytes).expect("canonical authority is UTF-8");
    let encoded_path =
        serde_json::to_string(&PathBuf::from("/run/monero/shared-wallet-file.password"))
            .expect("serialize wallet-file password path");
    let field = format!(",\"shared_wallet_file_password_file\":{encoded_path}");
    assert_eq!(missing_bytes.matches(&field).count(), 1);
    missing_bytes = missing_bytes.replacen(&field, "", 1);
    assert!(
        load_validated_xmr_effect_authority_bytes(
            missing_bytes.as_bytes(),
            ActorRole::Taker,
            SWAP,
            AGREEMENT,
            ACTIVATION,
            RUN,
        )
        .is_err(),
        "shared-wallet file password authority is mandatory"
    );

    let mut relative = manifest();
    relative.monero.shared_wallet_file_password_file = PathBuf::from("shared-wallet-file.password");
    assert!(
        load_validated_xmr_effect_authority_bytes(
            &canonical(&relative),
            ActorRole::Taker,
            SWAP,
            AGREEMENT,
            ACTIVATION,
            RUN,
        )
        .is_err(),
        "shared-wallet file password authority must be normalized and absolute"
    );
}

fn authority_or_input_pin_rejects(spec: &TakerEffectAuthority) -> bool {
    match load_validated_xmr_effect_authority_bytes(
        &canonical(spec),
        ActorRole::Taker,
        SWAP,
        AGREEMENT,
        ACTIVATION,
        RUN,
    ) {
        Ok(authority) => authority.pin_effect_inputs_at_use().is_err(),
        Err(_) => true,
    }
}

fn validated(spec: &TakerEffectAuthority) -> xmr_reference_actor::ValidatedXmrEffectAuthorityV1 {
    load_validated_xmr_effect_authority_bytes(
        &canonical(spec),
        ActorRole::Taker,
        SWAP,
        AGREEMENT,
        ACTIVATION,
        RUN,
    )
    .expect("canonical custody authority")
}

fn custody_manifest(root: &Path) -> (TakerEffectAuthority, PathBuf, Vec<PathBuf>, Vec<Vec<u8>>) {
    let runtime = root.join("lez-runtime.json");
    write_private_source(&runtime, b"{\"generation\":1}\n");
    let secret_paths = [
        "lez.capability",
        "daemon.username",
        "daemon.password",
        "funding.username",
        "funding.password",
        "shared.username",
        "shared.password",
        "taker.username",
        "taker.password",
        "shared-wallet-file.password",
    ]
    .map(|name| root.join(name))
    .to_vec();
    let secret_values = [
        b"lez-capability\n".to_vec(),
        b"daemon-user\r\n".to_vec(),
        b"daemon-password\n".to_vec(),
        b"funding-user\n".to_vec(),
        b"funding-password\r\n".to_vec(),
        b"shared-user".to_vec(),
        b"shared-password\n".to_vec(),
        b"taker-user\r\n".to_vec(),
        b"taker-password\n".to_vec(),
        b"shared-wallet-file-password\n".to_vec(),
    ]
    .to_vec();
    for (path, bytes) in secret_paths.iter().zip(&secret_values) {
        write_private_source(path, bytes);
    }

    let mut spec = manifest();
    spec.workflow_journal = root.join("workflow.sqlite3");
    spec.adaptor_journal = root.join("adaptor.sqlite3");
    spec.evidence_root = root.join("evidence");
    spec.lez.runtime_file.clone_from(&runtime);
    spec.lez.runtime_sha256 = hex::encode(Sha256::digest(b"{\"generation\":1}\n"));
    spec.lez.capability_file.clone_from(&secret_paths[0]);
    let rpcs = [
        &mut spec.monero.daemon,
        &mut spec.monero.funding_wallet,
        &mut spec.monero.shared_wallet,
        &mut spec.monero.role_wallet,
    ];
    for (index, rpc) in rpcs.into_iter().enumerate() {
        rpc.username_file.clone_from(&secret_paths[1 + index * 2]);
        rpc.password_file.clone_from(&secret_paths[2 + index * 2]);
    }
    spec.monero
        .shared_wallet_file_password_file
        .clone_from(&secret_paths[9]);
    (spec, runtime, secret_paths, secret_values)
}

fn write_private_source(path: &Path, bytes: &[u8]) {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn replace_named(path: &Path, bytes: &[u8]) {
    let mut old = path.as_os_str().to_os_string();
    old.push(".old");
    fs::rename(path, PathBuf::from(old)).unwrap();
    write_private_source(path, bytes);
}
