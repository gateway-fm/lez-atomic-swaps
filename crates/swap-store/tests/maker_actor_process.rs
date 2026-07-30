use std::{
    fs,
    os::unix::fs::{PermissionsExt as _, symlink},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Barrier},
    thread,
};

use lez_swap_core::{
    Chain, ChainPosition, ConfirmationPolicy, Pair, RecoverySchedule, SwapCoordinator,
    SwapDirection, SwapId, TimelockSafety,
};
use lez_swap_store::{
    MakerActorArtifacts, MakerActorAttemptResolution, MakerActorHeldLock, MakerActorKindV1,
    MakerActorLeaseOwner, MakerActorManifestV1, MakerActorProcessError, MakerActorScheduleState,
    SqliteSwapStore,
};
use sha2::{Digest as _, Sha256};
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
    assert_eq!(
        store
            .maker_actor_process(zec.id())
            .unwrap()
            .unwrap()
            .manifest(),
        &original
    );
    assert!(
        store
            .maker_actor_process(&SwapId::new("valid-absent-actor").unwrap())
            .unwrap()
            .is_none()
    );
    let monitor = store
        .maker_actor_monitor_snapshot(zec.id())
        .unwrap()
        .unwrap();
    assert_eq!(monitor.process().manifest(), &original);
    assert!(monitor.progress().is_none());
    assert!(monitor.manual_action().is_none());
    assert!(
        store
            .maker_actor_monitor_snapshot(&SwapId::new("valid-absent-actor").unwrap())
            .unwrap()
            .is_none()
    );

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
fn reaped_child_clear_requires_exact_lease_pid_and_start_ticks() {
    let root = tempdir().unwrap();
    let database = root.path().join("maker.sqlite3");
    let mut store = SqliteSwapStore::open(&database).unwrap();
    store.save(&swap("zec-child-clear", Pair::Zcash)).unwrap();
    store
        .register_maker_actor(
            &manifest(root.path(), "zec-child-clear", MakerActorKindV1::Zcash, 29),
            10,
        )
        .unwrap();
    let owner = MakerActorLeaseOwner::new([29; 16]).unwrap();
    let lease = store
        .claim_maker_actor(&SwapId::new("zec-child-clear").unwrap(), owner, 10)
        .unwrap()
        .unwrap();
    store.record_maker_actor_child(&lease, 91, 9_100).unwrap();

    assert!(matches!(
        store.clear_maker_actor_child(&lease, 91, 9_101),
        Err(MakerActorProcessError::LeaseConflict)
    ));
    let forged = lease.with_owner(MakerActorLeaseOwner::new([30; 16]).unwrap());
    assert!(matches!(
        store.clear_maker_actor_child(&forged, 91, 9_100),
        Err(MakerActorProcessError::LeaseConflict)
    ));
    assert_eq!(
        store.list_maker_actor_processes().unwrap()[0].child_identity(),
        Some((91, 9_100))
    );

    store.clear_maker_actor_child(&lease, 91, 9_100).unwrap();
    assert_eq!(
        store.list_maker_actor_processes().unwrap()[0].child_identity(),
        None
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

#[test]
fn inherited_lock_is_a_nonforgeable_abandoned_lease_recovery_capability() {
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let database = root.path().join("maker.sqlite3");
    let mut store = SqliteSwapStore::open(&database).unwrap();
    for (id, byte) in [("zec-locked", 41), ("zec-peer", 42)] {
        store.save(&swap(id, Pair::Zcash)).unwrap();
        store
            .register_maker_actor(
                &manifest(root.path(), id, MakerActorKindV1::Zcash, byte),
                10,
            )
            .unwrap();
    }
    let records = store.list_maker_actor_processes().unwrap();
    let locked_record = records
        .iter()
        .find(|record| record.swap_id().as_str() == "zec-locked")
        .unwrap()
        .clone();
    let peer_record = records
        .iter()
        .find(|record| record.swap_id().as_str() == "zec-peer")
        .unwrap()
        .clone();
    let locked_id = SwapId::new("zec-locked").unwrap();
    let lease = store
        .claim_maker_actor(&locked_id, MakerActorLeaseOwner::new([44; 16]).unwrap(), 10)
        .unwrap()
        .unwrap();

    let held = MakerActorHeldLock::acquire(&locked_record).unwrap();
    assert!(matches!(
        MakerActorHeldLock::acquire(&locked_record),
        Err(MakerActorProcessError::LockUnavailable)
    ));
    let peer = MakerActorHeldLock::acquire(&peer_record).unwrap();
    assert!(matches!(
        store.recover_abandoned_maker_actor(
            &lease,
            &peer,
            MakerActorLeaseOwner::new([45; 16]).unwrap(),
            20,
        ),
        Err(MakerActorProcessError::LockMismatch)
    ));
    drop(peer);

    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg("read inherited_lock_probe")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    held.inherit_into(&mut command).unwrap();
    let mut child = command.spawn().unwrap();
    drop(command);
    drop(held);
    assert!(matches!(
        MakerActorHeldLock::acquire(&locked_record),
        Err(MakerActorProcessError::LockUnavailable)
    ));

    child.kill().unwrap();
    child.wait().unwrap();
    let recovered = MakerActorHeldLock::acquire(&locked_record).unwrap();
    let next = store
        .recover_abandoned_maker_actor(
            &lease,
            &recovered,
            MakerActorLeaseOwner::new([45; 16]).unwrap(),
            20,
        )
        .unwrap();
    assert_eq!(next.generation(), 2);
    assert!(matches!(
        store.recover_abandoned_maker_actor(
            &lease,
            &recovered,
            MakerActorLeaseOwner::new([46; 16]).unwrap(),
            30,
        ),
        Err(MakerActorProcessError::LeaseConflict)
    ));
    let records = store.list_maker_actor_processes().unwrap();
    let locked = records
        .iter()
        .find(|record| record.swap_id().as_str() == "zec-locked")
        .unwrap();
    let peer = records
        .iter()
        .find(|record| record.swap_id().as_str() == "zec-peer")
        .unwrap();
    assert_eq!(locked.lease_generation(), 2);
    assert_eq!(locked.attempt_count(), 2);
    assert_eq!(peer.lease_generation(), 0);
    assert_eq!(peer.attempt_count(), 0);
}

#[test]
fn lock_acquisition_rejects_unsafe_parent_and_hardlinked_inode() {
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let database = root.path().join("maker.sqlite3");
    let mut store = SqliteSwapStore::open(&database).unwrap();
    store.save(&swap("zec-lock-safety", Pair::Zcash)).unwrap();
    store
        .register_maker_actor(
            &manifest(root.path(), "zec-lock-safety", MakerActorKindV1::Zcash, 51),
            10,
        )
        .unwrap();
    let record = store.list_maker_actor_processes().unwrap().remove(0);

    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o750)).unwrap();
    assert!(matches!(
        MakerActorHeldLock::acquire(&record),
        Err(MakerActorProcessError::UnsafeLock)
    ));

    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    drop(MakerActorHeldLock::acquire(&record).unwrap());
    let lock_path = root
        .path()
        .join("zec-lock-safety-actor.sqlite3.maker-actor.lock");
    fs::hard_link(&lock_path, root.path().join("attacker-link")).unwrap();
    assert!(matches!(
        MakerActorHeldLock::acquire(&record),
        Err(MakerActorProcessError::UnsafeLock)
    ));
}

