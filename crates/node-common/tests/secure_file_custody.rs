use std::{
    fs,
    os::unix::fs::{MetadataExt as _, PermissionsExt as _, symlink},
    path::{Path, PathBuf},
};

use lez_node_common::secure_file::{read_private_file, read_private_file_snapshot};

#[test]
fn snapshot_returns_bytes_and_identity_from_the_exact_private_file() {
    let run = tempfile::tempdir().unwrap();
    let path = private_file(run.path(), "authority.bin", b"private-authority", 0o600);
    let expected = fs::metadata(&path).unwrap();

    let snapshot = read_private_file_snapshot(&path, 64, "test authority").unwrap();

    assert_eq!(snapshot.bytes(), b"private-authority");
    assert_eq!(snapshot.identity().device(), expected.dev());
    assert_eq!(snapshot.identity().inode(), expected.ino());
    assert_eq!(snapshot.identity().length(), expected.len());

    let debug = format!("{snapshot:?}");
    assert!(debug.contains("[REDACTED]"));
    assert!(!debug.contains("private-authority"));
    assert!(!debug.contains(path.to_str().unwrap()));

    let delegated = read_private_file(&path, 64, "test authority").unwrap();
    assert_eq!(delegated.as_slice(), b"private-authority");
}

#[test]
fn snapshot_rejects_symlinks_hardlinks_wrong_modes_and_oversized_files() {
    let run = tempfile::tempdir().unwrap();

    let target = private_file(run.path(), "target.bin", b"target", 0o600);
    let alias = run.path().join("alias.bin");
    symlink(&target, &alias).unwrap();
    assert!(read_private_file_snapshot(&alias, 64, "test authority").is_err());

    let linked = private_file(run.path(), "linked.bin", b"linked", 0o600);
    fs::hard_link(&linked, run.path().join("second-link.bin")).unwrap();
    assert!(read_private_file_snapshot(&linked, 64, "test authority").is_err());

    for (name, mode) in [("shared.bin", 0o640), ("executable.bin", 0o700)] {
        let path = private_file(run.path(), name, b"private", mode);
        assert!(read_private_file_snapshot(&path, 64, "test authority").is_err());
    }

    let oversized = private_file(run.path(), "oversized.bin", &[0x41; 65], 0o600);
    assert!(read_private_file_snapshot(&oversized, 64, "test authority").is_err());
}

fn private_file(root: &Path, name: &str, bytes: &[u8], mode: u32) -> PathBuf {
    let path = root.join(name);
    fs::write(&path, bytes).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
    path
}
