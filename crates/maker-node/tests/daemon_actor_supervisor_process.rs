#[allow(dead_code)]
#[path = "support/btc_fixture.rs"]
mod btc_fixture;
mod support;
#[allow(dead_code)]
#[path = "support/xmr_chat_fixture.rs"]
mod xmr_chat_fixture;

use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _},
    path::Path,
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

use btc_fixture::BtcAuthorityFixture;
use btc_reference_actor::ActorConfig as BtcActorConfig;
use lez_bridge_protocol::RequestId;
use lez_swap_core::{
    Chain, ChainPosition, ConfirmationPolicy, Pair, RecoverySchedule, SwapCoordinator,
    SwapDirection, SwapId, TimelockSafety,
};
use lez_swap_store::{
    MakerActorKindV1, MakerActorManifestV1, MakerActorManualAction, MakerActorScheduleState,
    SqliteSwapStore,
};
use rustix::process::{Pid, Signal, kill_process};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use tempfile::tempdir;
use zec_reference_actor::ActorConfig;

use support::actor_deployment;
use xmr_chat_fixture::XmrChatFixture;

static DAEMON_SUPERVISOR_PROCESS_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
#[allow(clippy::too_many_lines)] // One process journey keeps readiness, RPC, and reap ordering visible.
fn enabled_daemon_supervises_actor_without_blocking_health_and_cancels_on_sigterm() {
    let _process_test_guard = DAEMON_SUPERVISOR_PROCESS_TEST_LOCK
        .lock()
        .expect("daemon-supervisor process test lock");
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
    store.save(&zec_swap(swap_id)).expect("save ZEC swap");
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
#[allow(clippy::too_many_lines)] // One process journey keeps all three durable actor rows visible.
fn daemon_runs_overlapping_actors_and_isolates_failing_peer_across_restart() {
    let _process_test_guard = DAEMON_SUPERVISOR_PROCESS_TEST_LOCK
        .lock()
        .expect("daemon-supervisor process test lock");
    let root = tempdir().expect("isolated three-pair daemon root");
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700))
        .expect("owner-only test root");
    let xmr_bytes = [0x58; 32];
    let xmr_id = hex::encode(xmr_bytes);
    let btc_bytes = [0x42; 32];
    let btc_id = hex::encode(btc_bytes);
    let zec_id = "m5-daemon-zec-terminal";
    let xmr_root = root.path().join("xmr-unavailable");
    let btc_root = root.path().join("btc-terminal");
    let zec_root = root.path().join("zec-terminal");
    for directory in [&xmr_root, &btc_root, &zec_root] {
        fs::create_dir(directory).expect("create disjoint actor fixture root");
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
            .expect("owner-only actor fixture root");
    }
    let zec_deployment = actor_deployment(&zec_root, zec_id);
    let zec_config = ActorConfig::load_private(&zec_deployment.source_config).expect("ZEC config");

    let xmr_pid_file = root.path().join("xmr-unavailable.pid");
    let xmr_release = root.path().join("xmr-unavailable.release");
    let xmr_invocations = root.path().join("xmr-unavailable.invocations");
    let xmr_program_path = root.path().join("xmr-unavailable-maker-actor");
    let xmr_program = format!(
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
        xmr_invocations.display(),
        xmr_pid_file.display(),
        xmr_release.display()
    );
    write_private(&xmr_program_path, xmr_program.as_bytes(), 0o700);
    let xmr_fixture =
        XmrChatFixture::new(&xmr_root, xmr_bytes, 1_000_000, 25_000, &xmr_program_path);

    let zec_invocations = root.path().join("terminal.invocations");
    let zec_program_path = root.path().join("terminal-zec-maker-actor");
    let zec_program = format!(
        "#!/bin/sh\n\
         test \"$1\" = \"--config-fd\" || exit 91\n\
         test \"$2\" = \"196\" || exit 92\n\
         test -r /proc/self/fd/196 || exit 93\n\
         test -r /proc/self/fd/198 || exit 94\n\
         test \"$3\" = \"status\" || exit 95\n\
         printf '%s\\n' \"$3\" >> \"{}\"\n\
         printf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"state\":\"active\",\"phase\":\"completed\",\"revision\":4,\"next_action\":\"complete\"}}'\n",
        zec_invocations.display()
    );
    write_private(&zec_program_path, zec_program.as_bytes(), 0o700);

    let btc_fixture = BtcAuthorityFixture::new(&btc_root, "daemon-concurrency", btc_bytes);
    let btc_config =
        BtcActorConfig::load_private(&btc_fixture.maker_source_config).expect("BTC Maker config");
    let btc_invocations = root.path().join("btc-terminal.invocations");
    let btc_program_path = root.path().join("terminal-btc-maker-actor");
    let btc_program = format!(
        "#!/bin/sh\ntest \"$1\" = \"--config-fd\" || exit 91\ntest \"$2\" = \"196\" || exit 92\ntest -r /proc/self/fd/196 || exit 93\ntest -r /proc/self/fd/198 || exit 94\ntest \"$3\" = \"status\" || exit 95\nprintf '%s\\n' \"$3\" >> \"{}\"\nprintf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"state\":\"active\",\"phase\":\"completed\",\"revision\":4,\"next_action\":\"complete\"}}'\n",
        btc_invocations.display()
    );
    write_private(&btc_program_path, btc_program.as_bytes(), 0o700);

    // These marker programs prove process and control-plane isolation only. They do not
    // contact chain nodes, classify finality, or provide cross-chain execution evidence.
    let xmr_manifest = MakerActorManifestV1::new(
        SwapId::new(xmr_id.clone()).unwrap(),
        MakerActorKindV1::Monero,
        xmr_fixture.maker_actor_config.clone(),
        Sha256::digest(fs::read(&xmr_fixture.maker_actor_config).expect("read XMR config")).into(),
        xmr_program_path,
        Sha256::digest(xmr_program.as_bytes()).into(),
        xmr_fixture.maker_actor_state,
    )
    .expect("valid XMR unavailable manifest");
    let btc_manifest = MakerActorManifestV1::new(
        SwapId::new(btc_id.clone()).unwrap(),
        MakerActorKindV1::Bitcoin,
        btc_fixture.maker_source_config.clone(),
        Sha256::digest(fs::read(&btc_fixture.maker_source_config).expect("read BTC config")).into(),
        btc_program_path,
        Sha256::digest(btc_program.as_bytes()).into(),
        btc_config.state_db().to_path_buf(),
    )
    .expect("valid BTC terminal manifest");
    let zec_manifest = MakerActorManifestV1::new(
        SwapId::new(zec_id).unwrap(),
        MakerActorKindV1::Zcash,
        zec_deployment.source_config.clone(),
        Sha256::digest(fs::read(&zec_deployment.source_config).expect("read ZEC config")).into(),
        zec_program_path,
        Sha256::digest(zec_program.as_bytes()).into(),
        zec_config.role_state_db().to_path_buf(),
    )
    .expect("valid ZEC terminal manifest");
    assert_ne!(xmr_manifest, btc_manifest);
    assert_ne!(xmr_manifest, zec_manifest);
    assert_ne!(btc_manifest, zec_manifest);
    let state_paths = [
        xmr_manifest.state_database_path(),
        btc_manifest.state_database_path(),
        zec_manifest.state_database_path(),
    ];
    assert!(state_paths[0] != state_paths[1]);
    assert!(state_paths[0] != state_paths[2]);
    assert!(state_paths[1] != state_paths[2]);

    let database = root.path().join("maker.sqlite3");
    let mut store = SqliteSwapStore::open(&database).expect("open isolated coordinator database");
    for (coordinator, manifest) in [
        (xmr_swap(&xmr_id), &xmr_manifest),
        (btc_swap(&btc_id), &btc_manifest),
        (zec_swap(zec_id), &zec_manifest),
    ] {
        store.save(&coordinator).expect("save pair-correct swap");
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
    let mut daemon = TestDaemon::spawn_with_workers(&database, &socket, &ready, 30_000, 600, 3);
    wait_for_file(
        &mut daemon,
        &ready,
        Duration::from_secs(10),
        "three-pair daemon readiness",
    );
    wait_for_file(
        &mut daemon,
        &xmr_pid_file,
        Duration::from_secs(5),
        "xmr-unavailable actor identity",
    );
    let child_pid: u32 = fs::read_to_string(&xmr_pid_file)
        .expect("read xmr-unavailable actor PID")
        .trim()
        .parse()
        .expect("numeric xmr-unavailable actor PID");

    let leased = SqliteSwapStore::open(&database)
        .expect("open observer while xmr-unavailable actor is running")
        .list_maker_actor_processes()
        .expect("inspect leased xmr-unavailable actor");
    let leased = leased
        .iter()
        .find(|record| record.swap_id().as_str() == xmr_id.as_str())
        .expect("leased xmr-unavailable row");
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
        "owner health while xmr-unavailable peer is leased",
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
        let xmr = records
            .iter()
            .find(|record| record.swap_id().as_str() == xmr_id.as_str())
            .expect("XMR unavailable overlap row");
        let btc = records
            .iter()
            .find(|record| record.swap_id().as_str() == btc_id.as_str())
            .expect("BTC terminal overlap row");
        let zec = records
            .iter()
            .find(|record| record.swap_id().as_str() == zec_id)
            .expect("ZEC terminal overlap row");
        if xmr.schedule_state() == MakerActorScheduleState::Leased
            && xmr.child_identity().is_some()
            && btc.schedule_state() == MakerActorScheduleState::Terminal
            && zec.schedule_state() == MakerActorScheduleState::Terminal
        {
            break;
        }
        if let Some(status) = daemon.child_mut().try_wait().expect("poll maker daemon") {
            panic!("maker daemon exited during overlap proof: {status}");
        }
        assert!(
            Instant::now() < overlap_deadline,
            "terminal peer did not finish while xmr-unavailable peer remained live: {records:?}"
        );
        thread::sleep(Duration::from_millis(10));
    }

    write_private(&xmr_release, b"release\n", 0o600);
    let deadline = Instant::now() + Duration::from_secs(10);
    let durable = loop {
        let records = SqliteSwapStore::open(&database)
            .expect("open independent three-pair observer")
            .list_maker_actor_processes()
            .expect("inspect three actor rows");
        let xmr = records
            .iter()
            .find(|record| record.swap_id().as_str() == xmr_id.as_str())
            .expect("XMR unavailable row");
        let btc = records
            .iter()
            .find(|record| record.swap_id().as_str() == btc_id.as_str())
            .expect("BTC terminal row");
        let zec = records
            .iter()
            .find(|record| record.swap_id().as_str() == zec_id)
            .expect("ZEC terminal row");
        if xmr.schedule_state() == MakerActorScheduleState::Backoff
            && btc.schedule_state() == MakerActorScheduleState::Terminal
            && zec.schedule_state() == MakerActorScheduleState::Terminal
        {
            break records;
        }
        if let Some(status) = daemon.child_mut().try_wait().expect("poll maker daemon") {
            panic!("maker daemon exited during three-pair journey: {status}");
        }
        assert!(
            Instant::now() < deadline,
            "three pair rows did not resolve independently: {records:?}"
        );
        thread::sleep(Duration::from_millis(10));
    };
    let xmr = durable
        .iter()
        .find(|record| record.swap_id().as_str() == xmr_id.as_str())
        .unwrap();
    let btc = durable
        .iter()
        .find(|record| record.swap_id().as_str() == btc_id.as_str())
        .unwrap();
    let zec = durable
        .iter()
        .find(|record| record.swap_id().as_str() == zec_id)
        .unwrap();
    for record in [xmr, btc, zec] {
        assert_eq!(record.attempt_count(), 1);
        assert_eq!(record.child_identity(), None);
    }
    assert_eq!(xmr.manifest(), &xmr_manifest);
    assert_eq!(btc.manifest(), &btc_manifest);
    assert_eq!(zec.manifest(), &zec_manifest);
    assert!(
        !Path::new("/proc").join(child_pid.to_string()).exists(),
        "xmr-unavailable child must be killed and reaped after its peer completes"
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

    let mut restarted = TestDaemon::spawn_with_workers(&database, &socket, &ready, 30_000, 600, 3);
    wait_for_file(
        &mut restarted,
        &ready,
        Duration::from_secs(10),
        "restarted three-pair daemon readiness",
    );
    thread::sleep(Duration::from_millis(300));
    let reopened = SqliteSwapStore::open(&database)
        .expect("reopen durable three-pair coordinator")
        .list_maker_actor_processes()
        .expect("inspect durable rows after restart");
    assert_eq!(reopened, durable);
    assert_eq!(
        fs::read_to_string(&xmr_invocations).expect("timeout invocation log"),
        "status\n"
    );
    assert_eq!(
        fs::read_to_string(&zec_invocations).expect("terminal invocation log"),
        "status\n"
    );
    assert_eq!(
        fs::read_to_string(&btc_invocations).expect("BTC terminal invocation log"),
        "status\n"
    );
    assert!(restarted.terminate(Duration::from_secs(2)).success());
}

