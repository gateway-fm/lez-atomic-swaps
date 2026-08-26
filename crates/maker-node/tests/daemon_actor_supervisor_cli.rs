use std::process::Command;
#[cfg(feature = "test-crash-hooks")]
use std::process::Output;

#[cfg(feature = "test-crash-hooks")]
use std::{
    fs,
    os::unix::fs::{PermissionsExt as _, symlink},
};
#[cfg(feature = "test-crash-hooks")]
use tempfile::tempdir;

#[cfg(feature = "test-crash-hooks")]
use lez_maker_node::MakerActorSupervisorConfig;
#[cfg(feature = "test-crash-hooks")]
use lez_swap_core::SwapId;
#[cfg(feature = "test-crash-hooks")]
use std::time::Duration;

#[test]
fn test_crash_hook_flags_are_absent_from_operator_help() {
    let output = daemon().arg("--help").output().expect("daemon help");
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(!help.contains("actor-test-pause"));
}

#[cfg(not(feature = "test-crash-hooks"))]
#[test]
fn default_build_rejects_test_crash_hook_flags() {
    let output = daemon()
        .args(["--actor-test-pause-swap-id", "unavailable"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unexpected argument"));
}

#[cfg(feature = "test-crash-hooks")]
#[test]
fn feature_build_requires_the_complete_hook_group() {
    let output = daemon()
        .args(["--actor-supervisor", "--actor-test-pause-swap-id", "swap-a"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("required arguments"));
}

#[cfg(feature = "test-crash-hooks")]
#[test]
fn feature_build_admits_exact_monero_refund_pause_operation() {
    let marker = std::path::PathBuf::from("/tmp/xmr-refund-paused.json");
    let config = MakerActorSupervisorConfig::new(Duration::from_secs(10), 5, 30, 8_192).unwrap();
    assert!(
        config
            .with_test_pause_after_submitted(
                SwapId::new("swap-xmr").unwrap(),
                "sweep_monero_refund",
                marker,
            )
            .is_ok()
    );
}

#[cfg(feature = "test-crash-hooks")]
#[test]
fn feature_build_rejects_unsafe_or_noncanonical_marker_parents() {
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o755)).unwrap();
    let unsafe_output = configured_output(
        &root.path().join("maker.sqlite3"),
        &root.path().join("pause.json"),
    );
    assert!(!unsafe_output.status.success());
    assert!(String::from_utf8_lossy(&unsafe_output.stderr).contains("marker parent"));

    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let private = root.path().join("private");
    fs::create_dir(&private).unwrap();
    fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).unwrap();
    let alias = root.path().join("alias");
    symlink(&private, &alias).unwrap();
    let alias_output = configured_output(
        &root.path().join("other.sqlite3"),
        &alias.join("pause.json"),
    );
    assert!(!alias_output.status.success());
    assert!(String::from_utf8_lossy(&alias_output.stderr).contains("marker parent"));
}

#[cfg(feature = "test-crash-hooks")]
fn configured_output(database: &std::path::Path, marker: &std::path::Path) -> Output {
    daemon()
        .arg("--database")
        .arg(database)
        .arg("--actor-supervisor")
        .arg("--actor-test-pause-swap-id")
        .arg("swap-a")
        .arg("--actor-test-pause-operation")
        .arg("zcash_fund")
        .arg("--actor-test-pause-marker")
        .arg(marker)
        .output()
        .unwrap()
}

fn daemon() -> Command {
    Command::new(env!("CARGO_BIN_EXE_lez-maker-daemon"))
}
