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

use lez_adaptor_role_runner::{
    Role, RunnerError, ValidatedSession, accept_published_peer_partial_and_adapt,
    verify_extracted_adaptor_secret, write_observed_final_signature_packet,
};
use lez_adaptor_signature::{
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
    adaptor_key: PathBuf,
    maker_journal: PathBuf,
    taker_journal: PathBuf,
    context: AdaptorSessionContext,
}

fn fixture(domain: Domain) -> Fixture {
    let directory = tempfile::tempdir().expect("temporary fixture");
    let session = directory.path().join("session.json");
    let maker_key = directory.path().join("maker.key");
    let taker_key = directory.path().join("taker.key");
    let adaptor_key = directory.path().join("adaptor.key");
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
    write_private(
        &adaptor_key,
        format!("{}\n", hex::encode(ADAPTOR_SECRET)).as_bytes(),
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
        adaptor_key,
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
    let adaptor = hex::encode(ADAPTOR_SECRET);
    let wrong_adaptor = hex::encode([0x54; 32]);
    for bytes in [&output.stdout, &output.stderr] {
        let text = String::from_utf8_lossy(bytes);
        assert!(!text.contains(&maker));
        assert!(!text.contains(&taker));
        assert!(!text.contains(&adaptor));
        assert!(!text.contains(&wrong_adaptor));
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
    let final_signature_packet = directory.join("final-signature.json");
    let bridged_final_signature_packet = directory.join("bridged-final-signature.json");
    let observed_final_signature_packet = directory.join("observed-final-signature.json");
    let final_signature_from_extracted = directory.join("final-signature-from-extracted.json");
    let extracted_adaptor_secret = directory.join("extracted-adaptor.key");

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

    if matches!(domain, Domain::Lez) {
        let session =
            ValidatedSession::from_untweaked_context(fixture.context.clone()).expect("session");
        let published_taker_partial = packet_payload(&taker_partial, "partial_signature", "taker");
        accept_published_peer_partial_and_adapt(
            &fixture.maker_journal,
            &session,
            Role::Maker,
            published_taker_partial,
            Zeroizing::new(ADAPTOR_SECRET),
            &bridged_final_signature_packet,
        )
        .expect("finalized peer partial completes the Maker claim");
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
    if matches!(domain, Domain::Lez) {
        let session =
            ValidatedSession::from_untweaked_context(fixture.context.clone()).expect("session");
        let bridged_signature: [u8; 64] = packet_payload(
            &bridged_final_signature_packet,
            "final_signature",
            "aggregate",
        );
        write_observed_final_signature_packet(
            &fixture.taker_journal,
            &session,
            Role::Taker,
            bridged_signature,
            &observed_final_signature_packet,
        )
        .expect("finalized aggregate signature is extraction-linked to Taker journal");
        assert_eq!(
            fs::read(&observed_final_signature_packet).expect("observed final packet"),
            fs::read(&bridged_final_signature_packet).expect("bridged final packet")
        );

        let mut invalid_signature = bridged_signature;
        invalid_signature[63] ^= 1;
        let invalid_observed = directory.join("invalid-observed-final-signature.json");
        assert!(
            write_observed_final_signature_packet(
                &fixture.taker_journal,
                &session,
                Role::Taker,
                invalid_signature,
                &invalid_observed,
            )
            .is_err()
        );
        assert!(!invalid_observed.exists());
    }

    if crosswire_checks {
        let wrong_adaptor_key = directory.join("wrong-adaptor.key");
        write_private(
            &wrong_adaptor_key,
            format!("{}\n", hex::encode([0x54; 32])).as_bytes(),
        );
        let wrong_secret_output = directory.join("wrong-secret-final.json");
        let mut wrong_secret = command("maker", &fixture.maker_journal, &fixture.session);
        wrong_secret
            .arg("adapt-presignature")
            .arg("--input")
            .arg(&maker_presignature)
            .arg("--adaptor-secret-file")
            .arg(&wrong_adaptor_key)
            .arg("--output")
            .arg(&wrong_secret_output);
        let failed = run_fail(wrong_secret);
        assert!(String::from_utf8_lossy(&failed.stderr).contains("cryptographic transcript"));
        assert!(!wrong_secret_output.exists());

        fs::set_permissions(&fixture.adaptor_key, fs::Permissions::from_mode(0o644))
            .expect("make adaptor scalar unsafe");
        let unsafe_secret_output = directory.join("unsafe-secret-final.json");
        let mut unsafe_secret = command("maker", &fixture.maker_journal, &fixture.session);
        unsafe_secret
            .arg("adapt-presignature")
            .arg("--input")
            .arg(&maker_presignature)
            .arg("--adaptor-secret-file")
            .arg(&fixture.adaptor_key)
            .arg("--output")
            .arg(&unsafe_secret_output);
        let failed = run_fail(unsafe_secret);
        assert!(String::from_utf8_lossy(&failed.stderr).contains("owner-private"));
        assert!(!unsafe_secret_output.exists());
        fs::set_permissions(&fixture.adaptor_key, fs::Permissions::from_mode(0o600))
            .expect("restore adaptor scalar permissions");

        let packet_bytes = fs::read(&maker_presignature).expect("read aggregate packet");
        let packet_value: Value =
            serde_json::from_slice(&packet_bytes).expect("aggregate packet JSON");
        let original_session_id = packet_value["session_id"]
            .as_str()
            .expect("packet session id");
        let crosswired_presignature = directory.join("crosswired-presignature.json");
        let packet_text = String::from_utf8(packet_bytes).expect("UTF-8 aggregate packet");
        let crosswired_text =
            packet_text.replacen(original_session_id, &hex::encode([0xff; 32]), 1);
        fs::write(&crosswired_presignature, crosswired_text).expect("write crosswire packet");
        let mut crosswired = command("maker", &fixture.maker_journal, &fixture.session);
        crosswired
            .arg("adapt-presignature")
            .arg("--input")
            .arg(&crosswired_presignature)
            .arg("--adaptor-secret-file")
            .arg(&fixture.adaptor_key)
            .arg("--output")
            .arg(directory.join("crosswired-final.json"));
        let failed = run_fail(crosswired);
        assert!(String::from_utf8_lossy(&failed.stderr).contains("another session"));

        let noncanonical_presignature = directory.join("noncanonical-presignature.json");
        fs::write(
            &noncanonical_presignature,
            serde_json::to_vec_pretty(&packet_value).expect("pretty aggregate packet"),
        )
        .expect("write noncanonical packet");
        let mut noncanonical = command("maker", &fixture.maker_journal, &fixture.session);
        noncanonical
            .arg("adapt-presignature")
            .arg("--input")
            .arg(&noncanonical_presignature)
            .arg("--adaptor-secret-file")
            .arg(&fixture.adaptor_key)
            .arg("--output")
            .arg(directory.join("noncanonical-final.json"));
        let failed = run_fail(noncanonical);
        assert!(String::from_utf8_lossy(&failed.stderr).contains("not canonical"));

        let session_bytes = fs::read(&fixture.session).expect("read canonical session");
        let session_value: Value = serde_json::from_slice(&session_bytes).expect("session JSON");
        let original_message = session_value["exact_message"]
            .as_str()
            .expect("session message");
        let wrong_message_session = directory.join("wrong-message-session.json");
        let session_text = String::from_utf8(session_bytes).expect("UTF-8 session");
        let wrong_message_text =
            session_text.replacen(original_message, &hex::encode([0x93; 32]), 1);
        fs::write(&wrong_message_session, wrong_message_text).expect("write wrong-message session");
        let mut wrong_message = command("maker", &fixture.maker_journal, &wrong_message_session);
        wrong_message
            .arg("adapt-presignature")
            .arg("--input")
            .arg(&maker_presignature)
            .arg("--adaptor-secret-file")
            .arg(&fixture.adaptor_key)
            .arg("--output")
            .arg(directory.join("wrong-message-final.json"));
        let failed = run_fail(wrong_message);
        assert!(String::from_utf8_lossy(&failed.stderr).contains("another session"));
    }

    let mut adapt = command("maker", &fixture.maker_journal, &fixture.session);
    adapt
        .arg("adapt-presignature")
        .arg("--input")
        .arg(&maker_presignature)
        .arg("--adaptor-secret-file")
        .arg(&fixture.adaptor_key)
        .arg("--output")
        .arg(&final_signature_packet);
    run_ok(adapt);
    let final_signature: [u8; 64] =
        packet_payload(&final_signature_packet, "final_signature", "aggregate");
    verify_final_signature(&fixture.context, final_signature).expect("final signature verifies");
    if matches!(domain, Domain::Lez) {
        assert_eq!(
            fs::read(&final_signature_packet).expect("manual final packet"),
            fs::read(&bridged_final_signature_packet).expect("bridged final packet")
        );
    }

    if crosswire_checks {
        let final_bytes = fs::read(&final_signature_packet).expect("read final packet");
        let final_value: Value = serde_json::from_slice(&final_bytes).expect("final packet JSON");
        let final_payload = final_value["payload"]
            .as_str()
            .expect("final packet payload");
        let replacement = if final_payload.starts_with("00") {
            "01"
        } else {
            "00"
        };
        let mut invalid_payload = final_payload.to_owned();
        invalid_payload.replace_range(..2, replacement);
        let final_text = String::from_utf8(final_bytes).expect("UTF-8 final packet");
        let invalid_final_packet = directory.join("invalid-final-signature.json");
        fs::write(
            &invalid_final_packet,
            final_text.replacen(final_payload, &invalid_payload, 1),
        )
        .expect("write invalid final packet");
        let invalid_extraction = directory.join("invalid-extracted.key");
        let mut extract_invalid = command("taker", &fixture.taker_journal, &fixture.session);
        extract_invalid
            .arg("extract-adaptor-secret")
            .arg("--presignature")
            .arg(&taker_presignature)
            .arg("--final-signature")
            .arg(&invalid_final_packet)
            .arg("--output")
            .arg(&invalid_extraction);
        let failed = run_fail(extract_invalid);
        assert!(String::from_utf8_lossy(&failed.stderr).contains("cryptographic transcript"));
        assert!(!invalid_extraction.exists());
    }

    let final_before_replay = fs::read(&final_signature_packet).expect("read final signature");
    let mut replay_adapt = command("maker", &fixture.maker_journal, &fixture.session);
    replay_adapt
        .arg("adapt-presignature")
        .arg("--input")
        .arg(&maker_presignature)
        .arg("--adaptor-secret-file")
        .arg(&fixture.adaptor_key)
        .arg("--output")
        .arg(&final_signature_packet);
    run_fail(replay_adapt);
    assert_eq!(
        fs::read(&final_signature_packet).expect("read final after rejected replay"),
        final_before_replay
    );

    let mut extract = command("taker", &fixture.taker_journal, &fixture.session);
    extract
        .arg("extract-adaptor-secret")
        .arg("--presignature")
        .arg(&taker_presignature)
        .arg("--final-signature")
        .arg(&final_signature_packet)
        .arg("--output")
        .arg(&extracted_adaptor_secret);
    run_ok(extract);
    assert_eq!(
        fs::read(&extracted_adaptor_secret).expect("read extracted scalar"),
        format!("{}\n", hex::encode(ADAPTOR_SECRET)).as_bytes()
    );
    if matches!(domain, Domain::Lez) {
        let session =
            ValidatedSession::from_untweaked_context(fixture.context.clone()).expect("session");
        let verified = verify_extracted_adaptor_secret(
            &fixture.taker_journal,
            &session,
            Role::Taker,
            final_signature,
            &extracted_adaptor_secret,
        )
        .expect("exact extracted scalar verifies against durable transcript");
        let redacted = format!("{verified:?}");
        assert_eq!(redacted, "VerifiedAdaptorSecret(REDACTED)");
        assert!(!redacted.contains(&hex::encode(ADAPTOR_SECRET)));
        assert_eq!(*verified.into_big_endian_bytes(), ADAPTOR_SECRET);

        let mut wrong_signature = final_signature;
        wrong_signature[63] ^= 1;
        assert!(matches!(
            verify_extracted_adaptor_secret(
                &fixture.taker_journal,
                &session,
                Role::Taker,
                wrong_signature,
                &extracted_adaptor_secret,
            ),
            Err(RunnerError::CryptographicValidation)
        ));

        let wrong_scalar = directory.join("wrong-extracted-adaptor.key");
        write_private(
            &wrong_scalar,
            format!("{}\n", hex::encode([0x54; 32])).as_bytes(),
        );
        assert!(matches!(
            verify_extracted_adaptor_secret(
                &fixture.taker_journal,
                &session,
                Role::Taker,
                final_signature,
                &wrong_scalar,
            ),
            Err(RunnerError::CryptographicValidation)
        ));

        let wrong_context = AdaptorSessionContext::untweaked(
            fixture.context.ordered_public_keys(),
            fixture.context.message(),
            fixture.context.adaptor_point(),
            [0xfe; 32],
        )
        .expect("alternate session context");
        let wrong_session =
            ValidatedSession::from_untweaked_context(wrong_context).expect("alternate session");
        assert!(matches!(
            verify_extracted_adaptor_secret(
                &fixture.taker_journal,
                &wrong_session,
                Role::Taker,
                final_signature,
                &extracted_adaptor_secret,
            ),
            Err(RunnerError::SessionUnavailable)
        ));
        assert!(matches!(
            verify_extracted_adaptor_secret(
                &fixture.taker_journal,
                &session,
                Role::Maker,
                final_signature,
                &extracted_adaptor_secret,
            ),
            Err(RunnerError::JournalRoleOrSessionCrosswire)
        ));
    }

    let extracted_before_replay =
        fs::read(&extracted_adaptor_secret).expect("read extracted scalar before replay");
    let mut replay_extract = command("taker", &fixture.taker_journal, &fixture.session);
    replay_extract
        .arg("extract-adaptor-secret")
        .arg("--presignature")
        .arg(&taker_presignature)
        .arg("--final-signature")
        .arg(&final_signature_packet)
        .arg("--output")
        .arg(&extracted_adaptor_secret);
    run_fail(replay_extract);
    assert_eq!(
        fs::read(&extracted_adaptor_secret).expect("read extracted scalar after replay"),
        extracted_before_replay
    );

    let mut adapt_extracted = command("maker", &fixture.maker_journal, &fixture.session);
    adapt_extracted
        .arg("adapt-presignature")
        .arg("--input")
        .arg(&maker_presignature)
        .arg("--adaptor-secret-file")
        .arg(&extracted_adaptor_secret)
        .arg("--output")
        .arg(&final_signature_from_extracted);
    run_ok(adapt_extracted);
    assert_eq!(
        fs::read(&final_signature_from_extracted).expect("read re-adapted final packet"),
        fs::read(&final_signature_packet).expect("read original final packet")
    );

    let sdk_final = adapt_presignature(
        &fixture.context,
        presignature,
        Zeroizing::new(ADAPTOR_SECRET),
    )
    .expect("adapt process presignature independently");
    assert_eq!(final_signature, sdk_final);

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
        &final_signature_packet,
        &final_signature_from_extracted,
        &extracted_adaptor_secret,
    ] {
        let mode = fs::metadata(path)
            .expect("output metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "{} must be owner-private", path.display());
    }
    if matches!(domain, Domain::Lez) {
        for path in [
            &bridged_final_signature_packet,
            &observed_final_signature_packet,
        ] {
            let mode = fs::metadata(path)
                .expect("bridge output metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "{} must be owner-private", path.display());
            let text = fs::read_to_string(path).expect("bridge public packet text");
            assert!(!text.contains(&hex::encode(MAKER_SECRET)));
            assert!(!text.contains(&hex::encode(TAKER_SECRET)));
            assert!(!text.contains(&hex::encode(ADAPTOR_SECRET)));
        }
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
        final_signature_packet,
        final_signature_from_extracted,
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