#[test]
#[allow(clippy::too_many_lines)] // The lease/restart/terminal barriers are one concurrency proof.
fn daemon_leases_two_accepted_xmr_applications_concurrently_across_restart() {
    let _process_test_guard = DAEMON_SUPERVISOR_PROCESS_TEST_LOCK
        .lock()
        .expect("daemon-supervisor process test lock");
    let root = tempdir().expect("isolated two-XMR daemon root");
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700))
        .expect("owner-only test root");

    let application_specs = [
        ("accepted-xmr-a", [0x61; 32], 1_000_000, 25_000),
        ("accepted-xmr-b", [0x62; 32], 2_000_000, 26_000),
    ];
    let mut manifests = Vec::new();
    let mut swap_ids = Vec::new();
    let mut pid_files = Vec::new();
    let mut release_files = Vec::new();
    let mut invocation_files = Vec::new();
    let mut config_paths = Vec::new();
    let mut state_paths = Vec::new();

    for (label, swap_bytes, foreign_units, lez_units) in application_specs {
        let fixture_root = root.path().join(label);
        fs::create_dir(&fixture_root).expect("create accepted XMR fixture root");
        fs::set_permissions(&fixture_root, fs::Permissions::from_mode(0o700))
            .expect("owner-only accepted XMR fixture root");
        let pid_file = root.path().join(format!("{label}.pid"));
        let release_file = root.path().join(format!("{label}.release"));
        let invocation_file = root.path().join(format!("{label}.invocations"));
        let program_path = root.path().join(format!("{label}-actor"));
        let program = format!(
            "#!/bin/sh\n\
             test \"$1\" = \"--config-fd\" || exit 91\n\
             test \"$2\" = \"196\" || exit 92\n\
             test -r /proc/self/fd/196 || exit 93\n\
             test -r /proc/self/fd/198 || exit 94\n\
             printf '%s\\n' \"$3\" >> \"{}\"\n\
             case \"$3\" in\n\
               status) printf '%s\\n' \"$$\" > \"{}\"; while test ! -f \"{}\"; do /usr/bin/sleep 0.01; done; printf '%s\\n' '{{\"schema_version\":1,\"actor_program\":\"xmr-maker-actor\",\"actor_abi\":\"lez_maker_xmr_pre_effect_v1\",\"role\":\"maker\",\"state\":\"active\",\"phase\":\"offered\",\"revision\":0,\"next_action\":\"xmr_chain_effects_not_yet_composed\",\"chain_effect_executed\":false}}' ;;\n\
               claim) printf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"command\":\"claim\",\"outcome\":\"completed\",\"phase\":\"completed\",\"revision\":4,\"next_action\":\"complete\"}}' ;;\n\
               *) exit 95 ;;\n\
             esac\n",
            invocation_file.display(),
            pid_file.display(),
            release_file.display()
        );
        write_private(&program_path, program.as_bytes(), 0o700);
        let fixture = XmrChatFixture::new(
            &fixture_root,
            swap_bytes,
            foreign_units,
            lez_units,
            &program_path,
        );
        let swap_id = hex::encode(swap_bytes);
        let manifest = MakerActorManifestV1::new(
            SwapId::new(swap_id.clone()).unwrap(),
            MakerActorKindV1::Monero,
            fixture.maker_actor_config.clone(),
            Sha256::digest(fs::read(&fixture.maker_actor_config).unwrap()).into(),
            program_path,
            Sha256::digest(program.as_bytes()).into(),
            fixture.maker_actor_state.clone(),
        )
        .expect("valid accepted XMR actor manifest");
        config_paths.push(fixture.maker_actor_config);
        state_paths.push(fixture.maker_actor_state);
        manifests.push(manifest);
        swap_ids.push(swap_id);
        pid_files.push(pid_file);
        release_files.push(release_file);
        invocation_files.push(invocation_file);
    }

    assert_ne!(
        config_paths[0], config_paths[1],
        "distinct XMR actor configurations"
    );
    assert_ne!(
        state_paths[0], state_paths[1],
        "distinct XMR actor state databases"
    );

    let database = root.path().join("maker.sqlite3");
    let mut store = SqliteSwapStore::open(&database).expect("open shared Maker database");
    for (index, (swap_id, manifest)) in swap_ids.iter().zip(&manifests).enumerate() {
        let id = SwapId::new(swap_id.clone()).unwrap();
        store
            .save(&xmr_swap(swap_id))
            .expect("save authenticated XMR application row");
        store
            .register_maker_actor(manifest, 0)
            .expect("register isolated XMR actor row");
        store
            .queue_maker_actor_manual_action(
                &RequestId::new(format!("m7-xmr-concurrency-{index}")).unwrap(),
                &id,
                MakerActorManualAction::Claim,
                0,
                0,
            )
            .expect("queue owner XMR Claim action");
    }
    drop(store);

    let runtime = root.path().join("runtime");
    fs::create_dir(&runtime).expect("create runtime directory");
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).expect("owner-only runtime");
    let socket = runtime.join("maker.sock");
    let ready = runtime.join("ready");
    let mut daemon = TestDaemon::spawn_with_workers(&database, &socket, &ready, 30_000, 1, 2);
    wait_for_file(
        &mut daemon,
        &ready,
        Duration::from_secs(10),
        "two-XMR daemon readiness",
    );
    for pid_file in &pid_files {
        wait_for_file(
            &mut daemon,
            pid_file,
            Duration::from_secs(10),
            "accepted XMR actor identity",
        );
    }
    let first_pids: Vec<u32> = pid_files
        .iter()
        .map(|path| fs::read_to_string(path).unwrap().trim().parse().unwrap())
        .collect();
    wait_for_concurrent_leases(
        &mut daemon,
        &database,
        Duration::from_secs(10),
        "two accepted XMR applications to hold concurrent leases",
    );

    assert!(daemon.terminate(Duration::from_secs(2)).success());
    for (pid_file, pid) in pid_files.iter().zip(&first_pids) {
        assert!(!Path::new("/proc").join(pid.to_string()).exists());
        fs::remove_file(pid_file).expect("remove first-generation actor identity");
    }
    let interrupted = SqliteSwapStore::open(&database)
        .expect("reopen interrupted XMR rows")
        .list_maker_actor_processes()
        .expect("inspect interrupted XMR rows");
    assert!(interrupted.iter().all(|record| {
        record.schedule_state() != MakerActorScheduleState::Leased
            && record.child_identity().is_none()
    }));

    let mut restarted = TestDaemon::spawn_with_workers(&database, &socket, &ready, 30_000, 1, 2);
    wait_for_file(
        &mut restarted,
        &ready,
        Duration::from_secs(10),
        "restarted two-XMR daemon readiness",
    );
    for pid_file in &pid_files {
        wait_for_file(
            &mut restarted,
            pid_file,
            Duration::from_secs(10),
            "restarted accepted XMR actor identity",
        );
    }
    let second_pids: Vec<u32> = pid_files
        .iter()
        .map(|path| fs::read_to_string(path).unwrap().trim().parse().unwrap())
        .collect();
    assert_ne!(first_pids, second_pids);
    wait_for_concurrent_leases(
        &mut restarted,
        &database,
        Duration::from_secs(10),
        "restarted accepted XMR applications to hold concurrent leases",
    );

    for release_file in &release_files {
        write_private(release_file, b"release\n", 0o600);
    }
    let terminal_deadline = Instant::now() + Duration::from_secs(10);
    let terminal_rows = loop {
        let rows = SqliteSwapStore::open(&database)
            .expect("open terminal concurrency observer")
            .list_maker_actor_processes()
            .expect("inspect terminal XMR rows");
        if rows.iter().all(|record| {
            record.schedule_state() == MakerActorScheduleState::Terminal
                && record.child_identity().is_none()
        }) {
            break rows;
        }
        assert!(
            Instant::now() < terminal_deadline,
            "accepted XMR actors did not terminalize independently: {rows:?}"
        );
        thread::sleep(Duration::from_millis(10));
    };
    assert!(restarted.terminate(Duration::from_secs(2)).success());
    for invocation_file in &invocation_files {
        assert_eq!(
            fs::read_to_string(invocation_file).unwrap(),
            "status\nstatus\nclaim\n"
        );
    }

    let mut terminal_restart =
        TestDaemon::spawn_with_workers(&database, &socket, &ready, 30_000, 1, 2);
    wait_for_file(
        &mut terminal_restart,
        &ready,
        Duration::from_secs(10),
        "terminal two-XMR daemon readiness",
    );
    thread::sleep(Duration::from_millis(300));
    let reopened = SqliteSwapStore::open(&database)
        .expect("reopen terminal XMR database")
        .list_maker_actor_processes()
        .expect("inspect terminal XMR isolation");
    assert_eq!(
        reopened, terminal_rows,
        "accepted XMR restart must preserve terminal isolation"
    );
    for invocation_file in &invocation_files {
        assert_eq!(
            fs::read_to_string(invocation_file).unwrap(),
            "status\nstatus\nclaim\n"
        );
    }
    assert!(terminal_restart.terminate(Duration::from_secs(2)).success());
}

