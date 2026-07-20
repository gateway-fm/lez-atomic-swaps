use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    os::unix::fs::OpenOptionsExt as _,
    path::Path,
    process::Command,
};

use lez_adaptor_role_runner::{Role, ValidatedSession};
use lez_adaptor_signature::AdaptorSessionContext;
use lez_swap_store::{AdaptorSessionIdentity, AdaptorSessionRole};
use secp256k1::{PublicKey, Secp256k1, SecretKey};

const MAKER_SECRET: [u8; 32] = [0x31; 32];
const TAKER_SECRET: [u8; 32] = [0x42; 32];
const ADAPTOR_SECRET: [u8; 32] = [0x53; 32];

fn public_key(secret: [u8; 32]) -> [u8; 33] {
    PublicKey::from_secret_key(
        &Secp256k1::signing_only(),
        &SecretKey::from_slice(&secret).expect("valid fixture key"),
    )
    .serialize()
}

fn untweaked_context() -> AdaptorSessionContext {
    AdaptorSessionContext::untweaked(
        [public_key(MAKER_SECRET), public_key(TAKER_SECRET)],
        [0x91; 32],
        public_key(ADAPTOR_SECRET),
        [0xa1; 32],
    )
    .expect("valid untweaked context")
}

fn write_private(path: &Path, bytes: &[u8]) {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    let mut file = options.open(path).expect("create owner-private file");
    file.write_all(bytes).expect("write owner-private file");
    file.sync_all().expect("sync owner-private file");
}

#[test]
fn identities_exactly_bind_the_public_transcript_and_local_role() {
    let context = untweaked_context();
    let session = ValidatedSession::from_untweaked_context(context.clone())
        .expect("validated untweaked session");
    let identities = [
        (Role::Maker, AdaptorSessionRole::Maker),
        (Role::Taker, AdaptorSessionRole::Taker),
    ]
    .map(|(runner_role, store_role)| {
        let actual = session.identity(runner_role);
        let expected = AdaptorSessionIdentity::new(
            context.session_id(),
            store_role,
            context.durable_context_binding(),
            context.message(),
            context.adaptor_point(),
            context.ordered_public_keys(),
        );
        assert_eq!(actual, expected);
        assert_eq!(actual.local_role(), store_role);
        actual
    });

    assert_ne!(identities[0], identities[1]);
    assert_eq!(identities[0].session_id(), identities[1].session_id());
    assert_eq!(
        identities[0].signing_domain(),
        identities[1].signing_domain()
    );
    assert_eq!(identities[0].exact_message(), identities[1].exact_message());
    assert_eq!(identities[0].adaptor_point(), identities[1].adaptor_point());
    assert_eq!(
        identities[0].ordered_public_keys(),
        identities[1].ordered_public_keys()
    );
}

#[test]
fn writer_emits_canonical_session_that_a_fresh_role_process_reloads() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let session_path = directory.path().join("session.json");
    ValidatedSession::from_untweaked_context(untweaked_context())
        .expect("validated session")
        .write_new(&session_path)
        .expect("write new canonical session");

    let encoded = fs::read(&session_path).expect("read session");
    assert_eq!(encoded.last(), Some(&b'\n'));
    assert!(!encoded[..encoded.len() - 1].contains(&b'\n'));

    let maker_key = directory.path().join("maker.key");
    write_private(
        &maker_key,
        format!("{}\n", hex::encode(MAKER_SECRET)).as_bytes(),
    );
    let output = Command::new(env!("CARGO_BIN_EXE_lez-adaptor-role-runner"))
        .arg("maker")
        .arg("--journal")
        .arg(directory.path().join("maker.sqlite"))
        .arg("--session")
        .arg(&session_path)
        .arg("reserve")
        .arg("--secret-key-file")
        .arg(&maker_key)
        .arg("--output")
        .arg(directory.path().join("maker-commitment.json"))
        .output()
        .expect("run fresh role process");
    assert!(
        output.status.success(),
        "writer output must reload: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn untweaked_constructor_rejects_a_taproot_context() {
    let context = AdaptorSessionContext::taproot(
        [public_key(MAKER_SECRET), public_key(TAKER_SECRET)],
        [0xb1; 32],
        [0x91; 32],
        public_key(ADAPTOR_SECRET),
        [0xa1; 32],
    )
    .expect("valid Taproot context");

    let error = ValidatedSession::from_untweaked_context(context)
        .expect_err("Taproot context must fail closed at the untweaked boundary");
    assert_eq!(error.to_string(), "session configuration is invalid");
}

#[test]
fn writer_does_not_clobber_an_existing_path() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let session_path = directory.path().join("session.json");
    fs::write(&session_path, b"caller-owned\n").expect("seed existing file");

    let before = fs::read(&session_path).expect("read existing file");
    let error = ValidatedSession::from_untweaked_context(untweaked_context())
        .expect("validated session")
        .write_new(&session_path)
        .expect_err("writer must use create-new semantics");
    assert_eq!(error.to_string(), "output file I/O failed");
    assert_eq!(
        fs::read(&session_path).expect("read preserved file"),
        before
    );
}
