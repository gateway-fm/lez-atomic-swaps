use std::{fs, os::unix::fs::PermissionsExt as _, path::Path};

use lez_bridge_protocol::RequestId;
use lez_swap_core::{
    Chain, ChainPosition, ConfirmationPolicy, Pair, RecoverySchedule, SwapCoordinator,
    SwapDirection, SwapId, TimelockSafety,
};
use lez_swap_store::{
    MakerActorAttemptResolution, MakerActorHeldLock, MakerActorKindV1, MakerActorLeaseOwner,
    MakerActorManifestV1, MakerActorManualAction, MakerActorManualActionState,
    MakerActorProcessError, MakerPairConfigurationV1, MakerPriceSourceKind, MakerRouteV1,
    SqliteSwapStore,
};
use tempfile::tempdir;

fn swap(id: &str) -> SwapCoordinator {
    SwapCoordinator::new_with_direction(
        SwapId::new(id).unwrap(),
        Pair::Zcash,
        SwapDirection::TakerSellsForeign,
        ConfirmationPolicy::new(2).unwrap(),
        RecoverySchedule::new(
            Pair::Zcash,
            SwapDirection::TakerSellsForeign,
            ChainPosition::block_height(Chain::Lez, 100),
            ChainPosition::block_height(Chain::Zcash, 120),
            TimelockSafety::between(Chain::Lez, Chain::Zcash, 1_000, 1_200, 100).unwrap(),
        )
        .unwrap(),
    )
}

fn register(store: &mut SqliteSwapStore, root: &Path, id: &str, byte: u8) {
    store.save(&swap(id)).unwrap();
    store
        .register_maker_actor(
            &MakerActorManifestV1::new(
                SwapId::new(id).unwrap(),
                MakerActorKindV1::Zcash,
                root.join(format!("{id}-actor.json")),
                [byte; 32],
                "/usr/bin/true".into(),
                [byte.wrapping_add(1); 32],
                root.join(format!("{id}-actor.sqlite3")),
            )
            .unwrap(),
            10,
        )
        .unwrap();
}

fn assert_restart_replay_and_conflicts(
    store: &mut SqliteSwapStore,
    request: &RequestId,
    id: &SwapId,
) {
    let replay = store
        .queue_maker_actor_manual_action(request, id, MakerActorManualAction::Claim, 0, 99)
        .unwrap();
    assert!(replay.was_replay());
    assert_eq!(replay.requested_after_generation(), 0);
    assert!(matches!(
        store.queue_maker_actor_manual_action(request, id, MakerActorManualAction::Refund, 0, 12,),
        Err(MakerActorProcessError::ManualActionRequestConflict)
    ));
    let competing = RequestId::new("manual-refund-002").unwrap();
    assert!(matches!(
        store.queue_maker_actor_manual_action(
            &competing,
            id,
            MakerActorManualAction::Refund,
            1,
            12,
        ),
        Err(MakerActorProcessError::ManualActionGenerationConflict)
    ));
    assert!(matches!(
        store.queue_maker_actor_manual_action(
            &competing,
            id,
            MakerActorManualAction::Refund,
            0,
            12,
        ),
        Err(MakerActorProcessError::ManualActionPending)
    ));
}

