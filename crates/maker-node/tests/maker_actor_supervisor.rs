mod support;

use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _},
    time::{Duration, Instant},
};

use lez_maker_node::{
    MakerActorSupervisorCancellation, MakerActorSupervisorConfig, MakerActorSupervisorResolution,
    prepare_maker_actor, supervise_one_abandoned_maker_actor, supervise_one_due_maker_actor,
    supervise_one_due_maker_actor_until,
};
use lez_swap_core::{
    Chain, ChainPosition, ConfirmationPolicy, Pair, RecoverySchedule, SwapCoordinator,
    SwapDirection, SwapId, TimelockSafety,
};
use lez_swap_store::{
    MakerActorHeldLock, MakerActorKindV1, MakerActorLeaseOwner, MakerActorManifestV1,
    MakerActorScheduleState, SqliteSwapStore, validate_maker_actor_program,
};
use sha2::{Digest as _, Sha256};
use tempfile::tempdir;
use zec_reference_actor::ActorConfig;

use support::actor_deployment;

#[test]
fn one_bounded_cycle_runs_exact_sealed_actor_and_durably_requeues() {
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let swap_id = "m5-supervised-zec";
    let deployment = actor_deployment(root.path(), swap_id);
    let actor_config = ActorConfig::load_private(&deployment.source_config).unwrap();
    let invocation_log = root.path().join("actor-invocations");
    let program_path = root.path().join("supervised-actor");
    let program = format!(
        "#!/bin/sh\n\
         test \"$1\" = \"--config-fd\" || exit 91\n\
         test \"$2\" = \"196\" || exit 92\n\
         test -r /proc/self/fd/196 || exit 93\n\
         test -r /proc/self/fd/198 || exit 94\n\
         printf '%s\\n' \"$3\" >> '{}'\n\
         case \"$3\" in\n\
           status) printf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"state\":\"not_activated\"}}' ;;\n\
           activate) printf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"command\":\"activate\",\"outcome\":\"activated\",\"phase\":\"offered\",\"revision\":1}}' ;;\n\
           *) exit 95 ;;\n\
         esac\n",
        invocation_log.display()
    );
    write_private(&program_path, program.as_bytes(), 0o700);
    let config_bytes = fs::read(&deployment.source_config).unwrap();
    let program_sha256: [u8; 32] = Sha256::digest(program.as_bytes()).into();

    let database = root.path().join("maker.sqlite3");
    let mut store = SqliteSwapStore::open(&database).unwrap();
    store.save(&swap(swap_id)).unwrap();
    store
        .register_maker_actor(
            &MakerActorManifestV1::new(
                SwapId::new(swap_id).unwrap(),
                MakerActorKindV1::Zcash,
                deployment.source_config,
                Sha256::digest(config_bytes).into(),
                program_path,
                program_sha256,
                actor_config.role_state_db().to_path_buf(),
            )
            .unwrap(),
            10,
        )
        .unwrap();
    let registered = store.list_maker_actor_processes().unwrap().remove(0);
    validate_maker_actor_program(
        registered.manifest().program_path(),
        registered.manifest().program_sha256(),
    )
    .expect("program preflight");
    prepare_maker_actor(&registered).expect("exact deployment preflight");

    let config = MakerActorSupervisorConfig::new(Duration::from_secs(2), 5, 30, 8_192)
        .expect("bounded supervisor config");
    let outcome = supervise_one_due_maker_actor(
        &mut store,
        MakerActorLeaseOwner::new([0x51; 16]).unwrap(),
        10,
        &config,
    )
    .expect("supervisor cycle")
    .expect("one due actor");

    assert_eq!(outcome.swap_id().as_str(), swap_id);
    assert_eq!(outcome.generation(), 1);
    assert_eq!(
        outcome.resolution(),
        MakerActorSupervisorResolution::Requeued,
        "invocations={:?} records={:?}",
        fs::read_to_string(&invocation_log),
        store.list_maker_actor_processes()
    );
    assert_eq!(
        fs::read_to_string(invocation_log).unwrap(),
        "status\nactivate\n"
    );

    let record = store.list_maker_actor_processes().unwrap().remove(0);
    assert_eq!(record.schedule_state(), MakerActorScheduleState::Queued);
    assert_eq!(record.attempt_count(), 1);
    assert_eq!(record.child_identity(), None);
    assert!(store.list_due_maker_actor_ids(14, 1).unwrap().is_empty());
    assert_eq!(
        store.list_due_maker_actor_ids(15, 1).unwrap(),
        [SwapId::new(swap_id).unwrap()]
    );
}