#[test]
fn verified_artifact_fds_survive_path_replacement_before_exec() {
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let config_path = root.path().join("zec-artifact-config.json");
    let program_path = root.path().join("zec-artifact-actor");
    let state_path = root.path().join("zec-artifact-state.sqlite3");
    let config = b"trusted-config\n";
    let program = b"#!/bin/sh\ncat /proc/self/fd/196\n";
    write_mode(&config_path, config, 0o600);
    write_mode(&program_path, program, 0o700);

    let database = root.path().join("maker.sqlite3");
    let mut store = SqliteSwapStore::open(&database).unwrap();
    store.save(&swap("zec-artifact", Pair::Zcash)).unwrap();
    store
        .register_maker_actor(
            &MakerActorManifestV1::new(
                SwapId::new("zec-artifact").unwrap(),
                MakerActorKindV1::Zcash,
                config_path.clone(),
                digest(config),
                program_path.clone(),
                digest(program),
                state_path,
            )
            .unwrap(),
            10,
        )
        .unwrap();
    let record = store.list_maker_actor_processes().unwrap().remove(0);
    let held = MakerActorHeldLock::acquire(&record).unwrap();
    let artifacts = MakerActorArtifacts::open_validated(&record, |verified_config| {
        assert_eq!(verified_config, config);
        fs::rename(&config_path, root.path().join("original-config")).unwrap();
        write_mode(&config_path, b"attacker-config\n", 0o600);
        Ok(())
    })
    .unwrap();
    fs::rename(&program_path, root.path().join("original-program")).unwrap();
    write_mode(&program_path, b"#!/bin/sh\necho attacker-program\n", 0o700);

    let mut command = artifacts.into_command(&held).unwrap();
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, config);
}

