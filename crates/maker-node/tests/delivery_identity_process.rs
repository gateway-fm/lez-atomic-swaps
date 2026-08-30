use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _, symlink},
    path::{Path, PathBuf},
    process::{Command, Output},
};

use secp256k1::{PublicKey, Secp256k1, SecretKey};
use serde_json::{Value, json};
use tempfile::tempdir;

#[test]
fn raw_and_hex_keys_emit_the_same_public_only_identity_without_rpc() {
    let run = private_tempdir();
    let raw_key = private_file(run.path(), "delivery.raw", &[8_u8; 32], 0o600);
    let mut encoded = hex::encode([8_u8; 32]).into_bytes();
    encoded.push(b'\n');
    let hex_key = private_file(run.path(), "delivery.hex", &encoded, 0o400);
    let missing_socket = run.path().join("missing.sock");
    let expected = hex::encode(
        PublicKey::from_secret_key(
            &Secp256k1::signing_only(),
            &SecretKey::from_slice(&[8_u8; 32]).unwrap(),
        )
        .serialize(),
    );

    let raw = delivery_identity(&missing_socket, &raw_key);
    assert_success(&raw);
    let raw_json = parse_identity(&raw);
    assert_eq!(
        raw_json,
        json!({"schema_version": 1, "public_key": expected})
    );

    let hexadecimal = delivery_identity(&missing_socket, &hex_key);
    assert_success(&hexadecimal);
    assert_eq!(parse_identity(&hexadecimal), raw_json);
    assert!(
        !missing_socket.exists(),
        "offline command touched the socket"
    );
}

#[test]
fn unsafe_or_invalid_delivery_keys_fail_without_public_output() {
    let run = private_tempdir();
    let missing_socket = run.path().join("missing.sock");
    let secret_hex = hex::encode([8_u8; 32]);
    let insecure = private_file(run.path(), "insecure.key", &[8_u8; 32], 0o644);
    let short = private_file(run.path(), "short.key", &[8_u8; 31], 0o600);
    let zero = private_file(run.path(), "zero.key", &[0_u8; 32], 0o600);
    let malformed = private_file(run.path(), "malformed.key", &[b'g'; 64], 0o600);
    let oversized = private_file(run.path(), "oversized.key", &[8_u8; 66], 0o600);
    let directory = run.path().join("directory.key");
    fs::create_dir(&directory).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    let missing = run.path().join("missing.key");
    let symlink_target = private_file(run.path(), "symlink-target.key", &[8_u8; 32], 0o600);
    let symlink_path = run.path().join("symlink.key");
    symlink(&symlink_target, &symlink_path).unwrap();
    let linked = private_file(run.path(), "linked.key", &[9_u8; 32], 0o600);
    let linked_alias = run.path().join("linked-alias.key");
    fs::hard_link(&linked, &linked_alias).unwrap();

    for path in [
        insecure,
        short,
        zero,
        malformed,
        oversized,
        directory,
        missing,
        symlink_path,
        linked,
        linked_alias,
    ] {
        let output = delivery_identity(&missing_socket, &path);
        assert!(
            !output.status.success(),
            "unsafe key unexpectedly succeeded: {}",
            path.display()
        );
        assert!(
            output.stdout.is_empty(),
            "failed identity command disclosed JSON for {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stdout)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("Delivery signing key"),
            "failure omitted its safe purpose label for {}: {stderr}",
            path.display()
        );
        assert!(
            !stderr.contains(&secret_hex),
            "failure disclosed private key bytes for {}",
            path.display()
        );
    }
    assert!(
        !missing_socket.exists(),
        "failed offline command touched socket"
    );
}

fn private_tempdir() -> tempfile::TempDir {
    let run = tempdir().unwrap();
    fs::set_permissions(run.path(), fs::Permissions::from_mode(0o700)).unwrap();
    run
}

fn delivery_identity(socket: &Path, key: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lez-maker-cli"))
        .arg("--socket")
        .arg(socket)
        .arg("delivery-identity")
        .arg("--signing-key-file")
        .arg(key)
        .output()
        .unwrap()
}

fn parse_identity(output: &Output) -> Value {
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let public_key = value["public_key"].as_str().unwrap();
    assert_eq!(public_key.len(), 66);
    assert!(public_key.starts_with("02") || public_key.starts_with("03"));
    assert_eq!(public_key, public_key.to_ascii_lowercase());
    value
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
}

fn private_file(root: &Path, name: &str, bytes: &[u8], mode: u32) -> PathBuf {
    let path = root.join(name);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
    path
}
