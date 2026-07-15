#![allow(
    clippy::similar_names,
    reason = "maker/taker variables intentionally mirror the two independent role processes"
)]
#![allow(
    clippy::too_many_lines,
    reason = "the process journey stays linear so restart and packet ordering remain auditable"
)]

use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    process::{Command, Output},
};

use lez_btc_swap_sdk::{
    AdaptorSessionContext, adapt_presignature, aggregate_adaptor_presignature,
    verify_final_signature,
};
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use serde_json::{Value, json};
use tempfile::TempDir;
use zeroize::Zeroizing;

const MAKER_SECRET: [u8; 32] = [0x31; 32];
const TAKER_SECRET: [u8; 32] = [0x42; 32];
const ADAPTOR_SECRET: [u8; 32] = [0x53; 32];

#[derive(Clone, Copy)]
enum Domain {
    Lez,
    Bitcoin,
}

struct Fixture {
    _directory: TempDir,
    session: PathBuf,
    maker_key: PathBuf,
    taker_key: PathBuf,
    maker_journal: PathBuf,
    taker_journal: PathBuf,
    context: AdaptorSessionContext,
}

fn fixture(domain: Domain) -> Fixture {
    let directory = tempfile::tempdir().expect("temporary fixture");
    let session = directory.path().join("session.json");
    let maker_key = directory.path().join("maker.key");
    let taker_key = directory.path().join("taker.key");
    let maker_journal = directory.path().join("maker.sqlite");
    let taker_journal = directory.path().join("taker.sqlite");

    write_private(
        &maker_key,
        format!("{}\n", hex::encode(MAKER_SECRET)).as_bytes(),
    );
    write_private(
        &taker_key,
        format!("{}\n", hex::encode(TAKER_SECRET)).as_bytes(),
    );

    let secp = Secp256k1::signing_only();
    let maker_public = PublicKey::from_secret_key(
        &secp,
        &SecretKey::from_slice(&MAKER_SECRET).expect("maker key"),
    )
    .serialize();
    let taker_public = PublicKey::from_secret_key(
        &secp,
        &SecretKey::from_slice(&TAKER_SECRET).expect("taker key"),
    )
    .serialize();
    let adaptor_point = PublicKey::from_secret_key(
        &secp,
        &SecretKey::from_slice(&ADAPTOR_SECRET).expect("adaptor key"),
    )
    .serialize();
    let exact_message = match domain {
        Domain::Lez => [0x91; 32],
        Domain::Bitcoin => [0x92; 32],
    };
    let session_id = match domain {
        Domain::Lez => [0xa1; 32],
        Domain::Bitcoin => [0xa2; 32],
    };
    let (context_json, context) = match domain {
        Domain::Lez => (
            json!({"kind": "lez_untweaked"}),
            AdaptorSessionContext::untweaked(
                [maker_public, taker_public],
                exact_message,
                adaptor_point,
                session_id,
            )
            .expect("LEZ context"),
        ),
        Domain::Bitcoin => {
            let merkle_root = [0xb1; 32];
            (
                json!({
                    "kind": "btc_taproot",
                    "merkle_root": hex::encode(merkle_root),
                }),
                AdaptorSessionContext::taproot(
                    [maker_public, taker_public],
                    merkle_root,
                    exact_message,
                    adaptor_point,
                    session_id,
                )
                .expect("Bitcoin context"),
            )
        }
    };
    let context_json = serde_json::to_string(&context_json).expect("context JSON");
    let encoded = format!(
        "{{\"schema_version\":1,\"context\":{context_json},\"session_id\":\"{}\",\"exact_message\":\"{}\",\"adaptor_point\":\"{}\",\"maker_public_key\":\"{}\",\"taker_public_key\":\"{}\"}}\n",
        hex::encode(session_id),
        hex::encode(exact_message),
        hex::encode(adaptor_point),
        hex::encode(maker_public),
        hex::encode(taker_public),
    );
    fs::write(&session, encoded).expect("write canonical public session");
    Fixture {
        _directory: directory,
        session,
        maker_key,
        taker_key,
        maker_journal,
        taker_journal,
        context,
    }
}

fn write_private(path: &Path, bytes: &[u8]) {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    let mut file = options.open(path).expect("create private file");
    file.write_all(bytes).expect("write private file");
    file.sync_all().expect("sync private file");
}

