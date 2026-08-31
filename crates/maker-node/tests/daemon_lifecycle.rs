//! Actual-process contract for the future Logos Core lifecycle seam.

use std::{
    fs,
    os::unix::fs::DirBuilderExt as _,
    path::{Path, PathBuf},
    time::Duration,
};

use lez_maker_node::{
    MakerDaemonLaunchConfig, MakerDaemonLifecycle, MakerDaemonLifecycleError, ProcessMakerDaemon,
};
use tempfile::tempdir;

#[tokio::test]
async fn actual_process_lifecycle_is_ready_healthy_and_gracefully_stopped() {
    let run = tempdir().expect("isolated lifecycle root");
    let database = run.path().join("maker.sqlite3");
    let (config, socket, ready) = config(run.path(), &database, "first");
    let mut lifecycle = ProcessMakerDaemon::default();

    assert_eq!(lifecycle.endpoint(), None);
    assert!(matches!(
        lifecycle.health().await,
        Err(MakerDaemonLifecycleError::NotRunning)
    ));

    lifecycle.start(config.clone()).await.expect("start daemon");
    assert_eq!(lifecycle.endpoint(), Some(socket.as_path()));
    let health = lifecycle.health().await.expect("read-only daemon health");
    assert_eq!(health.endpoint(), socket);
    assert!(health.process_id() > 1);
    assert!(socket.exists());
    assert!(ready.exists());
    assert!(matches!(
        lifecycle.start(config.clone()).await,
        Err(MakerDaemonLifecycleError::AlreadyRunning)
    ));

    lifecycle
        .stop(Duration::from_secs(5))
        .await
        .expect("SIGTERM graceful stop");
    assert_eq!(lifecycle.endpoint(), None);
    assert!(
        !socket.exists(),
        "daemon must remove its exact socket inode"
    );
    assert!(
        !ready.exists(),
        "daemon must remove its exact readiness inode"
    );

    lifecycle
        .start(config)
        .await
        .expect("restart same daemon state");
    lifecycle.health().await.expect("health after restart");
    lifecycle
        .stop(Duration::from_secs(5))
        .await
        .expect("second graceful stop");
    lifecycle
        .stop(Duration::from_millis(1))
        .await
        .expect("stopped lifecycle is idempotent");
}

#[tokio::test]
async fn one_database_has_one_process_lifetime_writer_lease() {
    let run = tempdir().expect("isolated lease root");
    let database = run.path().join("maker.sqlite3");
    let (first_config, _, _) = config(run.path(), &database, "first");
    let (second_config, _, _) = config(run.path(), &database, "second");
    let mut first = ProcessMakerDaemon::default();
    let mut second = ProcessMakerDaemon::default();

    first.start(first_config).await.expect("first lease owner");
    assert!(matches!(
        second.start(second_config.clone()).await,
        Err(MakerDaemonLifecycleError::ExitedBeforeReady(_))
    ));
    first
        .stop(Duration::from_secs(5))
        .await
        .expect("release first lease");

    second
        .start(second_config)
        .await
        .expect("lease transfers only after first exit");
    second
        .stop(Duration::from_secs(5))
        .await
        .expect("stop second lease owner");
}

#[test]
fn launch_config_rejects_relative_paths_before_process_creation() {
    let result = MakerDaemonLaunchConfig::new(
        "lez-maker-node",
        "/tmp/maker.sqlite3",
        "/tmp/maker.sock",
        "/tmp/ready",
        Duration::from_secs(1),
        Duration::from_secs(1),
    );
    assert!(matches!(
        result,
        Err(MakerDaemonLifecycleError::InvalidConfig(_))
    ));
}

fn config(
    root: &Path,
    database: &Path,
    generation: &str,
) -> (MakerDaemonLaunchConfig, PathBuf, PathBuf) {
    let runtime = root.join(format!("{generation}.runtime"));
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&runtime)
        .expect("create owner-only runtime");
    let socket = runtime.join("maker.sock");
    let ready = runtime.join("ready");
    let executable = Path::new(env!("CARGO_BIN_EXE_lez-maker-node"))
        .canonicalize()
        .expect("canonical daemon executable");
    let config = MakerDaemonLaunchConfig::new(
        executable,
        database,
        &socket,
        &ready,
        Duration::from_secs(10),
        Duration::from_secs(2),
    )
    .expect("valid lifecycle config");
    (config, socket, ready)
}