#[test]
fn artifact_binding_rejects_hash_symlink_hardlink_and_unsafe_state() {
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let database = root.path().join("maker.sqlite3");
    let mut store = SqliteSwapStore::open(&database).unwrap();
    let program = b"#!/bin/sh\nexit 0\n";
    let config = b"{}\n";

    for id in ["hash", "symlink", "hardlink", "state"] {
        store.save(&swap(id, Pair::Zcash)).unwrap();
        let config_path = root.path().join(format!("{id}-config.json"));
        let program_path = root.path().join(format!("{id}-actor"));
        write_mode(&config_path, config, 0o600);
        write_mode(&program_path, program, 0o700);
        let config_sha = if id == "hash" {
            [99; 32]
        } else {
            digest(config)
        };
        store
            .register_maker_actor(
                &MakerActorManifestV1::new(
                    SwapId::new(id).unwrap(),
                    MakerActorKindV1::Zcash,
                    config_path,
                    config_sha,
                    program_path,
                    digest(program),
                    root.path().join(format!("{id}-state.sqlite3")),
                )
                .unwrap(),
                10,
            )
            .unwrap();
    }
    let records = store.list_maker_actor_processes().unwrap();
    let record = |id: &str| {
        records
            .iter()
            .find(|record| record.swap_id().as_str() == id)
            .unwrap()
    };
    assert!(matches!(
        MakerActorArtifacts::open(record("hash")),
        Err(MakerActorProcessError::ArtifactHashMismatch)
    ));

    let symlink_config = root.path().join("symlink-config.json");
    fs::remove_file(&symlink_config).unwrap();
    let symlink_target = root.path().join("symlink-target.json");
    write_mode(&symlink_target, config, 0o600);
    symlink(&symlink_target, &symlink_config).unwrap();
    assert!(matches!(
        MakerActorArtifacts::open(record("symlink")),
        Err(MakerActorProcessError::UnsafeArtifact)
    ));

    let hardlink_program = root.path().join("hardlink-actor");
    fs::hard_link(&hardlink_program, root.path().join("program-alias")).unwrap();
    assert!(matches!(
        MakerActorArtifacts::open(record("hardlink")),
        Err(MakerActorProcessError::UnsafeArtifact)
    ));

    write_mode(&root.path().join("state-state.sqlite3"), b"sqlite", 0o644);
    assert!(matches!(
        MakerActorArtifacts::open(record("state")),
        Err(MakerActorProcessError::UnsafeArtifact)
    ));
}

#[test]
fn unexpected_state_creation_after_binding_fails_closed() {
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let config_path = root.path().join("state-race-config.json");
    let program_path = root.path().join("state-race-actor");
    let state_path = root.path().join("state-race.sqlite3");
    let config = b"{}\n";
    let program = b"#!/bin/sh\nexit 0\n";
    write_mode(&config_path, config, 0o600);
    write_mode(&program_path, program, 0o700);

    let database = root.path().join("maker.sqlite3");
    let mut store = SqliteSwapStore::open(&database).unwrap();
    store.save(&swap("state-race", Pair::Zcash)).unwrap();
    store
        .register_maker_actor(
            &MakerActorManifestV1::new(
                SwapId::new("state-race").unwrap(),
                MakerActorKindV1::Zcash,
                config_path,
                digest(config),
                program_path,
                digest(program),
                state_path.clone(),
            )
            .unwrap(),
            10,
        )
        .unwrap();
    let record = store.list_maker_actor_processes().unwrap().remove(0);
    let held = MakerActorHeldLock::acquire(&record).unwrap();
    let artifacts = MakerActorArtifacts::open(&record).unwrap();
    write_mode(&state_path, b"unexpected", 0o600);

    assert!(matches!(
        artifacts.into_command(&held),
        Err(MakerActorProcessError::UnsafeArtifact)
    ));
}

fn write_mode(path: &Path, bytes: &[u8], mode: u32) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

#[test]
fn random_lease_owners_are_nonzero_and_unique_in_a_small_sample() {
    let mut owners = Vec::new();
    for _ in 0..64 {
        let owner = MakerActorLeaseOwner::random().unwrap();
        assert!(!owners.contains(&owner));
        owners.push(owner);
    }
}

#[test]
fn monero_actor_registration_is_pair_bound_and_survives_reopen() {
    let root = tempdir().unwrap();
    let database = root.path().join("maker.sqlite3");
    let actor = manifest(root.path(), "xmr-a", MakerActorKindV1::Monero, 71);
    let mut store = SqliteSwapStore::open(&database).unwrap();
    store.save(&xmr_swap("xmr-a")).unwrap();
    assert!(!store.register_maker_actor(&actor, 10).unwrap().was_replay());
    drop(store);

    let reopened = SqliteSwapStore::open(&database).unwrap();
    let record = reopened
        .maker_actor_process(&SwapId::new("xmr-a").unwrap())
        .unwrap()
        .expect("reopened Monero actor row");
    assert_eq!(record.manifest(), &actor);
    assert_eq!(record.manifest().kind(), MakerActorKindV1::Monero);
}

