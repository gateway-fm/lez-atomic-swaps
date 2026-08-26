//! Focused contract for reusable owner-local Unix RPC server plumbing.

use std::{
    fs,
    os::unix::fs::{
        DirBuilderExt as _, FileTypeExt as _, MetadataExt as _, PermissionsExt as _, symlink,
    },
    path::{Path, PathBuf},
};

use lez_maker_node::owner_rpc_server::{bind_owner_socket, validate_runtime_directory};
use tempfile::tempdir;

#[tokio::test]
async fn owner_socket_requires_private_runtime_and_is_mode_0600() {
    let run = tempdir().unwrap();
    let runtime = private_runtime(run.path(), "runtime");
    validate_runtime_directory(&runtime).unwrap();

    let socket = runtime.join("owner.sock");
    let (listener, guard) = bind_owner_socket(&socket).unwrap();
    let runtime_metadata = fs::symlink_metadata(&runtime).unwrap();
    assert!(runtime_metadata.file_type().is_dir());
    assert_eq!(runtime_metadata.uid(), rustix::process::geteuid().as_raw());
    assert_eq!(runtime_metadata.permissions().mode() & 0o7777, 0o700);
    let socket_metadata = fs::symlink_metadata(&socket).unwrap();
    assert!(socket_metadata.file_type().is_socket());
    assert_eq!(socket_metadata.uid(), rustix::process::geteuid().as_raw());
    assert_eq!(socket_metadata.permissions().mode() & 0o7777, 0o600);

    drop(listener);
    drop(guard);
    assert!(!socket.exists());
}

#[tokio::test]
async fn owner_socket_refuses_every_preexisting_endpoint_kind() {
    let run = tempdir().unwrap();
    let runtime = private_runtime(run.path(), "runtime");

    let regular = runtime.join("regular.sock");
    fs::write(&regular, b"occupied").unwrap();
    let error = bind_owner_socket(&regular).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("refusing to replace existing maker RPC socket path")
    );

    let target = runtime.join("target");
    fs::write(&target, b"target").unwrap();
    let linked = runtime.join("linked.sock");
    symlink(&target, &linked).unwrap();
    let error = bind_owner_socket(&linked).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("refusing to replace existing maker RPC socket path")
    );
}

#[tokio::test]
async fn runtime_rejects_wrong_mode_and_symlink_alias() {
    let run = tempdir().unwrap();
    let wrong_mode = run.path().join("wrong-mode");
    fs::DirBuilder::new()
        .mode(0o755)
        .create(&wrong_mode)
        .unwrap();
    let error = validate_runtime_directory(&wrong_mode).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("maker RPC runtime directory must have mode 0700")
    );

    let real = private_runtime(run.path(), "real");
    let alias = run.path().join("alias");
    symlink(&real, &alias).unwrap();
    let error = validate_runtime_directory(&alias).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("maker RPC runtime path must be a real directory")
    );
    let error = bind_owner_socket(&alias.join("owner.sock")).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("maker RPC runtime path must be a real directory")
    );
}

#[tokio::test]
async fn path_guard_removes_only_the_inode_it_captured() {
    let run = tempdir().unwrap();
    let runtime = private_runtime(run.path(), "runtime");
    let socket = runtime.join("owner.sock");
    let (listener, guard) = bind_owner_socket(&socket).unwrap();
    drop(listener);

    let staged_replacement = runtime.join("replacement");
    fs::write(&staged_replacement, b"replacement").unwrap();
    let replacement = fs::symlink_metadata(&staged_replacement).unwrap();
    fs::remove_file(&socket).unwrap();
    fs::rename(&staged_replacement, &socket).unwrap();
    drop(guard);

    assert_eq!(fs::read(&socket).unwrap(), b"replacement");
    assert_eq!(
        fs::symlink_metadata(&socket).unwrap().ino(),
        replacement.ino()
    );
}

fn private_runtime(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
    path
}
