#![cfg(feature = "sessions")]

use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::Write as _,
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    process::{Command, Output},
};

use clap::Parser as _;
use lez_adaptor_role_runner::{Cli as RunnerCli, execute as execute_runner};
use lez_xmr_swap_sdk::{
    MoneroAddressNetworkV1, MoneroPrivateViewKey, MoneroSharedAddressV1,
    ValidatedXmrAgreementBodyV1, XmrActivatedAgreementV1, XmrAgreementBodyV1, XmrAgreementV1,
    XmrLezTermsV1, XmrMessagesV1, XmrMoneroTermsV1, XmrNamedProfileV1, XmrParticipantsV1,
    XmrSwapDirectionV1, XmrWindowsV1,
};
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use serde_json::Value;
use tempfile::TempDir;
use xmr_reference_actor::ValidatedRolePacket;

const TAKER_OWNER: &str = "1515151515151515151515151515151515151515151515151515151515151515";
const MAKER_OWNER: &str = "2424242424242424242424242424242424242424242424242424242424242424";

struct Fixture {
    _directory: TempDir,
    material: PathBuf,
    exchange: PathBuf,
    maker_root: PathBuf,
    taker_root: PathBuf,
    maker_packet: PathBuf,
    taker_packet: PathBuf,
    unsigned_stage_a: PathBuf,
}

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_xmr-reference-actor")
}

fn owner_directory(parent: &Path, name: &str) -> PathBuf {
    let path = parent.join(name);
    fs::create_dir(&path).expect("create owner directory");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
        .expect("set owner directory mode");
    path
}

fn write_new_private(path: &Path, bytes: &[u8]) {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    let mut file = options.open(path).expect("create private fixture file");
    file.write_all(bytes).expect("write private fixture file");
    file.sync_all().expect("sync private fixture file");
}