#[test]
fn manual_action_is_exact_replay_generation_fenced_and_restart_safe() {
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let database = root.path().join("maker.sqlite3");
    let id = SwapId::new("zec-manual-claim").unwrap();
    let request = RequestId::new("manual-claim-001").unwrap();
    let mut store = SqliteSwapStore::open(&database).unwrap();
    register(&mut store, root.path(), id.as_str(), 1);

    let first = store
        .queue_maker_actor_manual_action(&request, &id, MakerActorManualAction::Claim, 0, 11)
        .unwrap();
    assert!(!first.was_replay());
    assert_eq!(first.requested_after_generation(), 0);
    let queued = store
        .maker_actor_manual_action(&id)
        .unwrap()
        .expect("queued action");
    assert_eq!(queued.request_id(), &request);
    assert_eq!(queued.action(), MakerActorManualAction::Claim);
    assert_eq!(queued.state(), MakerActorManualActionState::Queued);
    assert_eq!(queued.lease_generation(), None);
    drop(store);

    let mut reopened = SqliteSwapStore::open(&database).unwrap();
    assert_restart_replay_and_conflicts(&mut reopened, &request, &id);

    let owner_one = MakerActorLeaseOwner::new([1; 16]).unwrap();
    let lease_one = reopened
        .claim_maker_actor(&id, owner_one, 11)
        .unwrap()
        .unwrap();
    let forged = lease_one.with_owner(MakerActorLeaseOwner::new([2; 16]).unwrap());
    assert!(matches!(
        reopened.claim_maker_actor_manual_action(&forged),
        Err(MakerActorProcessError::LeaseConflict)
    ));
    let leased = reopened
        .claim_maker_actor_manual_action(&lease_one)
        .unwrap()
        .expect("manual claim is leased with actor");
    assert_eq!(leased.action(), MakerActorManualAction::Claim);
    assert_eq!(leased.state(), MakerActorManualActionState::Leased);
    assert_eq!(leased.lease_generation(), Some(1));
    let exact = reopened
        .claim_maker_actor_manual_action(&lease_one)
        .unwrap()
        .expect("same lease exact-replays action claim");
    assert_eq!(exact, leased);

    reopened
        .resolve_maker_actor_attempt(
            &lease_one,
            MakerActorAttemptResolution::Requeue { not_before: 20 },
            15,
        )
        .unwrap();
    let queued = reopened
        .maker_actor_manual_action(&id)
        .unwrap()
        .expect("nonterminal action remains queued");
    assert_eq!(queued.state(), MakerActorManualActionState::Queued);
    assert_eq!(queued.lease_generation(), None);

    let lease_two = reopened
        .claim_maker_actor(&id, MakerActorLeaseOwner::new([3; 16]).unwrap(), 20)
        .unwrap()
        .unwrap();
    assert_eq!(lease_two.generation(), 2);
    let leased_again = reopened
        .claim_maker_actor_manual_action(&lease_two)
        .unwrap()
        .unwrap();
    assert_eq!(leased_again.lease_generation(), Some(2));
    assert!(matches!(
        reopened
            .resolve_maker_actor_attempt(&lease_one, MakerActorAttemptResolution::Terminal, 21,),
        Err(MakerActorProcessError::LeaseConflict)
    ));
    reopened
        .resolve_maker_actor_attempt(
            &lease_two,
            MakerActorAttemptResolution::ManualActionCompleted,
            21,
        )
        .unwrap();
    assert_eq!(
        reopened
            .maker_actor_manual_action(&id)
            .unwrap()
            .unwrap()
            .state(),
        MakerActorManualActionState::Completed
    );
    drop(reopened);

    let mut restarted = SqliteSwapStore::open(&database).unwrap();
    let terminal_replay = restarted
        .queue_maker_actor_manual_action(&request, &id, MakerActorManualAction::Claim, 0, 200)
        .unwrap();
    assert!(terminal_replay.was_replay());
}

#[test]
fn manual_action_request_ids_share_the_global_maker_mutation_namespace() {
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let database = root.path().join("maker.sqlite3");
    let id = SwapId::new("zec-global-request").unwrap();
    let request = RequestId::new("global-mutation-001").unwrap();
    let mut store = SqliteSwapStore::open(&database).unwrap();
    register(&mut store, root.path(), id.as_str(), 9);
    let route = MakerRouteV1::new(Pair::Zcash, SwapDirection::TakerSellsForeign).unwrap();
    let configuration =
        MakerPairConfigurationV1::new(route, false, MakerPriceSourceKind::Local, 1, 1, 60).unwrap();
    store
        .configure_maker_pair(&request, None, &configuration)
        .unwrap();

    assert!(matches!(
        store.queue_maker_actor_manual_action(&request, &id, MakerActorManualAction::Claim, 0, 11,),
        Err(MakerActorProcessError::ManualActionRequestConflict)
    ));
    assert!(store.maker_actor_manual_action(&id).unwrap().is_none());
}

