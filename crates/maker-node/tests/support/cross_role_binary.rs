use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
};

/// The peer binaries this test process has built, so each is built once.
static BUILT: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// The pair features the peer binary is built with: the Maker test's own.
/// A Zcash test run against a Taker built without the Zcash pair fails at its
/// command line (`--accept-zec-offer` unknown), and any `lez-taker-cli`
/// already in the target directory may have been built without it.
fn peer_features() -> Vec<&'static str> {
    let mut features = Vec::new();
    if cfg!(feature = "pair-zec") {
        features.push("pair-zec");
    }
    if cfg!(feature = "pair-xmr") {
        features.push("pair-xmr");
    }
    features
}

/// Resolves a separately owned workspace binary next to this test executable.
///
/// Cargo only defines `CARGO_BIN_EXE_*` for binaries owned by the package under
/// test. The peer binary is built from its owning package, with this test's
/// pair features, once per test process (a no-op when it is current), so a
/// binary left in the target directory by another build is never trusted.
pub fn workspace_binary(name: &str) -> PathBuf {
    let test_executable = std::env::current_exe().expect("resolve integration-test executable");
    let profile_directory = test_executable
        .parent()
        .and_then(std::path::Path::parent)
        .expect("integration test runs below target/<profile>/deps");
    let executable = profile_directory.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    let mut built = BUILT.lock().expect("lock peer-binary build");
    if built.iter().any(|done| done == name) && executable.is_file() {
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
    let features = peer_features();
    if package == "lez-taker-node" && !features.is_empty() {
        command.args(["--features", &features.join(",")]);
    }
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
    built.push(name.to_owned());
    executable
}
