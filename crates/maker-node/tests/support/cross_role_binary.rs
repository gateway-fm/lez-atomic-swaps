use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
};

static BUILD_LOCK: Mutex<()> = Mutex::new(());

/// Resolves a separately owned workspace binary next to this test executable.
///
/// Cargo only defines `CARGO_BIN_EXE_*` for binaries owned by the package under
/// test. Build a missing peer binary from its owning package so focused tests
/// remain directly runnable after the role-package split.
pub fn workspace_binary(name: &str) -> PathBuf {
    let test_executable = std::env::current_exe().expect("resolve integration-test executable");
    let profile_directory = test_executable
        .parent()
        .and_then(std::path::Path::parent)
        .expect("integration test runs below target/<profile>/deps");
    let executable = profile_directory.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    if executable.is_file() {
        return executable;
    }

    let _build = BUILD_LOCK.lock().expect("lock peer-binary build");
    if executable.is_file() {
        return executable;
    }
    let package = if name == "lez-chat-relay" {
        "lez-node-common"
    } else if name.starts_with("lez-taker-") {
        "lez-taker-node"
    } else {
        panic!("unsupported cross-role test binary: {name}");
    };
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("Maker package is below the workspace crates directory");
    let target_directory = profile_directory
        .parent()
        .expect("profile is below the Cargo target directory");
    let profile = profile_directory
        .file_name()
        .and_then(|name| name.to_str())
        .expect("Cargo profile directory is UTF-8");
    let mut command =
        Command::new(std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo")));
    command
        .current_dir(workspace)
        .env("CARGO_TARGET_DIR", target_directory)
        .args([
            "build",
            "--locked",
            "--offline",
            "-p",
            package,
            "--bin",
            name,
        ]);
    match profile {
        "debug" => {}
        "release" => {
            command.arg("--release");
        }
        custom => {
            command.args(["--profile", custom]);
        }
    }
    let status = command.status().expect("run Cargo peer-binary build");
    assert!(status.success(), "build {name} from package {package}");
    assert!(executable.is_file(), "Cargo did not produce {name}");
    executable
}