#[test]
fn abandoned_lease_is_generation_transferred_and_run_without_an_unleased_gap() {
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let swap_id = "m5-supervisor-recovery";
    let program = b"#!/bin/sh\n\
        test \"$1\" = \"--config-fd\" || exit 91\n\
        test \"$2\" = \"196\" || exit 92\n\
        test -r /proc/self/fd/196 || exit 93\n\
        test -r /proc/self/fd/198 || exit 94\n\
        case \"$3\" in\n\
          status) printf '%s\\n' '{\"schema_version\":1,\"role\":\"maker\",\"state\":\"not_activated\"}' ;;\n\
          activate) printf '%s\\n' '{\"schema_version\":1,\"role\":\"maker\",\"command\":\"activate\",\"outcome\":\"activated\",\"phase\":\"offered\",\"revision\":1}' ;;\n\
          *) exit 95 ;;\n\
        esac\n";
    let mut store = registered_store(root.path(), swap_id, program);
    let swap_id = SwapId::new(swap_id).unwrap();
    let stale_owner = MakerActorLeaseOwner::new([0x61; 16]).unwrap();
    let stale_lease = store
        .claim_maker_actor(&swap_id, stale_owner, 10)
        .unwrap()
        .unwrap();
    assert_eq!(stale_lease.generation(), 1);

    let config = MakerActorSupervisorConfig::new(Duration::from_secs(2), 5, 30, 8_192)
        .expect("bounded recovery config");
    let outcome = supervise_one_abandoned_maker_actor(
        &mut store,
        MakerActorLeaseOwner::new([0x62; 16]).unwrap(),
        20,
        &config,
    )
    .expect("recovery cycle")
    .expect("one abandoned actor");

    assert_eq!(outcome.swap_id(), &swap_id);
    assert_eq!(outcome.generation(), 2);
    assert_eq!(
        outcome.resolution(),
        MakerActorSupervisorResolution::Requeued
    );
    let record = store.list_maker_actor_processes().unwrap().remove(0);
    assert_eq!(record.schedule_state(), MakerActorScheduleState::Queued);
    assert_eq!(record.lease_generation(), 2);
    assert_eq!(record.attempt_count(), 2);
    assert_eq!(record.child_identity(), None);
}