fn assert_success(output: &Output, label: &str) {
    assert!(
        output.status.success(),
        "{label} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty(), "{label} wrote stdout");
    assert!(output.stderr.is_empty(), "{label} wrote stderr");
}

fn assert_private_output(path: &Path) {
    let metadata = fs::symlink_metadata(path).expect("output metadata");
    assert!(metadata.file_type().is_file());
    assert_eq!(metadata.mode() & 0o7777, 0o600);
    assert_eq!(metadata.nlink(), 1);
}

fn assert_session_bundle(path: &Path) {
    let metadata = fs::symlink_metadata(path).expect("session-root metadata");
    assert!(metadata.file_type().is_dir());
    assert_eq!(metadata.mode() & 0o7777, 0o700);
    let mut names = fs::read_dir(path)
        .expect("session root")
        .map(|entry| {
            entry
                .expect("session entry")
                .file_name()
                .into_string()
                .expect("ASCII session filename")
        })
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(names, ["claim.json", "refund.json"]);
    for name in names {
        assert_private_output(&path.join(name));
    }
}

fn assert_no_staging_entries(path: &Path) {
    for entry in fs::read_dir(path).expect("list session parent") {
        let name = entry
            .expect("session-parent entry")
            .file_name()
            .into_string()
            .expect("ASCII session-parent filename");
        assert!(!name.starts_with(".xmr-reference-actor-"), "{name}");
    }
}

fn provision_pair() -> Fixture {
    let directory = TempDir::new().expect("temporary root");
    let material = owner_directory(directory.path(), "material");
    let exchange = owner_directory(directory.path(), "exchange");
    let taker_root = material.join("taker");
    let maker_root = material.join("maker");
    let taker_packet = exchange.join("taker.json");
    let maker_packet = exchange.join("maker.json");

    let taker = Command::new(binary())
        .args(["provision", "taker", "--private-root"])
        .arg(&taker_root)
        .args(["--lez-owner-account", TAKER_OWNER, "--public-packet"])
        .arg(&taker_packet)
        .output()
        .expect("spawn Taker provision");
    assert_success(&taker, "Taker provision");

    let maker = Command::new(binary())
        .args(["provision", "maker", "--private-root"])
        .arg(&maker_root)
        .args(["--lez-owner-account", MAKER_OWNER, "--shared-view-key-file"])
        .arg(taker_root.join("monero-view.key"))
        .arg("--public-packet")
        .arg(&maker_packet)
        .output()
        .expect("spawn Maker provision");
    assert_success(&maker, "Maker provision");

    let unsigned_stage_a = exchange.join("unsigned-stage-a.bin");
    write_new_private(
        &unsigned_stage_a,
        &unsigned_wire(&maker_packet, &taker_packet),
    );
    Fixture {
        _directory: directory,
        material,
        exchange,
        maker_root,
        taker_root,
        maker_packet,
        taker_packet,
        unsigned_stage_a,
    }
}

fn unsigned_wire(maker_path: &Path, taker_path: &Path) -> Vec<u8> {
    let maker = ValidatedRolePacket::read(maker_path).expect("Maker packet");
    let taker = ValidatedRolePacket::read(taker_path).expect("Taker packet");
    let participants = XmrParticipantsV1::new(maker.identity().clone(), taker.identity().clone());
    let claim_key = participants
        .claim_aggregate_x_only_key()
        .expect("claim aggregate key");
    let refund_key = participants
        .refund_aggregate_x_only_key()
        .expect("refund aggregate key");
    let shared = MoneroSharedAddressV1::derive_from_public_view_key(
        MoneroAddressNetworkV1::Regtest,
        maker.proof(),
        taker.proof(),
        maker.public_view_key(),
    )
    .expect("shared address");
    let maker_proof = maker.proof().to_wire_bytes().expect("Maker proof wire");
    let taker_proof = taker.proof().to_wire_bytes().expect("Taker proof wire");
    let body = XmrAgreementBodyV1::new(
        XmrSwapDirectionV1::TakerSellsLez,
        XmrNamedProfileV1::AcceleratedRegtest,
        [19; 32],
        participants,
        XmrMoneroTermsV1::new(
            MoneroAddressNetworkV1::Regtest,
            [31; 32],
            1_000_000_000_000,
            XmrNamedProfileV1::AcceleratedRegtest.required_monero_confirmations(),
            maker_proof,
            taker_proof,
            shared.public_view_key(),
            shared.public_spend_key(),
            shared.address_string(),
        ),
        XmrLezTermsV1::new(
            [40; 32],
            [41; 32],
            [42; 8],
            [43; 8],
            XmrNamedProfileV1::AcceleratedRegtest.required_lez_finality_units(),
            [44; 32],
            [45; 32],
            taker.identity().lez_owner_account(),
            maker.identity().lez_owner_account(),
            claim_key,
            XmrLezTermsV1::authority_account_for_key(claim_key),
            refund_key,
            XmrLezTermsV1::authority_account_for_key(refund_key),
            maker.proof().transcript_commitment(),
            taker.proof().transcript_commitment(),
            500,
        ),
        XmrMessagesV1::new([51; 32], [52; 32], [53; 32]),
        XmrWindowsV1::new(10_000, 20_000, 30_000),
    );
    ValidatedXmrAgreementBodyV1::validate(body)
        .expect("valid Stage-A body")
        .encode_unsigned_wire()
        .expect("unsigned Stage-A wire")
}

fn sign(fixture: &Fixture, role: &str, output: &Path) -> Output {
    let (root, own, peer) = match role {
        "maker" => (
            &fixture.maker_root,
            &fixture.maker_packet,
            &fixture.taker_packet,
        ),
        "taker" => (
            &fixture.taker_root,
            &fixture.taker_packet,
            &fixture.maker_packet,
        ),
        _ => panic!("unknown role"),
    };
    Command::new(binary())
        .args(["sign-stage-a", role, "--private-root"])
        .arg(root)
        .arg("--own-public-packet")
        .arg(own)
        .arg("--peer-public-packet")
        .arg(peer)
        .arg("--unsigned-stage-a")
        .arg(&fixture.unsigned_stage_a)
        .arg("--output-signature")
        .arg(output)
        .output()
        .expect("spawn Stage-A signer")
}

fn assemble(fixture: &Fixture, maker: &Path, taker: &Path, output: &Path) -> Output {
    Command::new(binary())
        .arg("assemble-stage-a")
        .arg("--maker-public-packet")
        .arg(&fixture.maker_packet)
        .arg("--taker-public-packet")
        .arg(&fixture.taker_packet)
        .arg("--unsigned-stage-a")
        .arg(&fixture.unsigned_stage_a)
        .arg("--maker-signature")
        .arg(maker)
        .arg("--taker-signature")
        .arg(taker)
        .arg("--output-stage-a")
        .arg(output)
        .output()
        .expect("spawn Stage-A assembler")
}

fn initialize(fixture: &Fixture, role: &str, agreement: &Path, session_root: &Path) -> Output {
    let (root, own, peer) = match role {
        "maker" => (
            &fixture.maker_root,
            &fixture.maker_packet,
            &fixture.taker_packet,
        ),
        "taker" => (
            &fixture.taker_root,
            &fixture.taker_packet,
            &fixture.maker_packet,
        ),
        _ => panic!("unknown role"),
    };
    Command::new(binary())
        .args(["initialize-sessions", role, "--private-root"])
        .arg(root)
        .arg("--own-public-packet")
        .arg(own)
        .arg("--peer-public-packet")
        .arg(peer)
        .arg("--agreement-stage-a")
        .arg(agreement)
        .arg("--session-root")
        .arg(session_root)
        .output()
        .expect("spawn session initializer")
}

fn signed_agreement(fixture: &Fixture) -> (PathBuf, PathBuf, PathBuf) {
    let maker_signature = fixture.exchange.join("maker.sig");
    let taker_signature = fixture.exchange.join("taker.sig");
    assert_success(
        &sign(fixture, "maker", &maker_signature),
        "Maker Stage-A sign",
    );
    assert_success(
        &sign(fixture, "taker", &taker_signature),
        "Taker Stage-A sign",
    );
    let agreement = fixture.exchange.join("agreement.bin");
    assert_success(
        &assemble(fixture, &maker_signature, &taker_signature, &agreement),
        "Stage-A assemble",
    );
    (maker_signature, taker_signature, agreement)
}

fn runner(role: &str, journal: &Path, session: &Path, action: Vec<OsString>) {
    let mut arguments = vec![
        OsString::from("lez-adaptor-role-runner"),
        OsString::from(role),
        OsString::from("--journal"),
        journal.as_os_str().to_owned(),
        OsString::from("--session"),
        session.as_os_str().to_owned(),
    ];
    arguments.extend(action);
    let cli = RunnerCli::try_parse_from(arguments).expect("parse runner command");
    execute_runner(&cli).expect("execute runner command");
}

#[allow(
    clippy::too_many_lines,
    reason = "the two-role transcript stays linear so the exact packet order remains auditable"
)]
fn run_adaptor_round(
    fixture: &Fixture,
    purpose: &str,
    maker_session: &Path,
    taker_session: &Path,
    maker_journal: &Path,
    taker_journal: &Path,
) -> [u8; 32] {
    let exchange = owner_directory(&fixture.exchange, &format!("{purpose}-round"));
    let maker_commitment = exchange.join("maker-commitment.json");
    let taker_commitment = exchange.join("taker-commitment.json");
    let maker_nonce = exchange.join("maker-nonce.json");
    let taker_nonce = exchange.join("taker-nonce.json");
    let maker_partial = exchange.join("maker-partial.json");
    let taker_partial = fixture
        .taker_root
        .join(format!("{purpose}-partial.private.json"));
    let taker_presignature = exchange.join("taker-presignature.json");

    for (role, journal, session, key, output) in [
        (
            "maker",
            maker_journal,
            maker_session,
            fixture.maker_root.join(format!("{purpose}.key")),
            &maker_commitment,
        ),
        (
            "taker",
            taker_journal,
            taker_session,
            fixture.taker_root.join(format!("{purpose}.key")),
            &taker_commitment,
        ),
    ] {
        runner(
            role,
            journal,
            session,
            vec![
                "reserve".into(),
                "--secret-key-file".into(),
                key.into_os_string(),
                "--output".into(),
                output.as_os_str().to_owned(),
            ],
        );
    }
    runner(
        "maker",
        maker_journal,
        maker_session,
        vec![
            "accept-commitment".into(),
            "--input".into(),
            taker_commitment.as_os_str().to_owned(),
        ],
    );
    runner(
        "taker",
        taker_journal,
        taker_session,
        vec![
            "accept-commitment".into(),
            "--input".into(),
            maker_commitment.as_os_str().to_owned(),
        ],
    );
    for (role, journal, session, output) in [
        ("maker", maker_journal, maker_session, &maker_nonce),
        ("taker", taker_journal, taker_session, &taker_nonce),
    ] {
        runner(
            role,
            journal,
            session,
            vec![
                "reveal-nonce".into(),
                "--output".into(),
                output.as_os_str().to_owned(),
            ],
        );
    }
    for (role, journal, session, input, key, output) in [
        (
            "maker",
            maker_journal,
            maker_session,
            &taker_nonce,
            fixture.maker_root.join(format!("{purpose}.key")),
            &maker_partial,
        ),
        (
            "taker",
            taker_journal,
            taker_session,
            &maker_nonce,
            fixture.taker_root.join(format!("{purpose}.key")),
            &taker_partial,
        ),
    ] {
        runner(
            role,
            journal,
            session,
            vec![
                "accept-nonce-sign".into(),
                "--input".into(),
                input.as_os_str().to_owned(),
                "--secret-key-file".into(),
                key.into_os_string(),
                "--output".into(),
                output.as_os_str().to_owned(),
            ],
        );
    }
    runner(
        "taker",
        taker_journal,
        taker_session,
        vec![
            "accept-peer-partial".into(),
            "--input".into(),
            maker_partial.as_os_str().to_owned(),
            "--output".into(),
            taker_presignature.as_os_str().to_owned(),
        ],
    );
    if purpose == "refund" {
        runner(
            "maker",
            maker_journal,
            maker_session,
            vec![
                "accept-peer-partial".into(),
                "--input".into(),
                taker_partial.as_os_str().to_owned(),
                "--output".into(),
                exchange.join("maker-presignature.json").into_os_string(),
            ],
        );
    }

    let packet: Value =
        serde_json::from_slice(&fs::read(&taker_partial).expect("Taker partial packet"))
            .expect("Taker partial packet JSON");
    hex::decode(packet["payload"].as_str().expect("Taker partial payload"))
        .expect("Taker partial hex")
        .try_into()
        .expect("Taker partial width")
}

