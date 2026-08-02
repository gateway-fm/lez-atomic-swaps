//! Process-boundary matrix for pair-safe Maker lifecycle controls.
//!
//! The registered executable is a marker and the supervisor is deliberately
//! disabled. Assertions cover CLI/RPC routing and durable admission only; they
//! do not claim that Bitcoin, Monero, Zcash, or LEZ chain effects occurred.

use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    process::{Child, Command, Output},
    thread,
    time::{Duration, Instant},
};

use lez_swap_core::{
    Chain, ChainPosition, ConfirmationPolicy, Pair, RecoverySchedule, SwapCoordinator,
    SwapDirection, SwapId, TimelockSafety,
};
use lez_swap_store::{MakerActorKindV1, MakerActorManifestV1, SqliteSwapStore};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use tempfile::tempdir;

struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[derive(Clone, Copy)]
struct LifecycleCase {
    swap_id: &'static str,
    pair: Pair,
    kind: MakerActorKindV1,
    actor_kind: &'static str,
    action: &'static str,
}

const CASES: &[LifecycleCase] = &[
    LifecycleCase {
        swap_id: "m5-btc-maker-claim-route",
        pair: Pair::Bitcoin,
        kind: MakerActorKindV1::Bitcoin,
        actor_kind: "bitcoin",
        action: "claim",
    },
    LifecycleCase {
        swap_id: "m5-btc-maker-refund-route",
        pair: Pair::Bitcoin,
        kind: MakerActorKindV1::Bitcoin,
        actor_kind: "bitcoin",
        action: "refund",
    },
    LifecycleCase {
        swap_id: "m5-xmr-maker-claim-route",
        pair: Pair::Monero,
        kind: MakerActorKindV1::Monero,
        actor_kind: "monero",
        action: "claim",
    },
    LifecycleCase {
        swap_id: "m5-xmr-maker-refund-route",
        pair: Pair::Monero,
        kind: MakerActorKindV1::Monero,
        actor_kind: "monero",
        action: "refund",
    },
    LifecycleCase {
        swap_id: "m5-zec-maker-claim-route",
        pair: Pair::Zcash,
        kind: MakerActorKindV1::Zcash,
        actor_kind: "zcash",
        action: "claim",
    },
    LifecycleCase {
        swap_id: "m5-zec-maker-refund-route",
        pair: Pair::Zcash,
        kind: MakerActorKindV1::Zcash,
        actor_kind: "zcash",
        action: "refund",
    },
];

#[test]
fn maker_actor_lifecycle_control_plane_is_pair_safe_replay_safe_and_restart_durable() {
    let root = tempdir().expect("isolated lifecycle root");
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let database = root.path().join("maker.sqlite3");
    for case in CASES {
        register_marker_actor(root.path(), &database, case);
    }

    let (first_daemon, first_socket) = start_daemon(root.path(), &database, "first");
    for case in CASES {
        assert_initial_monitor(&first_socket, case);
        assert_generation_guard(&first_socket, case);
    }
    let before_restart: Vec<_> = CASES
        .iter()
        .map(|case| (*case, assert_action_route(&first_socket, case)))
        .collect();

    drop(first_daemon);
    let (_second_daemon, second_socket) = start_daemon(root.path(), &database, "second");
    for (case, expected) in before_restart {
        assert_eq!(
            monitor(&second_socket, case.swap_id),
            expected,
            "{} {} admission changed across daemon restart",
            case.actor_kind,
            case.action
        );
        assert_post_restart_replay(&second_socket, &case);
    }
}

fn assert_initial_monitor(socket: &Path, case: &LifecycleCase) {
    let value = monitor(socket, case.swap_id);
    let fields = value.as_object().expect("monitor object");
    assert_eq!(fields.len(), 8);
    for field in [
        "schema_version",
        "swap_id",
        "actor_kind",
        "schedule_state",
        "lease_generation",
        "attempt_count",
        "progress",
        "manual_action",
    ] {
        assert!(fields.contains_key(field), "missing monitor field {field}");
    }
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["swap_id"], case.swap_id);
    assert_eq!(value["actor_kind"], case.actor_kind);
    assert_eq!(value["schedule_state"], "queued");
    assert_eq!(value["lease_generation"], 0);
    assert_eq!(value["attempt_count"], 0);
    assert!(value["progress"].is_null());
    assert!(value["manual_action"].is_null());
}

fn assert_generation_guard(socket: &Path, case: &LifecycleCase) {
    let missing = maker_cli(
        socket,
        &[
            case.action,
            "--id",
            case.swap_id,
            "--request-id",
            "missing-generation",
        ],
    );
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("--expected-generation"));

    let request_id = format!("m5-{}-{}-stale", case.actor_kind, case.action);
    let stale = maker_cli(
        socket,
        &[
            case.action,
            "--id",
            case.swap_id,
            "--request-id",
            &request_id,
            "--expected-generation",
            "1",
        ],
    );
    assert_rpc_conflict(&stale);
}

