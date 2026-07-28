mod support;

use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _},
    path::Path,
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

use lez_swap_core::{
    Chain, ChainPosition, ConfirmationPolicy, Pair, RecoverySchedule, SwapCoordinator,
    SwapDirection, SwapId, TimelockSafety,
};
use lez_swap_store::{
    MakerActorKindV1, MakerActorManifestV1, MakerActorScheduleState, SqliteSwapStore,
};
use rustix::process::{Pid, Signal, kill_process};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use tempfile::tempdir;
use zec_reference_actor::ActorConfig;

use support::actor_deployment;

#[test]
#[allow(clippy::too_many_lines)] // One process journey keeps readiness, RPC, and reap ordering visible.
fn enabled_daemon_supervises_actor_without_blocking_health_and_cancels_on_sigterm() {
    let root = tempdir().expect("isolated daemon-supervisor root");
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700))
        .expect("owner-only test root");
    let swap_id = "m5-daemon-supervisor-process";
    let deployment = actor_deployment(root.path(), swap_id);
    let actor_config =
        ActorConfig::load_private(&deployment.source_config).expect("real Maker actor config");
    let actor_config_bytes =
        fs::read(&deployment.source_config).expect("read exact actor configuration");
    let actor_pid_file = root.path().join("actor.pid");
    let actor_program = root.path().join("long-running-zec-maker-actor");
    let program = format!(
        "#!/bin/sh\n\
         test \"$1\" = \"--config-fd\" || exit 91\n\
         test \"$2\" = \"196\" || exit 92\n\
         test -r /proc/self/fd/196 || exit 93\n\
         test -r /proc/self/fd/198 || exit 94\n\
         test \"$3\" = \"status\" || exit 95\n\
         printf '%s\\n' \"$$\" > \"{}\"\n\
         exec /usr/bin/sleep 300\n",
        actor_pid_file.display()
    );
    write_private(&actor_program, program.as_bytes(), 0o700);

    let database = root.path().join("maker.sqlite3");
    let mut store = SqliteSwapStore::open(&database).expect("open isolated coordinator database");
    store.save(&swap(swap_id)).expect("save ZEC swap");
    store
        .register_maker_actor(
            &MakerActorManifestV1::new(
                SwapId::new(swap_id).unwrap(),
                MakerActorKindV1::Zcash,
                deployment.source_config,
                Sha256::digest(actor_config_bytes).into(),
                actor_program,
                Sha256::digest(program.as_bytes()).into(),
                actor_config.role_state_db().to_path_buf(),
            )
            .expect("valid ZEC Maker manifest"),
            0,
        )
        .expect("pre-register queued ZEC Maker actor");
    drop(store);

    let runtime = root.path().join("runtime");
    fs::create_dir(&runtime).expect("create runtime directory");
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).expect("owner-only runtime");
    let socket = runtime.join("maker.sock");
    let ready = runtime.join("ready");
    let mut daemon = TestDaemon::spawn(&database, &socket, &ready);
    wait_for_file(
        &mut daemon,
        &ready,
        Duration::from_secs(10),
        "daemon readiness",
    );
    wait_for_file(
        &mut daemon,
        &actor_pid_file,
        Duration::from_secs(5),
        "actor process identity",
    );
    let actor_pid: u32 = fs::read_to_string(&actor_pid_file)
        .expect("read actor PID")
        .trim()
        .parse()
        .expect("numeric actor PID");

    let running = SqliteSwapStore::open(&database)
        .expect("open independent observer connection")
        .list_maker_actor_processes()
        .expect("inspect leased actor")
        .remove(0);
    assert_eq!(running.schedule_state(), MakerActorScheduleState::Leased);
    assert_eq!(
        running.child_identity().map(|identity| identity.0),
        Some(actor_pid)
    );

    let health_started = Instant::now();
    let health = command_output_with_timeout(
        Command::new(env!("CARGO_BIN_EXE_lez-maker"))
            .arg("--socket")
            .arg(&socket)
            .arg("health"),
        Duration::from_secs(1),
        "owner health RPC while actor is running",
    );
    assert!(
        health.status.success(),
        "health command failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&health.stdout),
        String::from_utf8_lossy(&health.stderr)
    );
    assert!(
        health_started.elapsed() < Duration::from_secs(1),
        "health must not wait for the actor supervisor's store connection"
    );
    let health: Value = serde_json::from_slice(&health.stdout).expect("health JSON");
    assert_eq!(health["ready"], true);

    let shutdown_started = Instant::now();
    let status = daemon.terminate(Duration::from_secs(2));
    assert!(
        status.success(),
        "daemon did not complete graceful SIGTERM shutdown: {status}"
    );
    assert!(
        shutdown_started.elapsed() < Duration::from_secs(2),
        "SIGTERM must promptly cancel and reap the in-flight actor"
    );

    let durable = SqliteSwapStore::open(&database)
        .expect("reopen durable coordinator database")
        .list_maker_actor_processes()
        .expect("inspect durable actor after shutdown")
        .remove(0);
    assert_ne!(durable.schedule_state(), MakerActorScheduleState::Leased);
    assert_eq!(durable.child_identity(), None);
    assert!(
        !Path::new("/proc").join(actor_pid.to_string()).exists(),
        "the recorded actor process must be reaped before daemon exit"
    );
    assert!(!socket.exists(), "daemon must remove its owner socket");
    assert!(!ready.exists(), "daemon must remove its readiness file");
}