fn xmr_swap(id: &str) -> SwapCoordinator {
    SwapCoordinator::new_with_confirmation_policies(
        SwapId::new(id).unwrap(),
        Pair::Monero,
        SwapDirection::TakerSellsLez,
        ConfirmationPolicy::new(2).unwrap(),
        ConfirmationPolicy::new(10).unwrap(),
        RecoverySchedule::xmr_lez_first(ChainPosition::timestamp(Chain::Lez, 20), 2).unwrap(),
    )
}

fn btc_swap(id: &str) -> SwapCoordinator {
    let direction = SwapDirection::TakerSellsForeign;
    SwapCoordinator::new_with_direction(
        SwapId::new(id).unwrap(),
        Pair::Bitcoin,
        direction,
        ConfirmationPolicy::new(2).unwrap(),
        RecoverySchedule::new(
            Pair::Bitcoin,
            direction,
            ChainPosition::block_height(Chain::Lez, 100),
            ChainPosition::block_height(Chain::Bitcoin, 120),
            TimelockSafety::between(Chain::Lez, Chain::Bitcoin, 1_000, 1_200, 100).unwrap(),
        )
        .unwrap(),
    )
}

fn zec_swap(id: &str) -> SwapCoordinator {
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

fn wait_for_concurrent_leases(
    daemon: &mut TestDaemon,
    database: &Path,
    timeout: Duration,
    description: &str,
) {
    let deadline = Instant::now() + timeout;
    loop {
        let rows = SqliteSwapStore::open(database)
            .expect("open concurrent lease observer")
            .list_maker_actor_processes()
            .expect("inspect concurrent actor leases");
        if rows.len() == 2
            && rows.iter().all(|record| {
                record.schedule_state() == MakerActorScheduleState::Leased
                    && record.child_identity().is_some()
            })
        {
            return;
        }
        if let Some(status) = daemon.child_mut().try_wait().expect("poll maker daemon") {
            panic!("maker daemon exited before {description}: {status}");
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {description}: {rows:?}"
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
