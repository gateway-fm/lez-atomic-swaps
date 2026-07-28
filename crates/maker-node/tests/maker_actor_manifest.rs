mod support;

use std::path::{Path, PathBuf};

use lez_maker_node::prepare_maker_actor;
use lez_swap_core::{
    Chain, ChainPosition, ConfirmationPolicy, Pair, RecoverySchedule, SwapCoordinator,
    SwapDirection, SwapId, TimelockSafety,
};
use lez_swap_store::{
    MakerActorKindV1, MakerActorManifestV1, MakerActorProcessRecordV1, SqliteSwapStore,
};
use sha2::{Digest as _, Sha256};
use tempfile::tempdir;
use zec_reference_actor::ActorConfig;

use support::actor_deployment;

#[test]
fn zec_manifest_semantics_match_exact_role_swap_and_state_before_spawn() {
    let root = tempdir().unwrap();
    let deployment = actor_deployment(root.path(), "m5-supervisor-zec");
    let config = ActorConfig::load_private(&deployment.source_config).unwrap();
    let config_sha256: [u8; 32] =
        Sha256::digest(std::fs::read(&deployment.source_config).unwrap()).into();
    let program_sha256: [u8; 32] = hex::decode(&deployment.program_sha256)
        .unwrap()
        .try_into()
        .unwrap();

    let exact = record(
        root.path(),
        "exact",
        "m5-supervisor-zec",
        deployment.source_config.clone(),
        config_sha256,
        deployment.program.clone(),
        program_sha256,
        config.role_state_db().to_path_buf(),
    );
    prepare_maker_actor(&exact).expect("exact ZEC manifest semantics");

    let wrong_swap = record(
        root.path(),
        "wrong-swap",
        "m5-supervisor-other",
        deployment.source_config.clone(),
        config_sha256,
        deployment.program.clone(),
        program_sha256,
        config.role_state_db().to_path_buf(),
    );
    assert!(prepare_maker_actor(&wrong_swap).is_err());

    let wrong_state = record(
        root.path(),
        "wrong-state",
        "m5-supervisor-zec",
        deployment.source_config,
        config_sha256,
        deployment.program,
        program_sha256,
        root.path().join("different-state.sqlite3"),
    );
    assert!(prepare_maker_actor(&wrong_state).is_err());
}

#[allow(clippy::too_many_arguments)]
fn record(
    root: &Path,
    label: &str,
    swap_id: &str,
    config_path: PathBuf,
    config_sha256: [u8; 32],
    program_path: PathBuf,
    program_sha256: [u8; 32],
    state_path: PathBuf,
) -> MakerActorProcessRecordV1 {
    let database = root.join(format!("{label}-maker.sqlite3"));
    let mut store = SqliteSwapStore::open(&database).unwrap();
    store.save(&swap(swap_id)).unwrap();
    store
        .register_maker_actor(
            &MakerActorManifestV1::new(
                SwapId::new(swap_id).unwrap(),
                MakerActorKindV1::Zcash,
                config_path,
                config_sha256,
                program_path,
                program_sha256,
                state_path,
            )
            .unwrap(),
            1,
        )
        .unwrap();
    store.list_maker_actor_processes().unwrap().remove(0)
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
