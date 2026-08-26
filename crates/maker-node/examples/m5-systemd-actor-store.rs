#[path = "../tests/support/mod.rs"]
mod support;

use std::{
    env, fs,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
};

use lez_swap_core::{
    Chain, ChainPosition, ConfirmationPolicy, Pair, RecoverySchedule, SwapCoordinator,
    SwapDirection, SwapId, TimelockSafety,
};
use lez_swap_store::{
    MakerActorKindV1, MakerActorManifestV1, MakerActorScheduleState, SqliteSwapStore,
    validate_maker_actor_program,
};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use support::actor_deployment;
use zec_reference_actor::ActorConfig;

const CRASH_SWAP_ID: &str = "aaa-systemd-submitted-effect";
const PEER_SWAP_ID: &str = "zzz-systemd-disjoint-peer";

fn main() {
    let arguments = env::args_os().skip(1).collect::<Vec<_>>();
    let output = match arguments.as_slice() {
        [command, root, database, program] if command == "setup" => {
            setup(Path::new(root), Path::new(database), Path::new(program))
        }
        [command, database] if command == "inspect" => inspect(Path::new(database)),
        _ => panic!("usage: m5-systemd-actor-store setup ROOT DATABASE PROGRAM | inspect DATABASE"),
    };
    println!(
        "{}",
        serde_json::to_string(&output).expect("serialize fixture evidence")
    );
}

fn setup(root: &Path, database: &Path, program: &Path) -> Value {
    assert!(root.is_absolute() && database.is_absolute() && program.is_absolute());
    let program = fs::canonicalize(program).expect("canonical fixture actor program");
    let program_sha256: [u8; 32] = Sha256::digest(fs::read(&program).unwrap()).into();
    validate_maker_actor_program(&program, program_sha256).expect("valid fixture actor program");
    let crash_root = private_subdirectory(root, "crash");
    let peer_root = private_subdirectory(root, "peer");
    let crash = actor_deployment(&crash_root, CRASH_SWAP_ID);
    let peer = actor_deployment(&peer_root, PEER_SWAP_ID);
    let mut store = SqliteSwapStore::open(database).expect("open isolated scheduler database");
    for (id, deployment) in [(CRASH_SWAP_ID, &crash), (PEER_SWAP_ID, &peer)] {
        let config_bytes = fs::read(&deployment.source_config).unwrap();
        let config = ActorConfig::load_private(&deployment.source_config).unwrap();
        store.save(&swap(id)).expect("save valid ZEC coordinator");
        store
            .register_maker_actor(
                &MakerActorManifestV1::new(
                    SwapId::new(id).unwrap(),
                    MakerActorKindV1::Zcash,
                    deployment.source_config.clone(),
                    Sha256::digest(config_bytes).into(),
                    program.clone(),
                    program_sha256,
                    config.role_state_db().to_path_buf(),
                )
                .expect("valid scheduler fixture manifest"),
                0,
            )
            .expect("register scheduler fixture actor");
    }
    let crash_config = ActorConfig::load_private(&crash.source_config).unwrap();
    json!({
        "schema_version": 1,
        "source_config": crash.source_config,
        "actor_root": crash.root,
        "crash_swap_id": CRASH_SWAP_ID,
        "peer_swap_id": PEER_SWAP_ID,
        "effect_file": effect_path(crash_config.role_state_db()),
        "program_sha256": hex::encode(program_sha256),
        "runtime_external_resources": "none"
    })
}

fn inspect(database: &Path) -> Value {
    let store = SqliteSwapStore::open(database).expect("reopen scheduler evidence database");
    let actors = store
        .list_maker_actor_processes()
        .expect("list scheduler evidence")
        .into_iter()
        .map(|record| {
            let state = match record.schedule_state() {
                MakerActorScheduleState::Queued => "queued",
                MakerActorScheduleState::Leased => "leased",
                MakerActorScheduleState::Backoff => "backoff",
                MakerActorScheduleState::Terminal => "terminal",
                MakerActorScheduleState::Failed => "failed",
            };
            json!({
                "swap_id": record.swap_id().as_str(),
                "schedule_state": state,
                "lease_generation": record.lease_generation(),
                "attempt_count": record.attempt_count(),
                "child_identity": record.child_identity().map(|(pid, start_ticks)| json!({
                    "pid": pid, "start_ticks": start_ticks
                }))
            })
        })
        .collect::<Vec<_>>();
    json!({"schema_version": 1, "actors": actors})
}

fn private_subdirectory(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    fs::DirBuilder::new()
        .create(&path)
        .expect("create fixture actor root");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}

fn effect_path(state_database: &Path) -> PathBuf {
    state_database.with_extension("m5-effect.json")
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