fn swap(id: &str) -> SwapCoordinator {
    let direction = SwapDirection::TakerSellsForeign;
    SwapCoordinator::new_with_direction(
        SwapId::new(id).unwrap(),
        Pair::Zcash,
        direction,
        ConfirmationPolicy::new(2).unwrap(),
        RecoverySchedule::new(
            Pair::Zcash,
            direction,
            ChainPosition::block_height(Chain::Lez, 100),
            ChainPosition::block_height(Chain::Zcash, 120),
            TimelockSafety::between(Chain::Lez, Chain::Zcash, 1_000, 1_200, 100).unwrap(),
        )
        .unwrap(),
    )
}

struct TestDaemon(Option<Child>);

impl TestDaemon {
    fn spawn(database: &Path, socket: &Path, ready: &Path) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_lez-maker-daemon"))
            .arg("--socket")
            .arg(socket)
            .arg("--database")
            .arg(database)
            .arg("--ready-file")
            .arg(ready)
            .arg("--actor-supervisor")
            .arg("--actor-poll-milliseconds")
            .arg("10")
            .spawn()
            .expect("start actor-supervising maker daemon");
        Self(Some(child))
    }

    fn child_mut(&mut self) -> &mut Child {
        self.0.as_mut().expect("daemon is running")
    }

    fn terminate(&mut self, timeout: Duration) -> std::process::ExitStatus {
        let child = self.child_mut();
        kill_process(Pid::from_child(child), Signal::TERM).expect("SIGTERM maker daemon");
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = child.try_wait().expect("poll maker daemon") {
                self.0 = None;
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "maker daemon exceeded graceful shutdown deadline"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for TestDaemon {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn wait_for_file(daemon: &mut TestDaemon, path: &Path, timeout: Duration, description: &str) {
    let deadline = Instant::now() + timeout;
    loop {
        if path.exists() {
            return;
        }
        if let Some(status) = daemon.child_mut().try_wait().expect("poll maker daemon") {
            panic!("maker daemon exited before {description}: {status}");
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {description}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn command_output_with_timeout(
    command: &mut Command,
    timeout: Duration,
    description: &str,
) -> Output {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("start {description}: {error}"));
    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait().expect("poll bounded command").is_some() {
            return child
                .wait_with_output()
                .unwrap_or_else(|error| panic!("collect {description}: {error}"));
        }
        if Instant::now() >= deadline {
            child.kill().expect("kill timed-out command");
            let output = child.wait_with_output().expect("reap timed-out command");
            panic!(
                "{description} exceeded {timeout:?}\nstdout: {}\nstderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn write_private(path: &Path, bytes: &[u8], mode: u32) {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)
        .expect("create private test artifact");
    file.write_all(bytes).expect("write private test artifact");
    file.sync_all().expect("sync private test artifact");
}