#[test]
fn live_old_lock_is_not_stolen_and_does_not_block_a_due_peer() {
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let terminal = b"#!/bin/sh\n\
        test \"$1\" = \"--config-fd\" || exit 91\n\
        test \"$2\" = \"196\" || exit 92\n\
        printf '%s\\n' '{\"schema_version\":1,\"role\":\"maker\",\"state\":\"active\",\"phase\":\"completed\",\"revision\":4,\"next_action\":\"complete\"}'\n";
    let database = root.path().join("maker.sqlite3");
    let mut store = SqliteSwapStore::open(database).unwrap();
    let locked_root = root.path().join("locked");
    let peer_root = root.path().join("peer");
    for actor_root in [&locked_root, &peer_root] {
        fs::create_dir(actor_root).unwrap();
        fs::set_permissions(actor_root, fs::Permissions::from_mode(0o700)).unwrap();
    }
    register_actor(&mut store, &locked_root, "aaa-locked", terminal);
    register_actor(&mut store, &peer_root, "zzz-peer", terminal);
    let locked_id = SwapId::new("aaa-locked").unwrap();
    store
        .claim_maker_actor(
            &locked_id,
            MakerActorLeaseOwner::new([0x63; 16]).unwrap(),
            10,
        )
        .unwrap()
        .unwrap();
    let locked_record = store
        .list_maker_actor_processes()
        .unwrap()
        .into_iter()
        .find(|record| record.swap_id() == &locked_id)
        .unwrap();
    let old_process_lock = MakerActorHeldLock::acquire(&locked_record).unwrap();
    let new_owner = MakerActorLeaseOwner::new([0x64; 16]).unwrap();
    let config = MakerActorSupervisorConfig::new(Duration::from_secs(2), 5, 30, 8_192)
        .expect("bounded peer config");

    assert!(
        supervise_one_abandoned_maker_actor(&mut store, new_owner, 20, &config)
            .unwrap()
            .is_none(),
        "a live inherited lock must prevent generation transfer"
    );
    let peer = supervise_one_due_maker_actor(&mut store, new_owner, 20, &config)
        .unwrap()
        .expect("unrelated due peer progresses");
    assert_eq!(peer.swap_id().as_str(), "zzz-peer");
    assert_eq!(peer.resolution(), MakerActorSupervisorResolution::Terminal);

    let records = store.list_maker_actor_processes().unwrap();
    let locked = records
        .iter()
        .find(|record| record.swap_id() == &locked_id)
        .unwrap();
    assert_eq!(locked.schedule_state(), MakerActorScheduleState::Leased);
    assert_eq!(locked.lease_generation(), 1);
    assert_eq!(locked.attempt_count(), 1);
    drop(old_process_lock);
}

#[test]
fn timed_out_actor_is_reaped_cleared_and_durably_backed_off() {
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let swap_id = "m5-supervisor-timeout";
    let program = b"#!/bin/sh\n\
        test \"$1\" = \"--config-fd\" || exit 91\n\
        test \"$2\" = \"196\" || exit 92\n\
        test -r /proc/self/fd/196 || exit 93\n\
        test -r /proc/self/fd/198 || exit 94\n\
        while :; do :; done &\n\
        wait\n";
    let mut store = registered_store(root.path(), swap_id, program);
    let config = MakerActorSupervisorConfig::new(Duration::from_millis(50), 5, 30, 8_192)
        .expect("bounded timeout config");
    let started = Instant::now();

    let outcome = supervise_one_due_maker_actor(
        &mut store,
        MakerActorLeaseOwner::new([0x52; 16]).unwrap(),
        10,
        &config,
    )
    .expect("timeout cycle")
    .expect("one due actor");

    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(
        outcome.resolution(),
        MakerActorSupervisorResolution::Backoff
    );
    let record = store.list_maker_actor_processes().unwrap().remove(0);
    assert_eq!(record.schedule_state(), MakerActorScheduleState::Backoff);
    assert_eq!(record.child_identity(), None);
    assert!(store.list_due_maker_actor_ids(39, 1).unwrap().is_empty());
    assert_eq!(
        store.list_due_maker_actor_ids(40, 1).unwrap(),
        [SwapId::new(swap_id).unwrap()]
    );
}

#[test]
fn successful_actor_leader_cannot_leave_stdout_holding_descendant() {
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let swap_id = "m5-supervisor-success-descendant";
    let program = b"#!/bin/sh\n\
        test \"$1\" = \"--config-fd\" || exit 91\n\
        test \"$2\" = \"196\" || exit 92\n\
        while :; do :; done &\n\
        printf '%s\\n' '{\"schema_version\":1,\"role\":\"maker\",\"state\":\"active\",\"phase\":\"completed\",\"revision\":4,\"next_action\":\"complete\"}'\n\
        exit 0\n";
    let mut store = registered_store(root.path(), swap_id, program);
    let config = MakerActorSupervisorConfig::new(Duration::from_secs(2), 5, 30, 8_192)
        .expect("bounded descendant config");
    let started = Instant::now();

    let outcome = supervise_one_due_maker_actor(
        &mut store,
        MakerActorLeaseOwner::new([0x57; 16]).unwrap(),
        10,
        &config,
    )
    .expect("descendant cycle")
    .expect("one due actor");

    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(
        outcome.resolution(),
        MakerActorSupervisorResolution::Terminal
    );
    let record = store.list_maker_actor_processes().unwrap().remove(0);
    assert_eq!(record.schedule_state(), MakerActorScheduleState::Terminal);
    assert_eq!(record.child_identity(), None);
}

