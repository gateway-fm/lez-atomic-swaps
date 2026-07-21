use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    process::{Command, Output},
};

use lez_xmr_swap_sdk::{
    MoneroAddressNetworkV1, MoneroSharedAddressV1, ValidatedXmrAgreementBodyV1, XmrAgreementBodyV1,
    XmrAgreementV1, XmrLezTermsV1, XmrMessagesV1, XmrMoneroTermsV1, XmrNamedProfileV1,
    XmrParticipantsV1, XmrSwapDirectionV1, XmrWindowsV1,
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