#[test]
#[allow(clippy::too_many_lines)] // The migration proof exercises every preserved dependent table.
fn schema_20_actor_kind_checks_widen_to_monero_without_losing_existing_rows() {
    let root = tempdir().unwrap();
    let database = root.path().join("maker.sqlite3");
    let mut store = SqliteSwapStore::open(&database).unwrap();
    for (id, kind, pair, byte) in [
        (
            "migration-btc",
            MakerActorKindV1::Bitcoin,
            Pair::Bitcoin,
            81,
        ),
        ("migration-zec", MakerActorKindV1::Zcash, Pair::Zcash, 82),
    ] {
        store.save(&swap(id, pair)).unwrap();
        store
            .register_maker_actor(&manifest(root.path(), id, kind, byte), 10)
            .unwrap();
    }
    drop(store);

    let raw = rusqlite::Connection::open(&database).unwrap();
    raw.execute(
        "INSERT INTO maker_actor_manual_actions (
             request_id, swap_id, action, state, requested_after_generation, created_at, updated_at
         ) VALUES (?1, ?2, 'claim', 'queued', 0, 11, 11)",
        rusqlite::params!["migration-request", "migration-btc"],
    )
    .unwrap();
    raw.execute(
        "INSERT INTO maker_actor_progress (
             swap_id, payload_version, actor_kind, source_generation, payload_json, observed_at
         ) VALUES (?1, 1, 'zcash', 1, ?2, 12)",
        rusqlite::params!["migration-zec", "{\"state\":\"not_activated\"}"],
    )
    .unwrap();
    raw.pragma_update(None, "writable_schema", true).unwrap();
    for table in ["maker_actor_processes", "maker_actor_progress"] {
        let sql: String = raw
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type = \"table\" AND name = ?1",
                rusqlite::params![table],
                |row| row.get(0),
            )
            .unwrap();
        let narrowed = sql.replace(", 'monero'", "").replace("'monero', ", "");
        assert_ne!(narrowed, sql, "fresh schema must already admit Monero");
        assert!(!narrowed.contains("'monero'"));
        raw.execute(
            "UPDATE sqlite_schema SET sql = ?1 WHERE type = \"table\" AND name = ?2",
            rusqlite::params![narrowed, table],
        )
        .unwrap();
    }
    raw.pragma_update(None, "user_version", 20).unwrap();
    let schema_version: i64 = raw
        .pragma_query_value(None, "schema_version", |row| row.get(0))
        .unwrap();
    raw.pragma_update(None, "schema_version", schema_version + 1)
        .unwrap();
    raw.pragma_update(None, "writable_schema", false).unwrap();
    drop(raw);

    let mut migrated = SqliteSwapStore::open(&database).unwrap();
    let preserved = migrated.list_maker_actor_processes().unwrap();
    assert_eq!(preserved.len(), 2);
    assert!(preserved.iter().any(|row| {
        row.swap_id().as_str() == "migration-btc"
            && row.manifest().kind() == MakerActorKindV1::Bitcoin
    }));
    assert!(preserved.iter().any(|row| {
        row.swap_id().as_str() == "migration-zec"
            && row.manifest().kind() == MakerActorKindV1::Zcash
    }));

    migrated.save(&xmr_swap("migration-xmr")).unwrap();
    migrated
        .register_maker_actor(
            &manifest(root.path(), "migration-xmr", MakerActorKindV1::Monero, 83),
            10,
        )
        .unwrap();
    drop(migrated);

    let reopened = SqliteSwapStore::open(&database).unwrap();
    let rows = reopened.list_maker_actor_processes().unwrap();
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().any(|row| {
        row.swap_id().as_str() == "migration-xmr"
            && row.manifest().kind() == MakerActorKindV1::Monero
    }));
    let raw = rusqlite::Connection::open(&database).unwrap();
    for table in ["maker_actor_manual_actions", "maker_actor_progress"] {
        let count: i64 = raw
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 1, "{table} row was lost during widening");
    }
    for table in ["maker_actor_processes", "maker_actor_progress"] {
        let sql: String = raw
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type = \"table\" AND name = ?1",
                rusqlite::params![table],
                |row| row.get(0),
            )
            .unwrap();
        assert!(sql.contains("'monero'"), "{table} CHECK was not widened");
    }
}