fn stage_b_sign(
    fixture: &Fixture,
    role: &str,
    agreement: &Path,
    unsigned: &Path,
    output: &Path,
) -> Output {
    let (root, own, peer) = match role {
        "maker" => (
            &fixture.maker_root,
            &fixture.maker_packet,
            &fixture.taker_packet,
        ),
        "taker" => (
            &fixture.taker_root,
            &fixture.taker_packet,
            &fixture.maker_packet,
        ),
        _ => panic!("unknown role"),
    };
    Command::new(binary())
        .args(["sign-stage-b", role, "--private-root"])
        .arg(root)
        .arg("--own-public-packet")
        .arg(own)
        .arg("--peer-public-packet")
        .arg(peer)
        .arg("--agreement-stage-a")
        .arg(agreement)
        .arg("--unsigned-stage-b")
        .arg(unsigned)
        .arg("--output-signature")
        .arg(output)
        .output()
        .expect("spawn Stage-B signer")
}

#[test]
fn separate_role_processes_produce_byte_identical_stage_a_and_sessions() {
    let fixture = provision_pair();
    let maker_signature_a = fixture.exchange.join("maker-a.sig");
    let maker_signature_b = fixture.exchange.join("maker-b.sig");
    let taker_signature = fixture.exchange.join("taker.sig");
    assert_success(
        &sign(&fixture, "maker", &maker_signature_a),
        "Maker Stage-A sign A",
    );
    assert_success(
        &sign(&fixture, "maker", &maker_signature_b),
        "Maker Stage-A sign B",
    );
    assert_success(
        &sign(&fixture, "taker", &taker_signature),
        "Taker Stage-A sign",
    );
    assert_eq!(
        fs::read(&maker_signature_a).expect("Maker signature A"),
        fs::read(&maker_signature_b).expect("Maker signature B")
    );

    let agreement_a = fixture.exchange.join("agreement-a.bin");
    let agreement_b = fixture.exchange.join("agreement-b.bin");
    assert_success(
        &assemble(&fixture, &maker_signature_a, &taker_signature, &agreement_a),
        "Stage-A assemble A",
    );
    assert_success(
        &assemble(&fixture, &maker_signature_b, &taker_signature, &agreement_b),
        "Stage-A assemble B",
    );
    let agreement_bytes = fs::read(&agreement_a).expect("agreement A");
    assert_eq!(
        agreement_bytes,
        fs::read(&agreement_b).expect("agreement B")
    );
    XmrAgreementV1::from_wire(&agreement_bytes).expect("validated signed Stage A");

    let maker_sessions = fixture.material.join("maker-sessions");
    let taker_sessions = fixture.material.join("taker-sessions");
    assert_success(
        &initialize(&fixture, "maker", &agreement_a, &maker_sessions),
        "Maker session initialization",
    );
    assert_success(
        &initialize(&fixture, "taker", &agreement_a, &taker_sessions),
        "Taker session initialization",
    );
    assert_session_bundle(&maker_sessions);
    assert_session_bundle(&taker_sessions);
    let maker_claim = maker_sessions.join("claim.json");
    let maker_refund = maker_sessions.join("refund.json");
    let taker_claim = taker_sessions.join("claim.json");
    let taker_refund = taker_sessions.join("refund.json");
    assert_eq!(
        fs::read(&maker_claim).expect("Maker claim session"),
        fs::read(&taker_claim).expect("Taker claim session")
    );
    assert_eq!(
        fs::read(&maker_refund).expect("Maker refund session"),
        fs::read(&taker_refund).expect("Taker refund session")
    );
    assert_ne!(
        fs::read(&maker_claim).expect("Maker claim session"),
        fs::read(&maker_refund).expect("Maker refund session")
    );
    assert_no_staging_entries(&fixture.material);
    for path in [
        maker_signature_a,
        maker_signature_b,
        taker_signature,
        agreement_a,
        agreement_b,
    ] {
        assert_private_output(&path);
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn crosswired_private_bindings_signatures_and_destinations_fail_closed() {
    let fixture = provision_pair();
    let wrong_root_output = fixture.exchange.join("wrong-root.sig");
    let wrong_root = Command::new(binary())
        .args(["sign-stage-a", "maker", "--private-root"])
        .arg(&fixture.taker_root)
        .arg("--own-public-packet")
        .arg(&fixture.maker_packet)
        .arg("--peer-public-packet")
        .arg(&fixture.taker_packet)
        .arg("--unsigned-stage-a")
        .arg(&fixture.unsigned_stage_a)
        .arg("--output-signature")
        .arg(&wrong_root_output)
        .output()
        .expect("spawn wrong-root signer");
    assert!(!wrong_root.status.success());
    assert!(String::from_utf8_lossy(&wrong_root.stderr).contains("manifest role mismatch"));
    assert!(!wrong_root_output.exists());

    let packet_bytes = fs::read(&fixture.maker_packet).expect("Maker packet bytes");
    let packet: Value = serde_json::from_slice(&packet_bytes).expect("Maker packet JSON");
    let original_key = packet["agreement_public_key"]
        .as_str()
        .expect("agreement public key");
    let replacement_secret = SecretKey::from_slice(&[99; 32]).expect("replacement secret");
    let replacement_key =
        hex::encode(PublicKey::from_secret_key(&Secp256k1::new(), &replacement_secret).serialize());
    let needle = format!("\"agreement_public_key\":\"{original_key}\"");
    let replacement = format!("\"agreement_public_key\":\"{replacement_key}\"");
    let packet_text = std::str::from_utf8(&packet_bytes).expect("packet UTF-8");
    assert_eq!(packet_text.matches(&needle).count(), 1);
    let alternate_packet = fixture.exchange.join("alternate-maker.json");
    write_new_private(
        &alternate_packet,
        packet_text.replacen(&needle, &replacement, 1).as_bytes(),
    );
    let _ = ValidatedRolePacket::read(&alternate_packet).expect("valid alternate Maker packet");
    let digest_output = fixture.exchange.join("wrong-digest.sig");
    let wrong_digest = Command::new(binary())
        .args(["sign-stage-a", "maker", "--private-root"])
        .arg(&fixture.maker_root)
        .arg("--own-public-packet")
        .arg(&alternate_packet)
        .arg("--peer-public-packet")
        .arg(&fixture.taker_packet)
        .arg("--unsigned-stage-a")
        .arg(&fixture.unsigned_stage_a)
        .arg("--output-signature")
        .arg(&digest_output)
        .output()
        .expect("spawn wrong-digest signer");
    assert!(!wrong_digest.status.success());
    assert!(
        String::from_utf8_lossy(&wrong_digest.stderr).contains("public-packet digest mismatch")
    );
    assert!(!digest_output.exists());

    let (maker_signature, taker_signature, agreement) = signed_agreement(&fixture);
    let crossed_agreement = fixture.exchange.join("crossed-agreement.bin");
    let crossed = assemble(
        &fixture,
        &taker_signature,
        &maker_signature,
        &crossed_agreement,
    );
    assert!(!crossed.status.success());
    assert!(String::from_utf8_lossy(&crossed.stderr).contains("signatures are invalid"));
    assert!(!crossed_agreement.exists());

    let collision = fixture.exchange.join("collision.sig");
    write_new_private(&collision, b"untouched");
    let collided = sign(&fixture, "maker", &collision);
    assert!(!collided.status.success());
    assert!(String::from_utf8_lossy(&collided.stderr).contains("already exists"));
    assert_eq!(fs::read(&collision).expect("collision file"), b"untouched");

    let sessions = owner_directory(&fixture.material, "collision-sessions");
    let sentinel = sessions.join("untouched");
    write_new_private(&sentinel, b"untouched");
    let collided = initialize(&fixture, "maker", &agreement, &sessions);
    assert!(!collided.status.success());
    assert!(String::from_utf8_lossy(&collided.stderr).contains("already exists"));
    assert!(!sessions.join("claim.json").exists());
    assert!(!sessions.join("refund.json").exists());
    assert_eq!(
        fs::read(&sentinel).expect("session-root collision"),
        b"untouched"
    );
    assert_no_staging_entries(&fixture.material);

    fs::write(
        fixture.maker_root.join("claim.key"),
        format!("{}\n", "01".repeat(32)),
    )
    .expect("crosswire claim key");
    let unrelated_key_output = fixture.exchange.join("unrelated-key.sig");
    let rejected = sign(&fixture, "maker", &unrelated_key_output);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("signing keys do not match"));
    assert!(!unrelated_key_output.exists());
    let rejected_session_root = fixture.material.join("rejected-sessions");
    let rejected = initialize(&fixture, "maker", &agreement, &rejected_session_root);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("signing keys do not match"));
    assert!(!rejected_session_root.exists());
    assert_no_staging_entries(&fixture.material);
}