fn assert_action_route(socket: &Path, case: &LifecycleCase) -> Value {
    let request_id = request_id(case);
    let arguments = [
        case.action,
        "--id",
        case.swap_id,
        "--request-id",
        &request_id,
        "--expected-generation",
        "0",
    ];
    let first = maker_cli(socket, &arguments);
    assert_success(&first);
    let commit: Value = serde_json::from_slice(&first.stdout).expect("action commit JSON");
    assert_eq!(commit.as_object().expect("action commit object").len(), 5);
    assert_eq!(commit["schema_version"], 1);
    assert_eq!(commit["swap_id"], case.swap_id);
    assert_eq!(commit["action"], case.action);
    assert_eq!(commit["requested_after_generation"], 0);
    assert_eq!(commit["was_replay"], false);

    let replay = maker_cli(socket, &arguments);
    assert_success(&replay);
    let replay: Value = serde_json::from_slice(&replay.stdout).expect("action replay JSON");
    assert_eq!(replay["was_replay"], true);

    let conflict = maker_cli(
        socket,
        &[
            case.action,
            "--id",
            case.swap_id,
            "--request-id",
            &request_id,
            "--expected-generation",
            "1",
        ],
    );
    assert_rpc_conflict(&conflict);

    let value = monitor(socket, case.swap_id);
    assert_eq!(value["actor_kind"], case.actor_kind);
    assert_eq!(value["lease_generation"], 0);
    assert_eq!(value["attempt_count"], 0);
    assert_eq!(value["manual_action"]["request_id"], request_id);
    assert_eq!(value["manual_action"]["action"], case.action);
    assert_eq!(value["manual_action"]["state"], "queued");
    assert_eq!(value["manual_action"]["requested_after_generation"], 0);
    assert!(value["manual_action"]["lease_generation"].is_null());
    value
}

fn assert_post_restart_replay(socket: &Path, case: &LifecycleCase) {
    let request_id = request_id(case);
    let replay = maker_cli(
        socket,
        &[
            case.action,
            "--id",
            case.swap_id,
            "--request-id",
            &request_id,
            "--expected-generation",
            "0",
        ],
    );
    assert_success(&replay);
    let replay: Value = serde_json::from_slice(&replay.stdout).expect("post-restart replay JSON");
    assert_eq!(replay["action"], case.action);
    assert_eq!(replay["was_replay"], true);
}

fn register_marker_actor(root: &Path, database: &Path, case: &LifecycleCase) {
    let actor_root = root.join(case.swap_id);
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&actor_root)
        .unwrap();
    let config = actor_root.join("marker-config.json");
    write_private(
        &config,
        format!(
            "{{\"control_plane_marker\":true,\"pair\":\"{}\"}}\n",
            case.actor_kind
        )
        .as_bytes(),
    );
    let program = PathBuf::from("/usr/bin/true").canonicalize().unwrap();
    let state = actor_root.join("marker-state.sqlite3");
    let swap_id = SwapId::new(case.swap_id).unwrap();
    let swap = lifecycle_swap(swap_id.clone(), case.pair);
    let mut store = SqliteSwapStore::open(database).unwrap();
    store.save(&swap).unwrap();
    store
        .register_maker_actor(
            &MakerActorManifestV1::new(
                swap_id,
                case.kind,
                config.clone(),
                Sha256::digest(fs::read(config).unwrap()).into(),
                program.clone(),
                Sha256::digest(fs::read(program).unwrap()).into(),
                state,
            )
            .unwrap(),
            10,
        )
        .unwrap();
}

fn lifecycle_swap(swap_id: SwapId, pair: Pair) -> SwapCoordinator {
    if pair == Pair::Monero {
        return SwapCoordinator::new_with_confirmation_policies(
            swap_id,
            pair,
            SwapDirection::TakerSellsLez,
            ConfirmationPolicy::new(2).unwrap(),
            ConfirmationPolicy::new(10).unwrap(),
            RecoverySchedule::xmr_lez_first(ChainPosition::timestamp(Chain::Lez, 20), 2).unwrap(),
        );
    }
    let foreign = Chain::from(pair);
    let direction = SwapDirection::TakerSellsForeign;
    SwapCoordinator::new_with_direction(
        swap_id,
        pair,
        direction,
        ConfirmationPolicy::new(2).unwrap(),
        RecoverySchedule::new(
            pair,
            direction,
            ChainPosition::block_height(Chain::Lez, 100),
            ChainPosition::block_height(foreign, 120),
            TimelockSafety::between(Chain::Lez, foreign, 1_000, 1_200, 100).unwrap(),
        )
        .unwrap(),
    )
}

fn start_daemon(root: &Path, database: &Path, label: &str) -> (Daemon, PathBuf) {
    let runtime = root.join(format!("{label}-runtime"));
    fs::DirBuilder::new().mode(0o700).create(&runtime).unwrap();
    let socket = runtime.join("maker.sock");
    let ready = runtime.join("ready");
    let child = Command::new(env!("CARGO_BIN_EXE_lez-maker-daemon"))
        .arg("--socket")
        .arg(&socket)
        .arg("--database")
        .arg(database)
        .arg("--ready-file")
        .arg(&ready)
        .spawn()
        .expect("start Maker daemon");
    let mut daemon = Daemon(child);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(published) = fs::read_to_string(&ready) {
            assert_eq!(published.trim(), socket.to_str().unwrap());
            return (daemon, socket);
        }
        if let Some(status) = daemon.0.try_wait().expect("poll Maker daemon") {
            panic!("Maker daemon exited before readiness: {status}");
        }
        assert!(
            Instant::now() < deadline,
            "Maker daemon readiness timed out"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn monitor(socket: &Path, swap_id: &str) -> Value {
    let output = maker_cli(socket, &["monitor", "--id", swap_id]);
    assert_success(&output);
    serde_json::from_slice(&output.stdout).expect("monitor JSON")
}

fn maker_cli(socket: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lez-maker"))
        .arg("--socket")
        .arg(socket)
        .args(arguments)
        .output()
        .expect("run Maker CLI")
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "command failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_rpc_conflict(output: &Output) {
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("-32009"),
        "unexpected conflict error: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn request_id(case: &LifecycleCase) -> String {
    format!("m5-{}-{}-route-001", case.actor_kind, case.action)
}

fn write_private(path: &Path, bytes: &[u8]) {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}