fn command(role: &str, journal: &Path, session: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lez-adaptor-role-runner"));
    command
        .arg(role)
        .arg("--journal")
        .arg(journal)
        .arg("--session")
        .arg(session);
    command
}

fn run_ok(mut command: Command) -> Output {
    let output = command.output().expect("run role process");
    assert_secret_free(&output);
    assert!(
        output.status.success(),
        "process failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty(), "success must emit no stdout");
    assert!(output.stderr.is_empty(), "success must emit no stderr");
    output
}

fn run_fail(mut command: Command) -> Output {
    let output = command.output().expect("run failing role process");
    assert_secret_free(&output);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty(), "failure must emit no stdout");
    output
}

fn assert_secret_free(output: &Output) {
    let maker = hex::encode(MAKER_SECRET);
    let taker = hex::encode(TAKER_SECRET);
    for bytes in [&output.stdout, &output.stderr] {
        let text = String::from_utf8_lossy(bytes);
        assert!(!text.contains(&maker));
        assert!(!text.contains(&taker));
    }
}

fn packet_payload<const N: usize>(path: &Path, kind: &str, role: &str) -> [u8; N] {
    let bytes = fs::read(path).expect("read packet");
    assert_eq!(bytes.last(), Some(&b'\n'));
    let value: Value = serde_json::from_slice(&bytes).expect("packet JSON");
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["kind"], kind);
    assert_eq!(value["sender_role"], role);
    let payload = value["payload"].as_str().expect("packet payload");
    assert_eq!(payload.len(), N * 2);
    assert_eq!(payload, payload.to_ascii_lowercase());
    hex::decode(payload)
        .expect("packet hex")
        .try_into()
        .expect("fixed packet width")
}