#[test]
fn request_during_a_live_lease_waits_for_the_next_generation() {
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let database = root.path().join("maker.sqlite3");
    let id = SwapId::new("zec-live-lease-request").unwrap();
    let mut store = SqliteSwapStore::open(&database).unwrap();
    register(&mut store, root.path(), id.as_str(), 10);
    let live = store
        .claim_maker_actor(&id, MakerActorLeaseOwner::new([10; 16]).unwrap(), 10)
        .unwrap()
        .unwrap();
    assert_eq!(live.generation(), 1);

    store
        .queue_maker_actor_manual_action(
            &RequestId::new("manual-claim-next-generation").unwrap(),
            &id,
            MakerActorManualAction::Claim,
            1,
            11,
        )
        .expect("a live worker cannot block a durable next-generation request");
    assert!(
        store
            .claim_maker_actor_manual_action(&live)
            .unwrap()
            .is_none(),
        "the request must not splice into the generation already running"
    );
    store
        .resolve_maker_actor_attempt(
            &live,
            MakerActorAttemptResolution::Requeue { not_before: 20 },
            12,
        )
        .unwrap();
    let next = store
        .claim_maker_actor(&id, MakerActorLeaseOwner::new([11; 16]).unwrap(), 20)
        .unwrap()
        .unwrap();
    assert_eq!(next.generation(), 2);
    assert_eq!(
        store
            .claim_maker_actor_manual_action(&next)
            .unwrap()
            .unwrap()
            .lease_generation(),
        Some(2)
    );
}

#[test]
fn abandoned_action_transfers_only_with_the_exact_kernel_locked_lease() {
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let database = root.path().join("maker.sqlite3");
    let id = SwapId::new("zec-manual-refund").unwrap();
    let mut store = SqliteSwapStore::open(&database).unwrap();
    register(&mut store, root.path(), id.as_str(), 11);
    store
        .queue_maker_actor_manual_action(
            &RequestId::new("manual-refund-crash").unwrap(),
            &id,
            MakerActorManualAction::Refund,
            0,
            11,
        )
        .unwrap();
    let old_lease = store
        .claim_maker_actor(&id, MakerActorLeaseOwner::new([11; 16]).unwrap(), 11)
        .unwrap()
        .unwrap();
    store
        .claim_maker_actor_manual_action(&old_lease)
        .unwrap()
        .unwrap();
    drop(store);

    let mut reopened = SqliteSwapStore::open(&database).unwrap();
    let held = MakerActorHeldLock::acquire(old_lease.record()).unwrap();
    let recovered = reopened
        .recover_abandoned_maker_actor(
            &old_lease,
            &held,
            MakerActorLeaseOwner::new([12; 16]).unwrap(),
            20,
        )
        .unwrap();
    assert_eq!(recovered.generation(), 2);
    let transferred = reopened
        .claim_maker_actor_manual_action(&recovered)
        .unwrap()
        .expect("recovered generation owns the same durable action");
    assert_eq!(transferred.action(), MakerActorManualAction::Refund);
    assert_eq!(transferred.state(), MakerActorManualActionState::Leased);
    assert_eq!(transferred.lease_generation(), Some(2));
    assert!(matches!(
        reopened
            .resolve_maker_actor_attempt(&old_lease, MakerActorAttemptResolution::Terminal, 21,),
        Err(MakerActorProcessError::LeaseConflict)
    ));
    reopened
        .resolve_maker_actor_attempt(
            &recovered,
            MakerActorAttemptResolution::Backoff {
                not_before: 30,
                failure_class: "dependency_unavailable".into(),
            },
            21,
        )
        .unwrap();
    assert_eq!(
        reopened
            .maker_actor_manual_action(&id)
            .unwrap()
            .unwrap()
            .state(),
        MakerActorManualActionState::Queued
    );
}
