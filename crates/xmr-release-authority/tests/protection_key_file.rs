//! Owner-private release protection-key file contract.

#![cfg(unix)]
#![forbid(unsafe_code)]

use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _, symlink},
    path::Path,
};

use lez_xmr_release_authority::{ProtectionKeyFileError, PublicationProtectionKey};

const KEY_HEX: &str = "5858585858585858585858585858585858585858585858585858585858585858";

#[test]
fn exact_lowercase_key_with_one_line_ending_loads_and_stays_redacted() {
    let directory = tempfile::tempdir().expect("private directory");
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
        .expect("private directory mode");
    let path = directory.path().join("journal-key");
    write_private(&path, format!("{KEY_HEX}\r\n").as_bytes());

    let key = PublicationProtectionKey::from_owner_private_file("release-key-v1", &path)
        .expect("owner-private protection key");
    assert_eq!(key.key_id(), "release-key-v1");
    let rendered = format!("{key:?}");
    assert!(rendered.contains("[REDACTED]"));
    assert!(!rendered.contains(KEY_HEX));
    assert!(!rendered.contains(&path.display().to_string()));
}

#[test]
fn invalid_or_zero_key_contents_fail_closed_without_disclosure() {
    let directory = tempfile::tempdir().expect("private directory");
    let path = directory.path().join("private-key-marker");
    write_private(&path, b"not-a-release-key");

    for contents in [
        "not-a-release-key".to_owned(),
        "AB".repeat(32),
        "00".repeat(32),
        format!("{KEY_HEX}\n\n"),
    ] {
        fs::write(&path, &contents).expect("replace invalid key contents");
        let error = PublicationProtectionKey::from_owner_private_file("release-key-v1", &path)
            .expect_err("invalid key is rejected");
        assert_eq!(error, ProtectionKeyFileError::InvalidContents);
        let rendered = format!("{error:?} {error}");
        assert!(!rendered.contains(&contents));
        assert!(!rendered.contains("private-key-marker"));
    }
}

#[test]
fn owner_mode_single_link_and_non_symlink_are_required() {
    let directory = tempfile::tempdir().expect("private directory");
    let path = directory.path().join("journal-key");
    write_private(&path, KEY_HEX.as_bytes());

    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).expect("weaken key mode");
    assert_unsafe(&path);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("restore key mode");

    let hard_link = directory.path().join("hard-link");
    fs::hard_link(&path, &hard_link).expect("create hard link");
    assert_unsafe(&path);
    assert_unsafe(&hard_link);
    fs::remove_file(hard_link).expect("remove hard link");

    let symbolic_link = directory.path().join("symbolic-link");
    symlink(&path, &symbolic_link).expect("create symbolic link");
    assert_unsafe(&symbolic_link);
}

fn assert_unsafe(path: &Path) {
    assert_eq!(
        PublicationProtectionKey::from_owner_private_file("release-key-v1", path)
            .expect_err("unsafe key file is rejected"),
        ProtectionKeyFileError::Unsafe
    );
}

fn write_private(path: &Path, contents: &[u8]) {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .expect("create owner-private key");
    file.write_all(contents).expect("write owner-private key");
}
