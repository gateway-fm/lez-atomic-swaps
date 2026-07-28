use std::{
    path::{Path, PathBuf},
    sync::{Arc, Barrier},
    thread,
};

use lez_swap_core::{
    Chain, ChainPosition, ConfirmationPolicy, Pair, RecoverySchedule, SwapCoordinator,
    SwapDirection, SwapId, TimelockSafety,
};
use lez_swap_store::{
    MakerActorAttemptResolution, MakerActorKindV1, MakerActorLeaseOwner, MakerActorManifestV1,
    MakerActorProcessError, MakerActorScheduleState, SqliteSwapStore,
};
use tempfile::tempdir;

fn swap(id: &str, pair: Pair) -> SwapCoordinator {
    let foreign = Chain::from(pair);
    let direction = SwapDirection::TakerSellsForeign;
    SwapCoordinator::new_with_direction(
        SwapId::new(id).unwrap(),
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

fn manifest(root: &Path, id: &str, kind: MakerActorKindV1, byte: u8) -> MakerActorManifestV1 {
    MakerActorManifestV1::new(
        SwapId::new(id).unwrap(),
        kind,
        root.join(format!("{id}-actor-config.json")),
        [byte; 32],
        PathBuf::from("/usr/bin/true"),
        [byte.wrapping_add(1); 32],
        root.join(format!("{id}-actor.sqlite3")),
    )
    .unwrap()
}

#[test]
fn registration_is_immutable_exact_replay_and_pair_bound() {
    let root = tempdir().unwrap();
    let database = root.path().join("maker.sqlite3");
    let mut store = SqliteSwapStore::open(&database).unwrap();
    let zec = swap("zec-a", Pair::Zcash);
    store.save(&zec).unwrap();
    let original = manifest(root.path(), "zec-a", MakerActorKindV1::Zcash, 7);

    let first = store.register_maker_actor(&original, 10).unwrap();
    assert!(!first.was_replay());
    let replay = store.register_maker_actor(&original, 11).unwrap();
    assert!(replay.was_replay());
    assert_eq!(store.list_maker_actor_processes().unwrap().len(), 1);

    let changed = manifest(root.path(), "zec-a", MakerActorKindV1::Zcash, 8);
    assert!(matches!(
        store.register_maker_actor(&changed, 12),
        Err(MakerActorProcessError::RegistrationConflict)
    ));

    let missing = manifest(root.path(), "missing", MakerActorKindV1::Zcash, 9);
    assert!(matches!(
        store.register_maker_actor(&missing, 13),
        Err(MakerActorProcessError::MissingSwap)
    ));

    let btc = swap("btc-a", Pair::Bitcoin);
    store.save(&btc).unwrap();
    let wrong = manifest(root.path(), "btc-a", MakerActorKindV1::Zcash, 10);
    assert!(matches!(
        store.register_maker_actor(&wrong, 14),
        Err(MakerActorProcessError::PairMismatch)
    ));
}

#[test]
fn restart_preserves_distinct_leases_while_one_swap_has_one_fenced_owner() {
    let root = tempdir().unwrap();
    let database = root.path().join("maker.sqlite3");
    let mut store = SqliteSwapStore::open(&database).unwrap();
    for (id, kind, pair, byte) in [
        ("btc-a", MakerActorKindV1::Bitcoin, Pair::Bitcoin, 1),
        ("zec-a", MakerActorKindV1::Zcash, Pair::Zcash, 2),
    ] {
        store.save(&swap(id, pair)).unwrap();
        store
            .register_maker_actor(&manifest(root.path(), id, kind, byte), 10)
            .unwrap();
    }
    let owner_a = MakerActorLeaseOwner::new([1; 16]).unwrap();
    let owner_b = MakerActorLeaseOwner::new([2; 16]).unwrap();
    let due = store.list_due_maker_actor_ids(10, 8).unwrap();
    assert_eq!(
        due,
        vec![SwapId::new("btc-a").unwrap(), SwapId::new("zec-a").unwrap()]
    );

    let btc = store
        .claim_maker_actor(&SwapId::new("btc-a").unwrap(), owner_a, 10)
        .unwrap()
        .unwrap();
    let zec = store
        .claim_maker_actor(&SwapId::new("zec-a").unwrap(), owner_b, 10)
        .unwrap()
        .unwrap();
    assert_eq!(btc.generation(), 1);
    assert_eq!(zec.generation(), 1);
    assert!(
        store
            .claim_maker_actor(&SwapId::new("btc-a").unwrap(), owner_b, u64::MAX)
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .list_due_maker_actor_ids(u64::MAX, 8)
            .unwrap()
            .is_empty()
    );

    store.record_maker_actor_child(&btc, 41, 4_100).unwrap();
    store.record_maker_actor_child(&zec, 42, 4_200).unwrap();
    drop(store);

    let mut reopened = SqliteSwapStore::open(&database).unwrap();
    let leased = reopened.list_leased_maker_actors().unwrap();
    assert_eq!(leased.len(), 2);
    assert_eq!(
        leased[0].record().schedule_state(),
        MakerActorScheduleState::Leased
    );
    assert!(
        reopened
            .claim_maker_actor(&SwapId::new("btc-a").unwrap(), owner_b, u64::MAX)
            .unwrap()
            .is_none(),
        "time alone must never steal a lease"
    );

    let forged_owner = btc.with_owner(owner_b);
    assert!(matches!(
        reopened.resolve_maker_actor_attempt(
            &forged_owner,
            MakerActorAttemptResolution::Requeue { not_before: 20 },
            15,
        ),
        Err(MakerActorProcessError::LeaseConflict)
    ));
    reopened
        .resolve_maker_actor_attempt(&zec, MakerActorAttemptResolution::Terminal, 15)
        .unwrap();
    let records = reopened.list_maker_actor_processes().unwrap();
    let btc_record = records
        .iter()
        .find(|row| row.swap_id().as_str() == "btc-a")
        .unwrap();
    let zec_record = records
        .iter()
        .find(|row| row.swap_id().as_str() == "zec-a")
        .unwrap();
    assert_eq!(btc_record.schedule_state(), MakerActorScheduleState::Leased);
    assert_eq!(
        zec_record.schedule_state(),
        MakerActorScheduleState::Terminal
    );
}

#[test]
fn normal_requeue_and_generation_fence_preserve_peer_state() {
    let root = tempdir().unwrap();
    let database = root.path().join("maker.sqlite3");
    let mut store = SqliteSwapStore::open(&database).unwrap();
    for (id, byte) in [("zec-a", 1), ("zec-b", 2)] {
        store.save(&swap(id, Pair::Zcash)).unwrap();
        store
            .register_maker_actor(&manifest(root.path(), id, MakerActorKindV1::Zcash, byte), 5)
            .unwrap();
    }
    let owner = MakerActorLeaseOwner::new([3; 16]).unwrap();
    let a = store
        .claim_maker_actor(&SwapId::new("zec-a").unwrap(), owner, 5)
        .unwrap()
        .unwrap();
    let b = store
        .claim_maker_actor(&SwapId::new("zec-b").unwrap(), owner, 5)
        .unwrap()
        .unwrap();
    store.record_maker_actor_child(&a, 51, 5_100).unwrap();
    store.record_maker_actor_child(&b, 52, 5_200).unwrap();
    store
        .resolve_maker_actor_attempt(
            &b,
            MakerActorAttemptResolution::Backoff {
                not_before: 30,
                failure_class: "dependency_unavailable".into(),
            },
            20,
        )
        .unwrap();

    store
        .resolve_maker_actor_attempt(
            &a,
            MakerActorAttemptResolution::Requeue { not_before: 25 },
            25,
        )
        .unwrap();
    assert!(store.list_due_maker_actor_ids(24, 8).unwrap().is_empty());
    let due = store.list_due_maker_actor_ids(25, 8).unwrap();
    assert_eq!(due, vec![SwapId::new("zec-a").unwrap()]);
    let next = store
        .claim_maker_actor(&due[0], MakerActorLeaseOwner::new([4; 16]).unwrap(), 25)
        .unwrap()
        .unwrap();
    assert_eq!(next.generation(), 2);
    assert!(matches!(
        store.resolve_maker_actor_attempt(
            &a,
            MakerActorAttemptResolution::Failed {
                failure_class: "stale".into(),
            },
            26,
        ),
        Err(MakerActorProcessError::LeaseConflict)
    ));

    assert!(store.list_due_maker_actor_ids(29, 8).unwrap().is_empty());
    assert_eq!(
        store.list_due_maker_actor_ids(30, 8).unwrap(),
        vec![SwapId::new("zec-b").unwrap()]
    );
}

#[test]
fn competing_connections_exact_replay_registration_and_fence_same_swap() {
    let root = tempdir().unwrap();
    let database = root.path().join("maker.sqlite3");
    let actor = manifest(root.path(), "zec-race", MakerActorKindV1::Zcash, 9);
    let setup = SqliteSwapStore::open(&database).unwrap();
    setup.save(&swap("zec-race", Pair::Zcash)).unwrap();
    drop(setup);

    let barrier = Arc::new(Barrier::new(3));
    let registrations = [1_u8, 2_u8].map(|_| {
        let database = database.clone();
        let actor = actor.clone();
        let barrier = Arc::clone(&barrier);
        thread::spawn(move || {
            let mut store = SqliteSwapStore::open(database).unwrap();
            barrier.wait();
            store.register_maker_actor(&actor, 10).unwrap().was_replay()
        })
    });
    barrier.wait();
    let mut replay_results = registrations.map(|handle| handle.join().unwrap());
    replay_results.sort_unstable();
    assert_eq!(replay_results, [false, true]);

    let barrier = Arc::new(Barrier::new(3));
    let claims = [
        MakerActorLeaseOwner::new([11; 16]).unwrap(),
        MakerActorLeaseOwner::new([12; 16]).unwrap(),
    ]
    .map(|owner| {
        let database = database.clone();
        let barrier = Arc::clone(&barrier);
        thread::spawn(move || {
            let mut store = SqliteSwapStore::open(database).unwrap();
            barrier.wait();
            store
                .claim_maker_actor(&SwapId::new("zec-race").unwrap(), owner, 10)
                .unwrap()
                .is_some()
        })
    });
    barrier.wait();
    let wins = claims
        .map(|handle| handle.join().unwrap())
        .into_iter()
        .filter(|won| *won)
        .count();
    assert_eq!(wins, 1);
}

#[test]
fn competing_connections_can_claim_distinct_swaps_independently() {
    let root = tempdir().unwrap();
    let database = root.path().join("maker.sqlite3");
    let mut setup = SqliteSwapStore::open(&database).unwrap();
    for (id, byte) in [("zec-left", 21), ("zec-right", 22)] {
        setup.save(&swap(id, Pair::Zcash)).unwrap();
        setup
            .register_maker_actor(
                &manifest(root.path(), id, MakerActorKindV1::Zcash, byte),
                10,
            )
            .unwrap();
    }
    drop(setup);

    let barrier = Arc::new(Barrier::new(3));
    let claims = [("zec-left", [31; 16]), ("zec-right", [32; 16])].map(|(id, owner)| {
        let database = database.clone();
        let barrier = Arc::clone(&barrier);
        thread::spawn(move || {
            let mut store = SqliteSwapStore::open(database).unwrap();
            barrier.wait();
            store
                .claim_maker_actor(
                    &SwapId::new(id).unwrap(),
                    MakerActorLeaseOwner::new(owner).unwrap(),
                    10,
                )
                .unwrap()
                .is_some()
        })
    });
    barrier.wait();
    assert!(claims.into_iter().all(|handle| handle.join().unwrap()));
}
