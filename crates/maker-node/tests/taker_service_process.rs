//! Actual-process contract for the owner-only read-only Taker service.

use std::{
    fs,
    os::unix::{
        fs::{DirBuilderExt as _, FileTypeExt as _, MetadataExt as _, PermissionsExt as _},
        net::UnixListener,
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use lez_maker_node::{
    TakerDependencyStateV1, TakerHealthRequestV1, TakerHealthV1, TakerOfferListRequestV1,
    TakerOfferListV1, call_local_rpc,
};
use rustix::process::{Pid, Signal, kill_process};
use serde_json::{Value, json};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_service_serves_only_read_methods_and_cleans_exact_socket_on_sigterm() {
    let run = tempfile::tempdir().unwrap();
    let runtime = private_directory(run.path().join("runtime"));
    let config = private_config(run.path().join("taker-service.json"), &valid_config());
    let socket = runtime.join("taker.sock");
    let mut service = TestService::spawn(&config, &socket);
    wait_until_ready(&mut service, &socket).await;

    let metadata = fs::symlink_metadata(&socket).unwrap();
    assert!(metadata.file_type().is_socket());
    assert_eq!(metadata.mode() & 0o7777, 0o600);

    let health: TakerHealthV1 = call_local_rpc(
        &socket,
        "taker_health",
        &TakerHealthRequestV1 { schema_version: 1 },
    )
    .await
    .unwrap();
    assert!(health.is_ready());
    assert_eq!(health.delivery(), TakerDependencyStateV1::Disabled);
    assert_eq!(health.chat(), TakerDependencyStateV1::Disabled);

    let offers: TakerOfferListV1 = call_local_rpc(
        &socket,
        "taker_offer_list_v1",
        &TakerOfferListRequestV1 {
            schema_version: 1,
            route: None,
        },
    )
    .await
    .unwrap();
    assert!(offers.offers.is_empty());

    for method in [
        "maker_health",
        "taker_swap_list_v1",
        "taker_swap_initiate_v1",
        "taker_swap_monitor_v1",
        "taker_swap_claim_v1",
        "taker_swap_refund_v1",
    ] {
        let error = call_local_rpc::<_, Value>(&socket, method, &json!({"schema_version": 1}))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("-32601"), "{error:#}");
    }

    assert!(service.terminate().await.success());
    assert!(!socket.exists(), "SIGTERM left the owned socket path");

    let mut restarted = TestService::spawn(&config, &socket);
    wait_until_ready(&mut restarted, &socket).await;
    let original = socket_identity(&socket);

    fs::remove_file(&socket).unwrap();
    let replacement = UnixListener::bind(&socket).unwrap();
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
    let replacement_identity = socket_identity(&socket);
    assert_ne!(replacement_identity, original);

    assert!(restarted.terminate().await.success());
    assert_eq!(
        socket_identity(&socket),
        replacement_identity,
        "service removed a replacement socket with a different inode"
    );
    drop(replacement);
    fs::remove_file(&socket).unwrap();
}

#[test]
fn startup_requires_private_config_and_rejects_invalid_or_relative_socket_before_bind() {
    let run = tempfile::tempdir().unwrap();
    let runtime = private_directory(run.path().join("runtime"));

    let help = Command::new(env!("CARGO_BIN_EXE_lez-taker-service"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    assert!(help.contains("--config"));
    assert!(help.contains("/run/lez-atomic-swaps/taker.sock"));

    let missing_config_socket = runtime.join("missing-config.sock");
    let missing = Command::new(env!("CARGO_BIN_EXE_lez-taker-service"))
        .arg("--socket")
        .arg(&missing_config_socket)
        .output()
        .unwrap();
    assert!(!missing.status.success());
    assert!(!missing_config_socket.exists());

    let invalid = private_config(
        run.path().join("invalid.json"),
        &json!({
            "schema_version": 2,
            "delivery_sources": [],
            "maximum_offers": 16,
        }),
    );
    let invalid_socket = runtime.join("invalid.sock");
    let failed = Command::new(env!("CARGO_BIN_EXE_lez-taker-service"))
        .arg("--config")
        .arg(&invalid)
        .arg("--socket")
        .arg(&invalid_socket)
        .output()
        .unwrap();
    assert!(!failed.status.success());
    assert!(!invalid_socket.exists());

    let valid = private_config(run.path().join("valid.json"), &valid_config());
    let relative = Command::new(env!("CARGO_BIN_EXE_lez-taker-service"))
        .current_dir(run.path())
        .arg("--config")
        .arg(&valid)
        .arg("--socket")
        .arg("relative.sock")
        .output()
        .unwrap();
    assert!(!relative.status.success());
    assert!(!run.path().join("relative.sock").exists());
}

fn valid_config() -> Value {
    json!({
        "schema_version": 1,
        "delivery_sources": [],
        "maximum_offers": 16,
    })
}

fn private_config(path: PathBuf, value: &Value) -> PathBuf {
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    path
}

fn private_directory(path: PathBuf) -> PathBuf {
    fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
    path
}

fn socket_identity(path: &Path) -> (u64, u64) {
    let metadata = fs::symlink_metadata(path).unwrap();
    (metadata.dev(), metadata.ino())
}

struct TestService(Option<Child>);

impl TestService {
    fn spawn(config: &Path, socket: &Path) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_lez-taker-service"))
            .arg("--config")
            .arg(config)
            .arg("--socket")
            .arg(socket)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        Self(Some(child))
    }

    fn child_mut(&mut self) -> &mut Child {
        self.0.as_mut().expect("Taker service is running")
    }

    async fn terminate(&mut self) -> std::process::ExitStatus {
        let child = self.child_mut();
        kill_process(Pid::from_child(child), Signal::TERM).unwrap();
        let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                self.0 = None;
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "Taker service exceeded graceful shutdown deadline"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

impl Drop for TestService {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

async fn wait_until_ready(service: &mut TestService, socket: &Path) {
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        assert!(
            service.child_mut().try_wait().unwrap().is_none(),
            "Taker service exited before readiness"
        );
        if fs::symlink_metadata(socket).is_ok_and(|metadata| metadata.file_type().is_socket()) {
            let request = TakerHealthRequestV1 { schema_version: 1 };
            if tokio::time::timeout(
                Duration::from_millis(250),
                call_local_rpc::<_, TakerHealthV1>(socket, "taker_health", &request),
            )
            .await
            .is_ok_and(|result| result.is_ok())
            {
                return;
            }
        }
        assert!(
            Instant::now() < deadline,
            "Taker service did not become ready"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