#[test]
#[allow(clippy::too_many_lines)]
fn completed_role_journals_activate_without_disclosing_taker_claim_partial() {
    let fixture = provision_pair();
    let (_, _, agreement) = signed_agreement(&fixture);
    let maker_sessions = fixture.material.join("stage-b-maker-sessions");
    let taker_sessions = fixture.material.join("stage-b-taker-sessions");
    assert_success(
        &initialize(&fixture, "maker", &agreement, &maker_sessions),
        "Maker Stage-B session initialization",
    );
    assert_success(
        &initialize(&fixture, "taker", &agreement, &taker_sessions),
        "Taker Stage-B session initialization",
    );

    let incomplete_journal = fixture.taker_root.join("incomplete.sqlite");
    let incomplete_commitment = fixture.taker_root.join("incomplete-commitment.json");
    runner(
        "taker",
        &incomplete_journal,
        &taker_sessions.join("claim.json"),
        vec![
            "reserve".into(),
            "--secret-key-file".into(),
            fixture.taker_root.join("claim.key").into_os_string(),
            "--output".into(),
            incomplete_commitment.into_os_string(),
        ],
    );
    let incomplete_output = fixture.exchange.join("incomplete-stage-b.bin");
    let incomplete = Command::new(binary())
        .arg("compose-stage-b")
        .arg("--private-root")
        .arg(&fixture.taker_root)
        .arg("--own-public-packet")
        .arg(&fixture.taker_packet)
        .arg("--peer-public-packet")
        .arg(&fixture.maker_packet)
        .arg("--agreement-stage-a")
        .arg(&agreement)
        .arg("--journal")
        .arg(&incomplete_journal)
        .arg("--output-unsigned-stage-b")
        .arg(&incomplete_output)
        .output()
        .expect("spawn incomplete Stage-B composer");
    assert!(!incomplete.status.success());
    assert!(String::from_utf8_lossy(&incomplete.stderr).contains("claim journal is incomplete"));
    assert!(!incomplete_output.exists());

    let maker_journal = fixture.maker_root.join("stage-b.sqlite");
    let taker_journal = fixture.taker_root.join("stage-b.sqlite");
    let taker_claim_partial = run_adaptor_round(
        &fixture,
        "claim",
        &maker_sessions.join("claim.json"),
        &taker_sessions.join("claim.json"),
        &maker_journal,
        &taker_journal,
    );
    let _taker_refund_partial = run_adaptor_round(
        &fixture,
        "refund",
        &maker_sessions.join("refund.json"),
        &taker_sessions.join("refund.json"),
        &maker_journal,
        &taker_journal,
    );

    let unsigned = fixture.exchange.join("unsigned-stage-b.bin");
    let compose = Command::new(binary())
        .arg("compose-stage-b")
        .arg("--private-root")
        .arg(&fixture.taker_root)
        .arg("--own-public-packet")
        .arg(&fixture.taker_packet)
        .arg("--peer-public-packet")
        .arg(&fixture.maker_packet)
        .arg("--agreement-stage-a")
        .arg(&agreement)
        .arg("--journal")
        .arg(&taker_journal)
        .arg("--output-unsigned-stage-b")
        .arg(&unsigned)
        .output()
        .expect("spawn Stage-B composer");
    assert_success(&compose, "Stage-B compose");
    assert_private_output(&unsigned);
    let unsigned_bytes = fs::read(&unsigned).expect("unsigned Stage-B wire");
    assert!(
        !unsigned_bytes
            .windows(taker_claim_partial.len())
            .any(|bytes| bytes == taker_claim_partial)
    );

    let maker_signature = fixture.exchange.join("maker-stage-b.sig");
    let taker_signature = fixture.exchange.join("taker-stage-b.sig");
    assert_success(
        &stage_b_sign(&fixture, "maker", &agreement, &unsigned, &maker_signature),
        "Maker Stage-B sign",
    );
    assert_success(
        &stage_b_sign(&fixture, "taker", &agreement, &unsigned, &taker_signature),
        "Taker Stage-B sign",
    );
    assert_private_output(&maker_signature);
    assert_private_output(&taker_signature);

    let activated = fixture.exchange.join("activated-stage-b.bin");
    let assemble = |maker: &Path, taker: &Path, output: &Path| {
        Command::new(binary())
            .args(["assemble-stage-b", "taker", "--private-root"])
            .arg(&fixture.taker_root)
            .arg("--own-public-packet")
            .arg(&fixture.taker_packet)
            .arg("--peer-public-packet")
            .arg(&fixture.maker_packet)
            .arg("--agreement-stage-a")
            .arg(&agreement)
            .arg("--unsigned-stage-b")
            .arg(&unsigned)
            .arg("--maker-signature")
            .arg(maker)
            .arg("--taker-signature")
            .arg(taker)
            .arg("--output-stage-b")
            .arg(output)
            .output()
            .expect("spawn Stage-B assembler")
    };
    assert_success(
        &assemble(&maker_signature, &taker_signature, &activated),
        "Stage-B assemble",
    );
    assert_private_output(&activated);
    let activated_bytes = fs::read(&activated).expect("activated Stage-B wire");
    assert!(
        !activated_bytes
            .windows(taker_claim_partial.len())
            .any(|bytes| bytes == taker_claim_partial)
    );
    let stage_a = XmrAgreementV1::from_wire(&fs::read(&agreement).expect("Stage-A wire"))
        .expect("validated Stage A");
    let view_bytes: [u8; 32] = hex::decode(
        std::str::from_utf8(
            &fs::read(fixture.taker_root.join("monero-view.key")).expect("view key"),
        )
        .expect("view key UTF-8")
        .trim(),
    )
    .expect("view key hex")
    .try_into()
    .expect("view key width");
    let view =
        MoneroPrivateViewKey::from_monero_little_endian(view_bytes).expect("private view key");
    let activated_agreement = XmrActivatedAgreementV1::from_wire(&stage_a, &activated_bytes, &view)
        .expect("validated activated agreement");
    let initial = activated_agreement
        .initial_coordinator(&stage_a)
        .expect("only countersigned Stage B mints the initial coordinator");
    let initial_json =
        serde_json::to_value(&initial).expect("serialize exact initial XMR coordinator");
    assert_eq!(
        initial_json,
        serde_json::json!({
            "id": hex::encode(stage_a.body().swap_id()),
            "pair": "Monero",
            "direction": "TakerSellsLez",
            "confirmation_policy": 2,
            "maker_confirmation_policy": 10,
            "recovery_schedule": {
                "maker_trigger": {
                    "CanonicalTakerRefund": {
                        "chain": "Lez",
                        "required_confirmations": 2
                    }
                },
                "taker_refund": {
                    "chain": "Lez",
                    "basis": "Timestamp",
                    "value": 20
                },
                "safety": null
            },
            "phase": "Offered",
            "taker_lock_transaction_id": null,
            "maker_lock_transaction_id": null,
            "claim_evidence": null,
            "revealing_claim_transaction_id": null,
            "followup_claim_transaction_id": null,
            "taker_refund_event_transaction_id": null,
            "maker_recovery_transaction_id": null
        }),
        "Stage B must derive the exact signed XMR application parameters"
    );

    let crossed_output = fixture.exchange.join("crossed-stage-b.bin");
    let crossed = assemble(&taker_signature, &maker_signature, &crossed_output);
    assert!(!crossed.status.success());
    assert!(String::from_utf8_lossy(&crossed.stderr).contains("role signatures are invalid"));
    assert!(!crossed_output.exists());

    let collision = fixture.exchange.join("stage-b-collision.bin");
    write_new_private(&collision, b"untouched");
    let collided = assemble(&maker_signature, &taker_signature, &collision);
    assert!(!collided.status.success());
    assert!(String::from_utf8_lossy(&collided.stderr).contains("already exists"));
    assert_eq!(fs::read(&collision).expect("collision file"), b"untouched");
}
