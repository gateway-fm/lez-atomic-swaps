//! Actual-process contract for exclusive owner-private Taker registry initialization.

use std::{
    fs,
    os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    process::{Command, Output},
};

use lez_swap_store::SqliteTakerFacadeStore;

#[test]
fn creates_one_private_registry_without_emitting_output() {
    let run = tempfile::tempdir().unwrap();
    let root = private_directory(run.path().join("registry-root"));
    let database = root.join("taker.sqlite3");

    let output = initialize(&database);

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    let metadata = fs::symlink_metadata(&database).unwrap();
    assert!(metadata.file_type().is_file());
    assert_eq!(metadata.uid(), rustix::process::geteuid().as_raw());
    assert_eq!(metadata.mode() & 0o7777, 0o600);
    assert_eq!(metadata.nlink(), 1);
    drop(SqliteTakerFacadeStore::open_existing(&database).unwrap());
}

#[test]
fn rejects_relative_existing_and_unsafe_paths_without_replacing_storage() {
    let run = tempfile::tempdir().unwrap();
    let root = private_directory(run.path().join("registry-root"));

    let missing = Command::new(env!("CARGO_BIN_EXE_lez-taker-registry-init"))
        .output()
        .unwrap();
    assert_failed_without_stdout(&missing);

    let duplicate_a = root.join("duplicate-a.sqlite3");
    let duplicate_b = root.join("duplicate-b.sqlite3");
    let duplicate = Command::new(env!("CARGO_BIN_EXE_lez-taker-registry-init"))
        .arg("--database")
        .arg(&duplicate_a)
        .arg("--database")
        .arg(&duplicate_b)
        .output()
        .unwrap();
    assert_failed_without_stdout(&duplicate);
    assert!(!duplicate_a.exists());
    assert!(!duplicate_b.exists());

    let relative = Command::new(env!("CARGO_BIN_EXE_lez-taker-registry-init"))
        .current_dir(&root)
        .args(["--database", "relative.sqlite3"])
        .output()
        .unwrap();
    assert_failed_without_stdout(&relative);
    assert!(!root.join("relative.sqlite3").exists());

    let database = root.join("existing.sqlite3");
    let first = initialize(&database);
    assert!(first.status.success(), "{}", stderr(&first));
    let before = StorageSnapshot::capture(&database);
    let existing = initialize(&database);
    assert_failed_without_stdout(&existing);
    assert_eq!(StorageSnapshot::capture(&database), before);
    drop(SqliteTakerFacadeStore::open_existing(&database).unwrap());

    let target = root.join("target.sqlite3");
    let target_bytes = b"must-not-change";
    fs::write(&target, target_bytes).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    let link = root.join("linked.sqlite3");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let linked = initialize(&link);
    assert_failed_without_stdout(&linked);
    assert_eq!(fs::read(&target).unwrap(), target_bytes);
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );

    let unsafe_root = run.path().join("unsafe-root");
    fs::DirBuilder::new()
        .mode(0o755)
        .create(&unsafe_root)
        .unwrap();
    let unsafe_database = unsafe_root.join("taker.sqlite3");
    let unsafe_parent = initialize(&unsafe_database);
    assert_failed_without_stdout(&unsafe_parent);
    assert!(!unsafe_database.exists());
}

fn initialize(database: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lez-taker-registry-init"))
        .arg("--database")
        .arg(database)
        .output()
        .unwrap()
}

fn assert_failed_without_stdout(output: &Output) {
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn private_directory(path: PathBuf) -> PathBuf {
    fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
    path
}

#[derive(Debug, Eq, PartialEq)]
struct StorageSnapshot {
    bytes: Vec<u8>,
    device: u64,
    inode: u64,
    mode: u32,
    links: u64,
}

impl StorageSnapshot {
    fn capture(path: &Path) -> Self {
        let metadata = fs::symlink_metadata(path).unwrap();
        Self {
            bytes: fs::read(path).unwrap(),
            device: metadata.dev(),
            inode: metadata.ino(),
            mode: metadata.mode(),
            links: metadata.nlink(),
        }
    }
}