#[test]
fn cancellation_kills_descendants_clears_identity_and_durably_backs_off() {
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let swap_id = "m5-supervisor-cancel";
    let program = b"#!/bin/sh\n\
        test \"$1\" = \"--config-fd\" || exit 91\n\
        test \"$2\" = \"196\" || exit 92\n\
        test -r /proc/self/fd/196 || exit 93\n\
        test -r /proc/self/fd/198 || exit 94\n\
        while :; do :; done &\n\
        wait\n";
    let mut store = registered_store(root.path(), swap_id, program);
    let config = MakerActorSupervisorConfig::new(Duration::from_secs(2), 5, 30, 8_192)
        .expect("bounded cancellation config");
    let cancellation = MakerActorSupervisorCancellation::new();
    let signal = cancellation.clone();
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        signal.cancel();
    });
    let started = Instant::now();

    let outcome = supervise_one_due_maker_actor_until(
        &mut store,
        MakerActorLeaseOwner::new([0x56; 16]).unwrap(),
        10,
        &config,
        &cancellation,
    )
    .expect("cancelled cycle")
    .expect("one due actor");
    canceller.join().unwrap();

    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(
        outcome.resolution(),
        MakerActorSupervisorResolution::Backoff
    );
    let record = store.list_maker_actor_processes().unwrap().remove(0);
    assert_eq!(record.schedule_state(), MakerActorScheduleState::Backoff);
    assert_eq!(record.child_identity(), None);
    assert!(store.list_due_maker_actor_ids(39, 1).unwrap().is_empty());
    assert_eq!(
        store.list_due_maker_actor_ids(40, 1).unwrap(),
        [SwapId::new(swap_id).unwrap()]
    );
}

