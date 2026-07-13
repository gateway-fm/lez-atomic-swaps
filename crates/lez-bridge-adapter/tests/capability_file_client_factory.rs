//! Security contract for constructing one fresh authenticated sidecar client per attempt.

#![forbid(unsafe_code)]

use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use lez_bridge_adapter::{
    CapabilityFileBridgeClientFactory, CapabilityFileBridgeClientFactoryError,
    FreshLezBridgeTransportFactory,
};
use lez_bridge_client::BridgeClient;
use lez_bridge_protocol::{Hex32, Participant, RunId, RuntimeCompatibility, RuntimeDescriptor};

const CAPABILITY: &str = "factory-capability-0000000000000001";
static TEST_DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[test]
fn every_transport_rereads_the_capability_and_builds_a_fresh_client() {
    let directory = TestDirectory::new("fresh");
    let capability_file = directory.path().join("capability");
    write_secret(&capability_file, format!("{CAPABILITY}\n").as_bytes());
    let factory = factory(&capability_file);

    let first: BridgeClient = factory.fresh_transport().expect("first fresh client");
    fs::write(&capability_file, b"invalid capability with spaces")
        .expect("replace capability contents");
    assert!(matches!(
        factory.fresh_transport(),
        Err(CapabilityFileBridgeClientFactoryError::InvalidCapability)
    ));
    fs::write(&capability_file, format!("{CAPABILITY}\r\n"))
        .expect("rotate valid capability contents");
    let second: BridgeClient = factory.fresh_transport().expect("second fresh client");

    assert_eq!(format!("{first:?}"), format!("{second:?}"));
}

#[test]
fn capability_file_is_nonempty_bounded_and_has_only_one_optional_line_ending() {
    let directory = TestDirectory::new("bounds");
    let capability_file = directory.path().join("capability");
    write_secret(&capability_file, b"");
    let factory = factory(&capability_file);
    assert!(matches!(
        factory.fresh_transport(),
        Err(CapabilityFileBridgeClientFactoryError::UnsafeCapabilityFile)
    ));

    fs::write(&capability_file, vec![b'a'; 131]).expect("write oversized capability");
    assert!(matches!(
        factory.fresh_transport(),
        Err(CapabilityFileBridgeClientFactoryError::UnsafeCapabilityFile)
    ));

    let mut maximum_with_crlf = vec![b'a'; 128];
    maximum_with_crlf.extend_from_slice(b"\r\n");
    fs::write(&capability_file, maximum_with_crlf).expect("write maximum capability");
    factory
        .fresh_transport()
        .expect("128-byte capability plus CRLF is accepted");

    fs::write(&capability_file, format!("{CAPABILITY}\nsecond-line"))
        .expect("write embedded newline");
    assert!(matches!(
        factory.fresh_transport(),
        Err(CapabilityFileBridgeClientFactoryError::InvalidCapability)
    ));

    fs::write(&capability_file, format!("{CAPABILITY}\n\n")).expect("write two line endings");
    assert!(matches!(
        factory.fresh_transport(),
        Err(CapabilityFileBridgeClientFactoryError::InvalidCapability)
    ));
}

#[test]
fn missing_and_non_regular_capability_paths_are_rejected() {
    let directory = TestDirectory::new("file-kind");
    let missing = directory.path().join("missing-secret-name");
    assert!(matches!(
        factory(&missing).fresh_transport(),
        Err(CapabilityFileBridgeClientFactoryError::CapabilityFileUnavailable)
    ));
    assert!(matches!(
        factory(directory.path()).fresh_transport(),
        Err(CapabilityFileBridgeClientFactoryError::UnsafeCapabilityFile)
    ));
}

#[cfg(unix)]
#[test]
fn unix_capability_file_requires_exact_mode_0600_and_rejects_symlinks() {
    use std::os::unix::fs::{PermissionsExt as _, symlink};

    let directory = TestDirectory::new("unix-safety");
    let target = directory.path().join("target");
    write_secret(&target, CAPABILITY.as_bytes());
    fs::set_permissions(&target, fs::Permissions::from_mode(0o640))
        .expect("weaken capability permissions");
    assert!(matches!(
        factory(&target).fresh_transport(),
        Err(CapabilityFileBridgeClientFactoryError::UnsafeCapabilityFile)
    ));

    fs::set_permissions(&target, fs::Permissions::from_mode(0o600))
        .expect("restore private permissions");
    fs::set_permissions(&target, fs::Permissions::from_mode(0o4600))
        .expect("add set-user-ID permission");
    assert!(matches!(
        factory(&target).fresh_transport(),
        Err(CapabilityFileBridgeClientFactoryError::UnsafeCapabilityFile)
    ));

    fs::set_permissions(&target, fs::Permissions::from_mode(0o600))
        .expect("restore exact private permissions");
    let link = directory.path().join("capability-link");
    symlink(&target, &link).expect("create capability symlink");
    assert!(matches!(
        factory(&link).fresh_transport(),
        Err(CapabilityFileBridgeClientFactoryError::UnsafeCapabilityFile)
    ));
}

#[test]
fn debug_and_errors_redact_capability_contents_and_path() {
    let directory = TestDirectory::new("redaction");
    let path_marker = "private-path-marker";
    let secret_marker = "private-secret-marker";
    let capability_file = directory.path().join(path_marker);
    write_secret(&capability_file, secret_marker.as_bytes());
    let factory = factory(&capability_file);
    let error = factory
        .fresh_transport()
        .expect_err("invalid capability is rejected");

    for rendered in [
        format!("{factory:?}"),
        format!("{error:?}"),
        error.to_string(),
    ] {
        assert!(!rendered.contains(path_marker));
        assert!(!rendered.contains(secret_marker));
        assert!(!rendered.contains(&capability_file.display().to_string()));
    }
}

fn factory(path: &Path) -> CapabilityFileBridgeClientFactory {
    CapabilityFileBridgeClientFactory::new(
        "http://127.0.0.1:31415",
        path,
        RunId::new("factory-test-run").expect("run id"),
        RuntimeDescriptor::new(
            Participant::Maker,
            RuntimeCompatibility::NssaV0_1_2,
            Hex32::from_bytes([0x11; 32]),
            Hex32::from_bytes([0x12; 32]),
            Hex32::from_bytes([0x13; 32]),
            Hex32::from_bytes([0x14; 32]),
            Hex32::from_bytes([0x15; 32]),
        ),
        Duration::from_secs(2),
    )
}

fn write_secret(path: &Path, bytes: &[u8]) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;

        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(path)
            .expect("create private capability file");
        file.write_all(bytes).expect("write capability file");
    }
    #[cfg(not(unix))]
    {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)
            .expect("create capability file");
        file.write_all(bytes).expect("write capability file");
    }
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "lez-bridge-adapter-{label}-{}-{}",
            std::process::id(),
            TEST_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("create isolated test directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
