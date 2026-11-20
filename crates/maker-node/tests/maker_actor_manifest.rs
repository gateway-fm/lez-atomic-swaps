//! Exercises the ZEC reference actor; compiled only with `pair-zec`.
#![cfg(feature = "pair-zec")]

mod support;

use std::path::{Path, PathBuf};

use lez_maker_node::prepare_maker_actor;
use lez_swap_core::{
    Chain, ChainPosition, ConfirmationPolicy, Pair, RecoverySchedule, SwapCoordinator,
    SwapDirection, SwapId, TimelockSafety,
};
use lez_swap_store::{
    MakerActorKindV1, MakerActorManifestV1, MakerActorProcessError, MakerActorProcessRecordV1,
    SqliteSwapStore,
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

#[test]
fn monero_manifest_is_visible_but_execution_fails_closed_without_a_semantic_parser() {
    let root = tempdir().unwrap();
    let deployment = actor_deployment(root.path(), "m5-supervisor-xmr");
    let config = ActorConfig::load_private(&deployment.source_config).unwrap();
    let config_sha256: [u8; 32] =
        Sha256::digest(std::fs::read(&deployment.source_config).unwrap()).into();
    let program_sha256: [u8; 32] = hex::decode(&deployment.program_sha256)
        .unwrap()
        .try_into()
        .unwrap();
    let id = SwapId::new("m5-supervisor-xmr").unwrap();
    let database = root.path().join("xmr-maker.sqlite3");
    let mut store = SqliteSwapStore::open(&database).unwrap();
    store.save(&xmr_swap(id.as_str())).unwrap();
    store
        .register_maker_actor(
            &MakerActorManifestV1::new(
                id,
                MakerActorKindV1::Monero,
                deployment.source_config,
                config_sha256,
                deployment.program,
                program_sha256,
                config.role_state_db().to_path_buf(),
            )
            .unwrap(),
            1,
        )
        .unwrap();
    let record = store.list_maker_actor_processes().unwrap().remove(0);

    assert!(matches!(
        prepare_maker_actor(&record),
        Err(MakerActorProcessError::ArtifactSemanticMismatch)
    ));
}

/// A stored agreement wire re-labelled under `deterministic-local-v1`, a
/// profile of the earlier reveal order: the body's profile byte follows the
/// schema (2 bytes), the swap id (4-byte length and bytes) and the direction.
fn legacy_profile_wire(mut wire: Vec<u8>) -> Vec<u8> {
    let id_length = usize::try_from(u32::from_le_bytes(wire[2..6].try_into().unwrap())).unwrap();
    wire[6 + id_length + 1] = 0;
    wire
}

fn insert_maker_agreement(database: &Path, swap_id: &str, wire: &[u8]) {
    rusqlite::Connection::open(database)
        .unwrap()
        .execute(
            "INSERT INTO zec_sdk_agreements
                 (local_role, swap_id, payload_version, agreement_wire, accepted_at, accepted_revision)
             VALUES ('maker', ?1, 1, ?2, 10, 0)",
            rusqlite::params![swap_id, wire],
        )
        .unwrap();
}

#[test]
fn a_maker_node_refuses_to_start_while_it_holds_an_open_legacy_taker_sells_lez_swap() {
    let root = tempdir().unwrap();
    let database = root.path().join("legacy-maker.sqlite3");
    let mut store = SqliteSwapStore::open(&database).unwrap();
    let wire_for = |swap_id: &str, direction: SwapDirection| {
        let swap_root = root.path().join(swap_id);
        std::fs::create_dir(&swap_root).unwrap();
        support::actor_deployment_with_direction(&swap_root, swap_id, direction);
        std::fs::read(swap_root.join("actor-source").join("agreement-v2.borsh")).unwrap()
    };
    let current = wire_for("legacy-start-current", SwapDirection::TakerSellsLez);
    assert!(!lez_zec_swap_sdk::legacy_taker_sells_lez(&current));
    insert_maker_agreement(&database, "legacy-start-current", &current);
    let forward = legacy_profile_wire(wire_for(
        "legacy-start-forward",
        SwapDirection::TakerSellsForeign,
    ));
    insert_maker_agreement(&database, "legacy-start-forward", &forward);
    lez_maker_node::refuse_start_with_legacy_zec_swaps(&store)
        .expect("current agreements and legacy TakerSellsForeign ones resume");

    // A completed legacy TakerSellsLez swap does not block.
    let completed = legacy_profile_wire(current.clone());
    assert!(lez_zec_swap_sdk::legacy_taker_sells_lez(&completed));
    store.save(&swap("legacy-start-completed")).unwrap();
    store
        .register_maker_actor(
            &MakerActorManifestV1::new(
                SwapId::new("legacy-start-completed").unwrap(),
                MakerActorKindV1::Zcash,
                root.path().join("completed-config.json"),
                [1; 32],
                root.path().join("completed-actor"),
                [2; 32],
                root.path().join("completed-state.sqlite3"),
            )
            .unwrap(),
            1,
        )
        .unwrap();
    let lease = store
        .claim_maker_actor(
            &SwapId::new("legacy-start-completed").unwrap(),
            lez_swap_store::MakerActorLeaseOwner::new([0x61; 16]).unwrap(),
            1,
        )
        .unwrap()
        .unwrap();
    store
        .resolve_maker_actor_attempt(
            &lease,
            lez_swap_store::MakerActorAttemptResolution::Terminal,
            2,
        )
        .unwrap();
    insert_maker_agreement(&database, "legacy-start-completed", &completed);
    lez_maker_node::refuse_start_with_legacy_zec_swaps(&store)
        .expect("a completed legacy swap does not block");

    // Two open ones do, by name.
    for swap_id in ["legacy-start-open-a", "legacy-start-open-b"] {
        insert_maker_agreement(&database, swap_id, &completed);
    }
    let refused = lez_maker_node::refuse_start_with_legacy_zec_swaps(&store)
        .unwrap_err()
        .to_string();
    assert!(
        refused.contains("legacy-start-open-a, legacy-start-open-b")
            && refused.contains("finish or refund them with the previous release first")
            && !refused.contains("legacy-start-completed")
            && !refused.contains("legacy-start-current")
            && !refused.contains("legacy-start-forward"),
        "{refused}"
    );
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