#[test]
fn oversized_actor_output_is_drained_reaped_and_failed_closed() {
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let swap_id = "m5-supervisor-output-cap";
    let program = b"#!/bin/sh\n\
        test \"$1\" = \"--config-fd\" || exit 91\n\
        test \"$2\" = \"196\" || exit 92\n\
        i=0\n\
        while [ \"$i\" -lt 300 ]; do printf x; i=$((i + 1)); done\n";
    let mut store = registered_store(root.path(), swap_id, program);
    let config = MakerActorSupervisorConfig::new(Duration::from_secs(2), 5, 30, 256)
        .expect("minimum bounded output config");

    let outcome = supervise_one_due_maker_actor(
        &mut store,
        MakerActorLeaseOwner::new([0x53; 16]).unwrap(),
        10,
        &config,
    )
    .expect("oversized-output cycle")
    .expect("one due actor");

    assert_eq!(outcome.resolution(), MakerActorSupervisorResolution::Failed);
    let record = store.list_maker_actor_processes().unwrap().remove(0);
    assert_eq!(record.schedule_state(), MakerActorScheduleState::Failed);
    assert_eq!(record.child_identity(), None);
    assert!(
        store
            .list_due_maker_actor_ids(u64::MAX, 1)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn unknown_actor_outcome_is_reaped_and_failed_closed() {
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let swap_id = "m5-supervisor-unknown-outcome";
    let program = b"#!/bin/sh\n\
        test \"$1\" = \"--config-fd\" || exit 91\n\
        test \"$2\" = \"196\" || exit 92\n\
        case \"$3\" in\n\
          status) printf '%s\\n' '{\"schema_version\":1,\"role\":\"maker\",\"state\":\"not_activated\"}' ;;\n\
          activate) printf '%s\\n' '{\"schema_version\":1,\"role\":\"maker\",\"command\":\"activate\",\"outcome\":\"future_untrusted_outcome\",\"phase\":\"offered\",\"revision\":1}' ;;\n\
          *) exit 93 ;;\n\
        esac\n";
    let mut store = registered_store(root.path(), swap_id, program);
    let config = MakerActorSupervisorConfig::new(Duration::from_secs(2), 5, 30, 8_192)
        .expect("bounded invalid-output config");

    let outcome = supervise_one_due_maker_actor(
        &mut store,
        MakerActorLeaseOwner::new([0x55; 16]).unwrap(),
        10,
        &config,
    )
    .expect("invalid-output cycle")
    .expect("one due actor");

    assert_eq!(outcome.resolution(), MakerActorSupervisorResolution::Failed);
    let record = store.list_maker_actor_processes().unwrap().remove(0);
    assert_eq!(record.schedule_state(), MakerActorScheduleState::Failed);
    assert_eq!(record.child_identity(), None);
}

#[test]
fn terminal_offline_status_resolves_without_an_effect_process() {
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let swap_id = "m5-supervisor-terminal";
    let invocation_log = root.path().join("terminal-invocations");
    let program = format!(
        "#!/bin/sh\n\
         test \"$1\" = \"--config-fd\" || exit 91\n\
         test \"$2\" = \"196\" || exit 92\n\
         printf '%s\\n' \"$3\" >> '{}'\n\
         test \"$3\" = \"status\" || exit 93\n\
         printf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"state\":\"active\",\"phase\":\"completed\",\"revision\":4,\"next_action\":\"complete\"}}'\n",
        invocation_log.display()
    );
    let mut store = registered_store(root.path(), swap_id, program.as_bytes());
    let config = MakerActorSupervisorConfig::new(Duration::from_secs(2), 5, 30, 8_192)
        .expect("bounded terminal config");

    let outcome = supervise_one_due_maker_actor(
        &mut store,
        MakerActorLeaseOwner::new([0x54; 16]).unwrap(),
        10,
        &config,
    )
    .expect("terminal cycle")
    .expect("one due actor");

    assert_eq!(
        outcome.resolution(),
        MakerActorSupervisorResolution::Terminal
    );
    assert_eq!(fs::read_to_string(invocation_log).unwrap(), "status\n");
    let record = store.list_maker_actor_processes().unwrap().remove(0);
    assert_eq!(record.schedule_state(), MakerActorScheduleState::Terminal);
    assert_eq!(record.child_identity(), None);
}

fn registered_store(root: &std::path::Path, swap_id: &str, program: &[u8]) -> SqliteSwapStore {
    let mut store = SqliteSwapStore::open(root.join(format!("{swap_id}-maker.sqlite3"))).unwrap();
    register_actor(&mut store, root, swap_id, program);
    store
}

fn register_actor(
    store: &mut SqliteSwapStore,
    root: &std::path::Path,
    swap_id: &str,
    program: &[u8],
) {
    let deployment = actor_deployment(root, swap_id);
    let actor_config = ActorConfig::load_private(&deployment.source_config).unwrap();
    let config_bytes = fs::read(&deployment.source_config).unwrap();
    let program_path = root.join(format!("{swap_id}-actor"));
    write_private(&program_path, program, 0o700);
    let program_sha256: [u8; 32] = Sha256::digest(program).into();
    store.save(&swap(swap_id)).unwrap();
    store
        .register_maker_actor(
            &MakerActorManifestV1::new(
                SwapId::new(swap_id).unwrap(),
                MakerActorKindV1::Zcash,
                deployment.source_config,
                Sha256::digest(config_bytes).into(),
                program_path,
                program_sha256,
                actor_config.role_state_db().to_path_buf(),
            )
            .unwrap(),
            10,
        )
        .unwrap();
    let record = store
        .list_maker_actor_processes()
        .unwrap()
        .into_iter()
        .find(|record| record.swap_id().as_str() == swap_id)
        .unwrap();
    prepare_maker_actor(&record).expect("exact deployment preflight");
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

fn write_private(path: &std::path::Path, bytes: &[u8], mode: u32) {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}
