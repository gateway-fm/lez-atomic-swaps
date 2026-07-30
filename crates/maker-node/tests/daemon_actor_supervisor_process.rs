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

#[test]
#[allow(clippy::too_many_lines)] // One process journey keeps both durable actor rows visible.
fn daemon_runs_overlapping_actors_and_isolates_failing_peer_across_restart() {
    let root = tempdir().expect("isolated two-swap daemon root");
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700))
        .expect("owner-only test root");
    let timed_out_id = "m5-daemon-a-timeout";
    let terminal_id = "m5-daemon-b-terminal";
    let timed_out_root = root.path().join("timed-out");
    let terminal_root = root.path().join("terminal");
    for directory in [&timed_out_root, &terminal_root] {
        fs::create_dir(directory).expect("create disjoint actor fixture root");
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
            .expect("owner-only actor fixture root");
    }
    let timed_out_deployment = actor_deployment(&timed_out_root, timed_out_id);
    let terminal_deployment = actor_deployment(&terminal_root, terminal_id);
    let timed_out_config =
        ActorConfig::load_private(&timed_out_deployment.source_config).expect("timeout config");
    let terminal_config =
        ActorConfig::load_private(&terminal_deployment.source_config).expect("terminal config");

    let timed_out_pid_file = root.path().join("timed-out.pid");
    let timed_out_release = root.path().join("timed-out.release");
    let timed_out_invocations = root.path().join("timed-out.invocations");
    let timed_out_program_path = root.path().join("timed-out-zec-maker-actor");
    let timed_out_program = format!(
        "#!/bin/sh\n\
         test \"$1\" = \"--config-fd\" || exit 91\n\
         test \"$2\" = \"196\" || exit 92\n\
         test -r /proc/self/fd/196 || exit 93\n\
         test -r /proc/self/fd/198 || exit 94\n\
         test \"$3\" = \"status\" || exit 95\n\
         printf '%s\\n' \"$3\" >> \"{}\"\n\
         printf '%s\\n' \"$$\" > \"{}\"\n\
         while test ! -f \"{}\"; do /usr/bin/sleep 0.01; done\n\
         exit 73\n",
        timed_out_invocations.display(),
        timed_out_pid_file.display(),
        timed_out_release.display()
    );
    write_private(&timed_out_program_path, timed_out_program.as_bytes(), 0o700);

    let terminal_invocations = root.path().join("terminal.invocations");
    let terminal_program_path = root.path().join("terminal-zec-maker-actor");
    let terminal_program = format!(
        "#!/bin/sh\n\
         test \"$1\" = \"--config-fd\" || exit 91\n\
         test \"$2\" = \"196\" || exit 92\n\
         test -r /proc/self/fd/196 || exit 93\n\
         test -r /proc/self/fd/198 || exit 94\n\
         test \"$3\" = \"status\" || exit 95\n\
         printf '%s\\n' \"$3\" >> \"{}\"\n\
         printf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"state\":\"active\",\"phase\":\"completed\",\"revision\":4,\"next_action\":\"complete\"}}'\n",
        terminal_invocations.display()
    );
    write_private(&terminal_program_path, terminal_program.as_bytes(), 0o700);

    let timed_out_manifest = MakerActorManifestV1::new(
        SwapId::new(timed_out_id).unwrap(),
        MakerActorKindV1::Zcash,
        timed_out_deployment.source_config.clone(),
        Sha256::digest(fs::read(&timed_out_deployment.source_config).expect("read timeout config"))
            .into(),
        timed_out_program_path,
        Sha256::digest(timed_out_program.as_bytes()).into(),
        timed_out_config.role_state_db().to_path_buf(),
    )
    .expect("valid timeout manifest");
    let terminal_manifest = MakerActorManifestV1::new(
        SwapId::new(terminal_id).unwrap(),
        MakerActorKindV1::Zcash,
        terminal_deployment.source_config.clone(),
        Sha256::digest(fs::read(&terminal_deployment.source_config).expect("read terminal config"))
            .into(),
        terminal_program_path,
        Sha256::digest(terminal_program.as_bytes()).into(),
        terminal_config.role_state_db().to_path_buf(),
    )
    .expect("valid terminal manifest");
    assert_ne!(timed_out_manifest, terminal_manifest);
    assert_ne!(
        timed_out_manifest.state_database_path(),
        terminal_manifest.state_database_path()
    );

    let database = root.path().join("maker.sqlite3");
    let mut store = SqliteSwapStore::open(&database).expect("open isolated coordinator database");
    for (id, manifest) in [
        (timed_out_id, &timed_out_manifest),
        (terminal_id, &terminal_manifest),
    ] {
        store.save(&swap(id)).expect("save disjoint ZEC swap");
        store
            .register_maker_actor(manifest, 0)
            .expect("register disjoint actor row");
    }
    drop(store);

    let runtime = root.path().join("runtime");
    fs::create_dir(&runtime).expect("create runtime directory");
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).expect("owner-only runtime");
    let socket = runtime.join("maker.sock");
    let ready = runtime.join("ready");
    let mut daemon = TestDaemon::spawn_with_workers(&database, &socket, &ready, 30_000, 600, 2);
    wait_for_file(
        &mut daemon,
        &ready,
        Duration::from_secs(10),
        "two-swap daemon readiness",
    );
    wait_for_file(
        &mut daemon,
        &timed_out_pid_file,
        Duration::from_secs(5),
        "timed-out actor identity",
    );
    let child_pid: u32 = fs::read_to_string(&timed_out_pid_file)
        .expect("read timed-out actor PID")
        .trim()
        .parse()
        .expect("numeric timed-out actor PID");

    let leased = SqliteSwapStore::open(&database)
        .expect("open observer while timed-out actor is running")
        .list_maker_actor_processes()
        .expect("inspect leased timed-out actor");
    let leased = leased
        .iter()
        .find(|record| record.swap_id().as_str() == timed_out_id)
        .expect("leased timed-out row");
    assert_eq!(leased.schedule_state(), MakerActorScheduleState::Leased);
    assert_eq!(
        leased.child_identity().map(|identity| identity.0),
        Some(child_pid)
    );
    let health = command_output_with_timeout(
        Command::new(env!("CARGO_BIN_EXE_lez-maker"))
            .arg("--socket")
            .arg(&socket)
            .arg("health"),
        Duration::from_secs(1),
        "owner health while timed-out peer is leased",
    );
    assert!(
        health.status.success(),
        "owner health must remain responsive while the actor is leased"
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&health.stdout).expect("health JSON")["ready"],
        true
    );

    let overlap_deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let records = SqliteSwapStore::open(&database)
            .expect("open independent overlap observer")
            .list_maker_actor_processes()
            .expect("inspect overlapping actor rows");
        let timed_out = records
            .iter()
            .find(|record| record.swap_id().as_str() == timed_out_id)
            .expect("timed-out overlap row");
        let terminal = records
            .iter()
            .find(|record| record.swap_id().as_str() == terminal_id)
            .expect("terminal overlap row");
        if timed_out.schedule_state() == MakerActorScheduleState::Leased
            && timed_out.child_identity().is_some()
            && terminal.schedule_state() == MakerActorScheduleState::Terminal
        {
            break;
        }
        if let Some(status) = daemon.child_mut().try_wait().expect("poll maker daemon") {
            panic!("maker daemon exited during overlap proof: {status}");
        }
        assert!(
            Instant::now() < overlap_deadline,
            "terminal peer did not finish while timed-out peer remained live: {records:?}"
        );
        thread::sleep(Duration::from_millis(10));
    }

    write_private(&timed_out_release, b"release\n", 0o600);
    let deadline = Instant::now() + Duration::from_secs(10);
    let durable = loop {
        let records = SqliteSwapStore::open(&database)
            .expect("open independent two-swap observer")
            .list_maker_actor_processes()
            .expect("inspect two actor rows");
        let timed_out = records
            .iter()
            .find(|record| record.swap_id().as_str() == timed_out_id)
            .expect("timed-out row");
        let terminal = records
            .iter()
            .find(|record| record.swap_id().as_str() == terminal_id)
            .expect("terminal row");
        if timed_out.schedule_state() == MakerActorScheduleState::Backoff
            && terminal.schedule_state() == MakerActorScheduleState::Terminal
        {
            break records;
        }
        if let Some(status) = daemon.child_mut().try_wait().expect("poll maker daemon") {
            panic!("maker daemon exited during two-swap journey: {status}");
        }
        assert!(
            Instant::now() < deadline,
            "timed-out and terminal peers did not resolve independently: {records:?}"
        );
        thread::sleep(Duration::from_millis(10));
    };
    let timed_out = durable
        .iter()
        .find(|record| record.swap_id().as_str() == timed_out_id)
        .unwrap();
    let terminal = durable
        .iter()
        .find(|record| record.swap_id().as_str() == terminal_id)
        .unwrap();
    assert_eq!(timed_out.attempt_count(), 1);
    assert_eq!(terminal.attempt_count(), 1);
    assert_eq!(timed_out.child_identity(), None);
    assert_eq!(terminal.child_identity(), None);
    assert_eq!(timed_out.manifest(), &timed_out_manifest);
    assert_eq!(terminal.manifest(), &terminal_manifest);
    assert!(
        !Path::new("/proc").join(child_pid.to_string()).exists(),
        "timed-out child must be killed and reaped after its peer completes"
    );

    assert!(daemon.terminate(Duration::from_secs(2)).success());
    assert!(
        !socket.exists(),
        "first daemon must remove its owner socket"
    );
    assert!(
        !ready.exists(),
        "first daemon must remove its readiness file"
    );

    let mut restarted = TestDaemon::spawn_with_workers(&database, &socket, &ready, 30_000, 600, 2);
    wait_for_file(
        &mut restarted,
        &ready,
        Duration::from_secs(10),
        "restarted two-swap daemon readiness",
    );
    thread::sleep(Duration::from_millis(300));
    let reopened = SqliteSwapStore::open(&database)
        .expect("reopen durable two-swap coordinator")
        .list_maker_actor_processes()
        .expect("inspect durable rows after restart");
    assert_eq!(reopened, durable);
    assert_eq!(
        fs::read_to_string(&timed_out_invocations).expect("timeout invocation log"),
        "status\n"
    );
    assert_eq!(
        fs::read_to_string(&terminal_invocations).expect("terminal invocation log"),
        "status\n"
    );
    assert!(restarted.terminate(Duration::from_secs(2)).success());
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
        Self::spawn_with_limits(database, socket, ready, 30_000, 30)
    }

    fn spawn_with_limits(
        database: &Path,
        socket: &Path,
        ready: &Path,
        attempt_timeout_milliseconds: u64,
        failure_backoff_seconds: u64,
    ) -> Self {
        Self::spawn_with_workers(
            database,
            socket,
            ready,
            attempt_timeout_milliseconds,
            failure_backoff_seconds,
            1,
        )
    }

    fn spawn_with_workers(
        database: &Path,
        socket: &Path,
        ready: &Path,
        attempt_timeout_milliseconds: u64,
        failure_backoff_seconds: u64,
        worker_count: u16,
    ) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_lez-maker-daemon"))
            .arg("--socket")
            .arg(socket)
            .arg("--database")
            .arg(database)
            .arg("--ready-file")
            .arg(ready)
            .arg("--actor-supervisor")
            .arg("--actor-worker-count")
            .arg(worker_count.to_string())
            .arg("--actor-poll-milliseconds")
            .arg("10")
            .arg("--actor-attempt-timeout-milliseconds")
            .arg(attempt_timeout_milliseconds.to_string())
            .arg("--actor-failure-backoff-seconds")
            .arg(failure_backoff_seconds.to_string())
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