fn exercise(domain: Domain, crosswire_checks: bool) {
    let fixture = fixture(domain);
    let directory = fixture.session.parent().expect("fixture directory");
    let maker_commitment = directory.join("maker-commitment.json");
    let taker_commitment = directory.join("taker-commitment.json");
    let maker_commitment_replay = directory.join("maker-commitment-replay.json");
    let maker_nonce = directory.join("maker-nonce.json");
    let taker_nonce = directory.join("taker-nonce.json");
    let maker_partial = directory.join("maker-partial.json");
    let taker_partial = directory.join("taker-partial.json");
    let maker_partial_replay = directory.join("maker-partial-replay.json");
    let taker_partial_replay = directory.join("taker-partial-replay.json");
    let maker_presignature = directory.join("maker-presignature.json");
    let taker_presignature = directory.join("taker-presignature.json");
    let maker_presignature_replay = directory.join("maker-presignature-replay.json");

    let mut reserve_maker = command("maker", &fixture.maker_journal, &fixture.session);
    reserve_maker
        .arg("reserve")
        .arg("--secret-key-file")
        .arg(&fixture.maker_key)
        .arg("--output")
        .arg(&maker_commitment);
    run_ok(reserve_maker);
    let mut reserve_taker = command("taker", &fixture.taker_journal, &fixture.session);
    reserve_taker
        .arg("reserve")
        .arg("--secret-key-file")
        .arg(&fixture.taker_key)
        .arg("--output")
        .arg(&taker_commitment);
    run_ok(reserve_taker);

    let mut replay_reserve = command("maker", &fixture.maker_journal, &fixture.session);
    replay_reserve
        .arg("reserve")
        .arg("--secret-key-file")
        .arg(directory.join("deliberately-absent.key"))
        .arg("--output")
        .arg(&maker_commitment_replay);
    run_ok(replay_reserve);
    assert_eq!(
        fs::read(&maker_commitment).unwrap(),
        fs::read(&maker_commitment_replay).unwrap()
    );

    if crosswire_checks {
        let mut own_packet = command("maker", &fixture.maker_journal, &fixture.session);
        own_packet
            .arg("accept-commitment")
            .arg("--input")
            .arg(&maker_commitment);
        let output = run_fail(own_packet);
        assert!(String::from_utf8_lossy(&output.stderr).contains("sender role"));

        let mut wrong_journal = command("taker", &fixture.maker_journal, &fixture.session);
        wrong_journal
            .arg("accept-commitment")
            .arg("--input")
            .arg(&maker_commitment);
        let output = run_fail(wrong_journal);
        assert!(String::from_utf8_lossy(&output.stderr).contains("another role"));

        let premature_nonce = directory.join("premature-nonce.json");
        let mut premature = command("maker", &fixture.maker_journal, &fixture.session);
        premature
            .arg("reveal-nonce")
            .arg("--output")
            .arg(&premature_nonce);
        run_fail(premature);
        assert!(!premature_nonce.exists());
    }

    let mut maker_accept = command("maker", &fixture.maker_journal, &fixture.session);
    maker_accept
        .arg("accept-commitment")
        .arg("--input")
        .arg(&taker_commitment);
    run_ok(maker_accept);
    let mut taker_accept = command("taker", &fixture.taker_journal, &fixture.session);
    taker_accept
        .arg("accept-commitment")
        .arg("--input")
        .arg(&maker_commitment);
    run_ok(taker_accept);

    let mut reveal_maker = command("maker", &fixture.maker_journal, &fixture.session);
    reveal_maker
        .arg("reveal-nonce")
        .arg("--output")
        .arg(&maker_nonce);
    run_ok(reveal_maker);
    let mut reveal_taker = command("taker", &fixture.taker_journal, &fixture.session);
    reveal_taker
        .arg("reveal-nonce")
        .arg("--output")
        .arg(&taker_nonce);
    run_ok(reveal_taker);

    let mut sign_maker = command("maker", &fixture.maker_journal, &fixture.session);
    sign_maker
        .arg("accept-nonce-sign")
        .arg("--input")
        .arg(&taker_nonce)
        .arg("--secret-key-file")
        .arg(&fixture.maker_key)
        .arg("--output")
        .arg(&maker_partial);
    run_ok(sign_maker);
    let mut sign_taker = command("taker", &fixture.taker_journal, &fixture.session);
    sign_taker
        .arg("accept-nonce-sign")
        .arg("--input")
        .arg(&maker_nonce)
        .arg("--secret-key-file")
        .arg(&fixture.taker_key)
        .arg("--output")
        .arg(&taker_partial);
    run_ok(sign_taker);

    let mut replay_maker = command("maker", &fixture.maker_journal, &fixture.session);
    replay_maker
        .arg("replay-partial")
        .arg("--output")
        .arg(&maker_partial_replay);
    run_ok(replay_maker);
    let mut replay_taker = command("taker", &fixture.taker_journal, &fixture.session);
    replay_taker
        .arg("replay-partial")
        .arg("--output")
        .arg(&taker_partial_replay);
    run_ok(replay_taker);
    assert_eq!(
        fs::read(&maker_partial).unwrap(),
        fs::read(&maker_partial_replay).unwrap()
    );
    assert_eq!(
        fs::read(&taker_partial).unwrap(),
        fs::read(&taker_partial_replay).unwrap()
    );

    if crosswire_checks {
        let mut own_partial = command("maker", &fixture.maker_journal, &fixture.session);
        own_partial
            .arg("accept-peer-partial")
            .arg("--input")
            .arg(&maker_partial)
            .arg("--output")
            .arg(directory.join("crosswired-presignature.json"));
        let output = run_fail(own_partial);
        assert!(String::from_utf8_lossy(&output.stderr).contains("sender role"));
    }

    let mut aggregate_maker = command("maker", &fixture.maker_journal, &fixture.session);
    aggregate_maker
        .arg("accept-peer-partial")
        .arg("--input")
        .arg(&taker_partial)
        .arg("--output")
        .arg(&maker_presignature);
    run_ok(aggregate_maker);
    let mut aggregate_taker = command("taker", &fixture.taker_journal, &fixture.session);
    aggregate_taker
        .arg("accept-peer-partial")
        .arg("--input")
        .arg(&maker_partial)
        .arg("--output")
        .arg(&taker_presignature);
    run_ok(aggregate_taker);
    assert_eq!(
        fs::read(&maker_presignature).unwrap(),
        fs::read(&taker_presignature).unwrap()
    );
    let mut aggregate_maker_replay = command("maker", &fixture.maker_journal, &fixture.session);
    aggregate_maker_replay
        .arg("accept-peer-partial")
        .arg("--input")
        .arg(&taker_partial)
        .arg("--output")
        .arg(&maker_presignature_replay);
    run_ok(aggregate_maker_replay);
    assert_eq!(
        fs::read(&maker_presignature).unwrap(),
        fs::read(&maker_presignature_replay).unwrap()
    );

    let maker_nonce_bytes = packet_payload(&maker_nonce, "public_nonce", "maker");
    let taker_nonce_bytes = packet_payload(&taker_nonce, "public_nonce", "taker");
    let maker_partial_bytes = packet_payload(&maker_partial, "partial_signature", "maker");
    let taker_partial_bytes = packet_payload(&taker_partial, "partial_signature", "taker");
    let process_presignature: [u8; 65] =
        packet_payload(&maker_presignature, "presignature", "aggregate");
    let presignature = aggregate_adaptor_presignature(
        &fixture.context,
        maker_nonce_bytes,
        taker_nonce_bytes,
        maker_partial_bytes,
        taker_partial_bytes,
    )
    .expect("independent process partials aggregate");
    assert_eq!(process_presignature, presignature);
    let final_signature = adapt_presignature(
        &fixture.context,
        presignature,
        Zeroizing::new(ADAPTOR_SECRET),
    )
    .expect("adapt process presignature");
    verify_final_signature(&fixture.context, final_signature).expect("final signature verifies");

    for path in [
        &fixture.maker_journal,
        &fixture.taker_journal,
        &maker_commitment,
        &taker_commitment,
        &maker_nonce,
        &taker_nonce,
        &maker_partial,
        &taker_partial,
        &maker_presignature,
        &taker_presignature,
    ] {
        let mode = fs::metadata(path)
            .expect("output metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "{} must be owner-private", path.display());
    }
    for path in [
        maker_commitment,
        taker_commitment,
        maker_nonce,
        taker_nonce,
        maker_partial,
        taker_partial,
        maker_presignature,
        taker_presignature,
    ] {
        let text = fs::read_to_string(path).expect("public packet text");
        assert!(!text.contains(&hex::encode(MAKER_SECRET)));
        assert!(!text.contains(&hex::encode(TAKER_SECRET)));
    }
}

#[test]
fn noncanonical_session_json_is_rejected_before_journal_mutation() {
    let fixture = fixture(Domain::Lez);
    let value: Value =
        serde_json::from_slice(&fs::read(&fixture.session).unwrap()).expect("session value");
    fs::write(
        &fixture.session,
        serde_json::to_vec_pretty(&value).expect("pretty session"),
    )
    .expect("replace session with noncanonical JSON");
    let output = fixture.session.parent().unwrap().join("commitment.json");
    let mut reserve = command("maker", &fixture.maker_journal, &fixture.session);
    reserve
        .arg("reserve")
        .arg("--secret-key-file")
        .arg(&fixture.maker_key)
        .arg("--output")
        .arg(&output);
    let failed = run_fail(reserve);
    assert!(String::from_utf8_lossy(&failed.stderr).contains("not canonical"));
    assert!(!fixture.maker_journal.exists());
    assert!(!output.exists());
}

#[test]
fn lez_roles_restart_between_phases_reject_crosswire_and_replay_exact_partial() {
    exercise(Domain::Lez, true);
}

#[test]
fn bitcoin_taproot_roles_restart_and_produce_a_valid_adaptor_presignature() {
    exercise(Domain::Bitcoin, false);
}

#[test]
fn secret_key_must_be_owner_private_and_cannot_be_passed_inline() {
    let fixture = fixture(Domain::Lez);
    fs::set_permissions(&fixture.maker_key, fs::Permissions::from_mode(0o644))
        .expect("make key unsafe");
    let output = fixture.session.parent().unwrap().join("commitment.json");
    let mut reserve = command("maker", &fixture.maker_journal, &fixture.session);
    reserve
        .arg("reserve")
        .arg("--secret-key-file")
        .arg(&fixture.maker_key)
        .arg("--output")
        .arg(&output);
    let failed = run_fail(reserve);
    assert!(String::from_utf8_lossy(&failed.stderr).contains("owner-private"));
    assert!(!output.exists());

    let help = Command::new(env!("CARGO_BIN_EXE_lez-adaptor-role-runner"))
        .arg("maker")
        .arg("--help")
        .output()
        .expect("runner help");
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).expect("UTF-8 help");
    assert!(!help.contains("--secret-key "));
    assert!(!help.contains("--secret-nonce"));
}
