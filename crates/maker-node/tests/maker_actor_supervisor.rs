//! Exercises the ZEC and XMR reference actors; needs `pair-zec` and `pair-xmr`.
#![cfg(all(feature = "pair-zec", feature = "pair-xmr"))]

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
    path::{Path, PathBuf},
    process::{Child, Command, Output},
    thread,
    time::{Duration, Instant},
};

use btc_fixture::BtcAuthorityFixture;
use btc_reference_actor::ActorConfig as BtcActorConfig;
use lez_bridge_protocol::RequestId;
use lez_maker_node::{
    MakerActorSupervisorCancellation, MakerActorSupervisorConfig, MakerActorSupervisorError,
    MakerActorSupervisorOutcome, MakerActorSupervisorResolution, clear_maker_lez_claim_admissions,
    prepare_maker_actor, supervise_one_due_maker_actor_until,
};
use lez_swap_core::{
    Chain, ChainPosition, ConfirmationPolicy, Pair, RecoverySchedule, SwapCoordinator,
    SwapDirection, SwapId, TimelockSafety,
};
use lez_swap_store::{
    ActorHeldLock, MakerActorAttemptResolution, MakerActorKindV1, MakerActorLeaseOwner,
    MakerActorManifestV1, MakerActorManualAction, MakerActorManualActionState,
    MakerActorProgressObservationV1, MakerActorScheduleState, SqliteSwapStore,
    validate_maker_actor_program,
};
use rustix::{
    process::{Pid, Signal, kill_process},
    time::{ClockId, clock_gettime},
};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use tempfile::tempdir;
use zec_reference_actor::ActorConfig;

use support::actor_deployment;
use xmr_chat_fixture::XmrChatFixture;

/// How long a test waits, after a pass, for its actor's lock to be released.
///
/// A pass ends with its actor process reaped and its lock dropped, and the
/// next pass of these tests starts at once, where a Node waits at least its
/// requeue delay (a second or more). The kernel releases the lock when the
/// last descriptor of it closes, and on macOS that can trail the pass: a
/// reaped child's `flock` can stay held for a few milliseconds after `wait`
/// returns (tens under load). A next pass in that window finds the lock taken
/// and backs off
/// (`actor_lock_unavailable`), which is right for a Node and not what the test
/// is about. Each pass here therefore waits until its actor's lock is free; a
/// lock still held after this bound is held by something real, and the test
/// fails on it.
const PASS_LOCK_RELEASE: Duration = Duration::from_secs(30);

fn assert_lock_released(
    store: &SqliteSwapStore,
    outcome: Result<Option<MakerActorSupervisorOutcome>, MakerActorSupervisorError>,
) -> Result<Option<MakerActorSupervisorOutcome>, MakerActorSupervisorError> {
    if let Ok(Some(outcome)) = &outcome {
        let record = store
            .maker_actor_process(outcome.swap_id())
            .unwrap()
            .expect("the supervised actor's row");
        let released = ActorHeldLock::acquire_for_within(
            record.swap_id(),
            record.manifest().state_database_path(),
            PASS_LOCK_RELEASE,
        );
        assert!(
            released.is_ok(),
            "{}: the actor's lock is still held after its pass: {released:?}",
            outcome.swap_id().as_str()
        );
    }
    outcome
}

fn supervise_one_due_maker_actor(
    store: &mut SqliteSwapStore,
    owner: MakerActorLeaseOwner,
    now: u64,
    config: &MakerActorSupervisorConfig,
) -> Result<Option<MakerActorSupervisorOutcome>, MakerActorSupervisorError> {
    let outcome = lez_maker_node::supervise_one_due_maker_actor(store, owner, now, config);
    assert_lock_released(store, outcome)
}

fn supervise_one_abandoned_maker_actor(
    store: &mut SqliteSwapStore,
    owner: MakerActorLeaseOwner,
    now: u64,
    config: &MakerActorSupervisorConfig,
) -> Result<Option<MakerActorSupervisorOutcome>, MakerActorSupervisorError> {
    let outcome = lez_maker_node::supervise_one_abandoned_maker_actor(store, owner, now, config);
    assert_lock_released(store, outcome)
}

fn boottime_milliseconds() -> u64 {
    #[cfg(target_os = "linux")]
    let now = clock_gettime(ClockId::Boottime);
    // The supervisor's macOS clock: CLOCK_MONOTONIC counts across sleep there.
    #[cfg(target_os = "macos")]
    let now = clock_gettime(ClockId::Monotonic);
    u64::try_from(now.tv_sec)
        .unwrap()
        .saturating_mul(1_000)
        .saturating_add(u64::try_from(now.tv_nsec).unwrap() / 1_000_000)
}
#[test]
fn xmr_pre_effect_cycle_validates_real_authority_and_never_invokes_an_effect() {
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let swap_bytes = [0x5a; 32];
    let swap_id = hex::encode(swap_bytes);
    let built_actor_program = std::path::Path::new(env!("CARGO_BIN_EXE_lez-xmr-maker-actor"));
    let actor_program = root.path().join("lez-xmr-maker-actor");
    fs::copy(built_actor_program, &actor_program).unwrap();
    fs::set_permissions(&actor_program, fs::Permissions::from_mode(0o700)).unwrap();
    let fixture = XmrChatFixture::new(root.path(), swap_bytes, 1_000_000, 25_000, &actor_program);
    let config_bytes = fs::read(&fixture.maker_actor_config).unwrap();
    let program_bytes = fs::read(&actor_program).unwrap();

    let mut store = SqliteSwapStore::open(root.path().join("xmr-maker.sqlite3")).unwrap();
    store.save(&xmr_swap(&swap_id)).unwrap();
    store
        .register_maker_actor(
            &MakerActorManifestV1::new(
                SwapId::new(swap_id.clone()).unwrap(),
                MakerActorKindV1::Monero,
                fixture.maker_actor_config,
                Sha256::digest(config_bytes).into(),
                actor_program,
                Sha256::digest(program_bytes).into(),
                fixture.maker_actor_state,
            )
            .unwrap(),
            10,
        )
        .unwrap();
    let record = store.list_maker_actor_processes().unwrap().remove(0);
    prepare_maker_actor(&record).expect("exact XMR deployment preflight");

    let config = MakerActorSupervisorConfig::new(Duration::from_secs(20), 5, 30, 8_192).unwrap();
    let outcome = supervise_one_due_maker_actor(
        &mut store,
        MakerActorLeaseOwner::new([0x58; 16]).unwrap(),
        10,
        &config,
    )
    .unwrap()
    .unwrap();

    assert_eq!(
        outcome.resolution(),
        MakerActorSupervisorResolution::Blocked
    );
    let record = store.list_maker_actor_processes().unwrap().remove(0);
    assert_eq!(record.schedule_state(), MakerActorScheduleState::Queued);
    assert_eq!(record.child_identity(), None);
    assert_eq!(
        record.attempt_count(),
        1,
        "attempt_count records one successful authority observation, not a failure retry"
    );
    let swap_id = SwapId::new(swap_id).unwrap();
    assert!(store.maker_actor_manual_action(&swap_id).unwrap().is_none());
    assert!(store.list_due_maker_actor_ids(69, 1).unwrap().is_empty());
    assert_eq!(
        store.list_due_maker_actor_ids(70, 1).unwrap(),
        std::slice::from_ref(&swap_id),
        "typed blocked authority is rechecked conservatively without backoff"
    );
    assert_eq!(
        store
            .maker_actor_progress(&swap_id)
            .unwrap()
            .unwrap()
            .observation(),
        &MakerActorProgressObservationV1::active(
            "offered",
            0,
            "xmr_chain_effects_not_yet_composed",
        )
        .unwrap()
    );
}

#[test]
fn queued_xmr_recover_overrides_typed_blocked_status_without_generic_effect() {
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let invocation_log = root.path().join("xmr-recover-invocations");
    let actor_program = root.path().join("xmr-recover-actor");
    let program = format!(
        "#!/bin/sh\n\
         test \"$1\" = \"--config-fd\" || exit 91\n\
         test \"$2\" = \"196\" || exit 92\n\
         printf \"%s\\n\" \"$3\" >> \"{}\"\n\
         case \"$3\" in\n\
           status) printf \"%s\\n\" \"{{\\\"schema_version\\\":1,\\\"actor_program\\\":\\\"lez-xmr-maker-actor\\\",\\\"actor_abi\\\":\\\"lez_maker_xmr_pre_effect_v1\\\",\\\"role\\\":\\\"maker\\\",\\\"state\\\":\\\"active\\\",\\\"phase\\\":\\\"offered\\\",\\\"revision\\\":0,\\\"next_action\\\":\\\"xmr_chain_effects_not_yet_composed\\\",\\\"chain_effect_executed\\\":false}}\" ;;\n\
           recover) printf \"%s\\n\" \"{{\\\"schema_version\\\":1,\\\"role\\\":\\\"maker\\\",\\\"command\\\":\\\"recover\\\",\\\"outcome\\\":\\\"awaiting_observation\\\",\\\"phase\\\":\\\"maker_recovery_available\\\",\\\"revision\\\":1,\\\"next_action\\\":\\\"xmr_chain_effects_not_yet_composed\\\"}}\" ;;\n\
           *) exit 95 ;;\n\
         esac\n",
        invocation_log.display()
    );
    write_private(&actor_program, program.as_bytes(), 0o700);
    let swap_bytes = [0x5b; 32];
    let fixture = XmrChatFixture::new(root.path(), swap_bytes, 1_000_000, 25_000, &actor_program);
    let config_bytes = fs::read(&fixture.maker_actor_config).unwrap();
    let program_bytes = fs::read(&actor_program).unwrap();
    let mut store = SqliteSwapStore::open(root.path().join("xmr-maker.sqlite3")).unwrap();
    let swap_id = hex::encode(swap_bytes);
    store.save(&xmr_swap(&swap_id)).unwrap();
    let id = SwapId::new(swap_id).unwrap();
    store
        .register_maker_actor(
            &MakerActorManifestV1::new(
                id.clone(),
                MakerActorKindV1::Monero,
                fixture.maker_actor_config,
                Sha256::digest(config_bytes).into(),
                actor_program,
                Sha256::digest(program_bytes).into(),
                fixture.maker_actor_state,
            )
            .unwrap(),
            10,
        )
        .unwrap();
    store
        .queue_maker_actor_manual_action(
            &RequestId::new("m7-xmr-recover-001").unwrap(),
            &id,
            MakerActorManualAction::Refund,
            0,
            10,
        )
        .unwrap();
    let config = MakerActorSupervisorConfig::new(Duration::from_secs(20), 5, 30, 8_192).unwrap();

    let outcome = supervise_one_due_maker_actor(
        &mut store,
        MakerActorLeaseOwner::new([0x73; 16]).unwrap(),
        10,
        &config,
    )
    .unwrap()
    .unwrap();

    assert_eq!(
        outcome.resolution(),
        MakerActorSupervisorResolution::Requeued
    );
    assert_eq!(
        fs::read_to_string(invocation_log).unwrap(),
        "status\nrecover\n"
    );
    assert_eq!(
        store
            .maker_actor_manual_action(&id)
            .unwrap()
            .unwrap()
            .state(),
        MakerActorManualActionState::Queued
    );
    assert_eq!(
        store
            .maker_actor_progress(&id)
            .unwrap()
            .unwrap()
            .observation(),
        &MakerActorProgressObservationV1::active(
            "maker_recovery_available",
            1,
            "xmr_chain_effects_not_yet_composed",
        )
        .unwrap()
    );
}

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
           activate) printf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"command\":\"activate\",\"outcome\":\"activated\",\"phase\":\"offered\",\"revision\":0,\"next_action\":\"create_and_fund_lez\"}}' ;;\n\
           *) exit 95 ;;\n\
         esac\n",
        invocation_log.display()
    );
    let program = actor_script(program);
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

    let config = MakerActorSupervisorConfig::new(Duration::from_secs(20), 5, 30, 8_192)
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
    assert_eq!(
        store
            .maker_actor_progress(&SwapId::new(swap_id).unwrap())
            .unwrap()
            .unwrap()
            .observation(),
        &MakerActorProgressObservationV1::active("offered", 0, "create_and_fund_lez").unwrap()
    );
    assert!(store.list_due_maker_actor_ids(14, 1).unwrap().is_empty());
    assert_eq!(
        store.list_due_maker_actor_ids(15, 1).unwrap(),
        [SwapId::new(swap_id).unwrap()]
    );
}

#[test]
fn manual_claim_invokes_only_claim_and_atomically_completes_action() {
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let swap_id = "m5-supervisor-manual-claim";
    let invocation_log = root.path().join("claim-invocations");
    let program = format!(
        "#!/bin/sh\n\
         test \"$1\" = \"--config-fd\" || exit 91\n\
         test \"$2\" = \"196\" || exit 92\n\
         printf '%s\\n' \"$3\" >> '{}'\n\
         case \"$3\" in\n\
           status) printf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"state\":\"active\",\"phase\":\"claim_evidence_available\",\"revision\":3,\"next_action\":\"claim_lez\"}}' ;;\n\
           claim) printf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"command\":\"claim\",\"outcome\":\"completed\",\"phase\":\"completed\",\"revision\":5,\"next_action\":\"complete\"}}' ;;\n\
           *) exit 95 ;;\n\
         esac\n",
        invocation_log.display()
    );
    let mut store = registered_store(root.path(), swap_id, program.as_bytes());
    let id = SwapId::new(swap_id).unwrap();
    store
        .queue_maker_actor_manual_action(
            &RequestId::new("m5-claim-001").unwrap(),
            &id,
            MakerActorManualAction::Claim,
            0,
            10,
        )
        .unwrap();
    let config = MakerActorSupervisorConfig::new(Duration::from_secs(20), 5, 30, 8_192).unwrap();

    let outcome = supervise_one_due_maker_actor(
        &mut store,
        MakerActorLeaseOwner::new([0x71; 16]).unwrap(),
        10,
        &config,
    )
    .unwrap()
    .unwrap();

    assert_eq!(
        outcome.resolution(),
        MakerActorSupervisorResolution::Terminal
    );
    assert_eq!(
        fs::read_to_string(invocation_log).unwrap(),
        "status\nclaim\n"
    );
    assert_eq!(
        store
            .maker_actor_manual_action(&id)
            .unwrap()
            .unwrap()
            .state(),
        MakerActorManualActionState::Completed
    );
    assert_eq!(
        store
            .list_maker_actor_processes()
            .unwrap()
            .remove(0)
            .schedule_state(),
        MakerActorScheduleState::Terminal
    );
    assert_eq!(
        store
            .maker_actor_progress(&id)
            .unwrap()
            .unwrap()
            .observation(),
        &MakerActorProgressObservationV1::active("completed", 5, "complete").unwrap()
    );
}

#[test]
fn a_makers_earlier_zcash_refund_first_observes_the_takers_zcash_claim() {
    // TakerSellsLez past the claim window: the actor names the Maker's Zcash
    // refund (the earlier one) with both legs locked. A Taker claim sent in
    // the window can still be mined and only a drive reads its preimage, so
    // the supervisor observes before it recovers.
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let swap_id = "m5-supervisor-earlier-zcash-refund";
    let invocation_log = root.path().join("earlier-refund-invocations");
    let program = format!(
        "#!/bin/sh\n\
         test \"$1\" = \"--config-fd\" || exit 91\n\
         test \"$2\" = \"196\" || exit 92\n\
         printf '%s\\n' \"$3\" >> '{}'\n\
         case \"$3\" in\n\
           status) printf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"state\":\"active\",\"phase\":\"both_legs_locked\",\"revision\":3,\"next_action\":\"refund_zcash\"}}' ;;\n\
           drive) printf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"command\":\"drive\",\"outcome\":\"awaiting_observation\",\"phase\":\"both_legs_locked\",\"revision\":3,\"next_action\":\"refund_zcash\"}}' ;;\n\
           recover) printf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"command\":\"recover\",\"outcome\":\"awaiting_deadline\",\"phase\":\"both_legs_locked\",\"revision\":3,\"next_action\":\"refund_zcash\"}}' ;;\n\
           *) exit 95 ;;\n\
         esac\n",
        invocation_log.display()
    );
    let mut store = registered_store(root.path(), swap_id, program.as_bytes());
    let config = MakerActorSupervisorConfig::new(Duration::from_secs(20), 5, 30, 8_192).unwrap();

    for now in [10, 20] {
        let outcome = supervise_one_due_maker_actor(
            &mut store,
            MakerActorLeaseOwner::new([0x73; 16]).unwrap(),
            now,
            &config,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            outcome.resolution(),
            MakerActorSupervisorResolution::Requeued
        );
    }
    let invocations = fs::read_to_string(invocation_log).unwrap();
    let effects: Vec<_> = invocations
        .lines()
        .filter(|command| *command != "status")
        .collect();
    assert_eq!(effects.len(), 2, "{invocations}");
    assert!(
        effects.contains(&"drive") && effects.contains(&"recover"),
        "the earlier refund must alternate with observing the claim: {invocations}"
    );
}

/// A Zcash Maker actor script that logs each command and reports `status`
/// with `next_action` in `phase`; its claim is submitted.
fn claim_logging_program(log: &Path, phase: &str, next_action: &str) -> String {
    format!(
        "#!/bin/sh\n\
         test \"$1\" = \"--config-fd\" || exit 91\n\
         test \"$2\" = \"196\" || exit 92\n\
         printf '%s\\n' \"$3\" >> '{}'\n\
         case \"$3\" in\n\
           status) printf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"state\":\"active\",\"phase\":\"{phase}\",\"revision\":4,\"next_action\":\"{next_action}\"}}' ;;\n\
           claim) printf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"command\":\"claim\",\"outcome\":\"submitted\",\"phase\":\"{phase}\",\"revision\":4,\"next_action\":\"{next_action}\"}}' ;;\n\
           *) exit 95 ;;\n\
         esac\n",
        log.display()
    )
}

#[test]
fn a_makers_followup_lez_claim_waits_behind_another_swaps_lez_claim_in_flight() {
    // TakerSellsLez: the Maker's follow-up LEZ claim is planned at its one
    // LEZ account's nonce, so it waits while another swap's LEZ claim is in
    // flight (its marker written on submission). Its Zcash claim
    // (TakerSellsForeign) spends no LEZ nonce and goes ahead.
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut store = SqliteSwapStore::open(root.path().join("gate-maker.sqlite3")).unwrap();
    let holder_root = root.path().join("holder");
    fs::create_dir(&holder_root).unwrap();
    fs::set_permissions(&holder_root, fs::Permissions::from_mode(0o700)).unwrap();
    let holder_log = root.path().join("holder-invocations");
    register_actor(
        &mut store,
        &holder_root,
        "m5-gate-holder",
        claim_logging_program(&holder_log, "claim_evidence_available", "wait").as_bytes(),
    );
    let holder_state =
        ActorConfig::load_private(&actor_deployment(&holder_root, "m5-gate-holder").source_config)
            .unwrap()
            .role_state_db()
            .to_path_buf();
    let marker = zec_reference_actor::revealing_claim_in_flight_marker(&holder_state).unwrap();
    fs::write(&marker, b"").unwrap();
    // The holder is mid-attempt elsewhere: leased, not terminal, not due here.
    store
        .claim_maker_actor(
            &SwapId::new("m5-gate-holder").unwrap(),
            MakerActorLeaseOwner::new([0x74; 16]).unwrap(),
            10,
        )
        .unwrap()
        .expect("holder lease");

    for (swap_id, next_action, expected_class, expected_log) in [
        (
            "m5-gate-followup-lez",
            "claim_lez",
            Some("lez_revealing_claim_in_flight"),
            "status\n",
        ),
        (
            "m5-gate-followup-zcash",
            "claim_zcash",
            None,
            "status\nclaim\n",
        ),
    ] {
        let swap_root = root.path().join(swap_id);
        fs::create_dir(&swap_root).unwrap();
        fs::set_permissions(&swap_root, fs::Permissions::from_mode(0o700)).unwrap();
        let log = root.path().join(format!("{swap_id}-invocations"));
        register_actor(
            &mut store,
            &swap_root,
            swap_id,
            claim_logging_program(&log, "claim_evidence_available", next_action).as_bytes(),
        );
        let id = SwapId::new(swap_id).unwrap();
        let config =
            MakerActorSupervisorConfig::new(Duration::from_secs(20), 5, 30, 8_192).unwrap();
        let outcome = supervise_one_due_maker_actor(
            &mut store,
            MakerActorLeaseOwner::new([0x75; 16]).unwrap(),
            10,
            &config,
        )
        .unwrap()
        .expect("the due actor runs");
        assert_eq!(outcome.swap_id(), &id);
        let record = store
            .list_maker_actor_processes()
            .unwrap()
            .into_iter()
            .find(|record| record.swap_id() == &id)
            .unwrap();
        match expected_class {
            Some(class) => {
                assert_eq!(
                    outcome.resolution(),
                    MakerActorSupervisorResolution::Backoff
                );
                assert_eq!(record.last_failure_class(), Some(class));
            }
            None => assert_eq!(
                outcome.resolution(),
                MakerActorSupervisorResolution::Requeued
            ),
        }
        assert_eq!(fs::read_to_string(&log).unwrap(), expected_log, "{swap_id}");
    }
    assert!(!holder_log.exists(), "the holder itself is not run here");
}

/// A Zcash Maker actor script for one swap at `claim_evidence_available`
/// whose `claim` behaves as the actor's does at the one-claim gate: the first
/// claim writes the in-flight marker (and drops the waiting marker the Node
/// wrote) and is `submitted`; the next one finds the claim final, removes the
/// in-flight marker and completes. Each submission is appended to `claims`.
fn gated_lez_claim_program(
    swap_id: &str,
    state_db: &Path,
    log: &Path,
    claims: &Path,
    lez_refund_at_ms: u64,
) -> String {
    let in_flight = zec_reference_actor::revealing_claim_in_flight_marker(state_db).unwrap();
    let waiting = zec_reference_actor::lez_claim_waiting_marker(state_db).unwrap();
    let done = state_db.with_file_name("claim-final");
    format!(
        "#!/bin/sh\n\
         test \"$1\" = \"--config-fd\" || exit 91\n\
         test \"$2\" = \"196\" || exit 92\n\
         printf '%s\\n' \"$3\" >> '{log}'\n\
         case \"$3\" in\n\
           status)\n\
             if [ -e '{done}' ]; then\n\
               printf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"state\":\"active\",\"phase\":\"completed\",\"revision\":6,\"next_action\":\"complete\"}}'\n\
             else\n\
               printf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"state\":\"active\",\"phase\":\"claim_evidence_available\",\"revision\":4,\"next_action\":\"claim_lez\",\"lez_refund_at_ms\":{lez_refund_at_ms},\"revealing_claim_hold_attention_ms\":60000}}'\n\
             fi ;;\n\
           claim)\n\
             if [ -e '{in_flight}' ]; then\n\
               rm -f '{in_flight}'; : > '{done}'\n\
               printf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"command\":\"claim\",\"outcome\":\"completed\",\"phase\":\"completed\",\"revision\":6,\"next_action\":\"complete\"}}'\n\
             else\n\
               : > '{in_flight}'; rm -f '{waiting}'; printf '%s\\n' '{swap_id}' >> '{claims}'\n\
               printf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"command\":\"claim\",\"outcome\":\"submitted\",\"phase\":\"claim_evidence_available\",\"revision\":5,\"next_action\":\"claim_lez\"}}'\n\
             fi ;;\n\
           *) exit 95 ;;\n\
         esac\n",
        log = log.display(),
        done = done.display(),
        in_flight = in_flight.display(),
        waiting = waiting.display(),
        claims = claims.display(),
    )
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one journey: both claims held, then admitted in refund order, then final"
)]
fn two_outstanding_maker_lez_claims_go_one_at_a_time_earliest_refund_first() {
    // TakerSellsLez: the Taker revealed on two swaps while a third swap's
    // Maker LEZ claim was in flight. Both follow-up LEZ claims wait at the
    // one-claim gate. When the holder's claim is final the gate admits the
    // swap whose LEZ refund time is earliest, not the one scheduled first,
    // and the other waits behind it; both complete before their refund times.
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut store = SqliteSwapStore::open(root.path().join("order-maker.sqlite3")).unwrap();
    let claims = root.path().join("lez-claims-submitted");
    let now_ms = u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap();

    let holder_root = root.path().join("holder");
    fs::create_dir(&holder_root).unwrap();
    fs::set_permissions(&holder_root, fs::Permissions::from_mode(0o700)).unwrap();
    register_actor(
        &mut store,
        &holder_root,
        "m5-order-holder",
        claim_logging_program(
            &root.path().join("holder-log"),
            "claim_evidence_available",
            "wait",
        )
        .as_bytes(),
    );
    let holder_state =
        ActorConfig::load_private(&actor_deployment(&holder_root, "m5-order-holder").source_config)
            .unwrap()
            .role_state_db()
            .to_path_buf();
    let holder_marker =
        zec_reference_actor::revealing_claim_in_flight_marker(&holder_state).unwrap();
    fs::write(&holder_marker, b"").unwrap();
    store
        .claim_maker_actor(
            &SwapId::new("m5-order-holder").unwrap(),
            MakerActorLeaseOwner::new([0x76; 16]).unwrap(),
            10,
        )
        .unwrap()
        .expect("holder lease");

    // `a` is scheduled first (swap id order) but refunds later than `b`.
    let swaps = [
        ("m5-order-a-later-refund", now_ms + 3_600_000),
        ("m5-order-b-earlier-refund", now_ms + 1_800_000),
    ];
    for (swap_id, refund_at_ms) in swaps {
        let swap_root = root.path().join(swap_id);
        fs::create_dir(&swap_root).unwrap();
        fs::set_permissions(&swap_root, fs::Permissions::from_mode(0o700)).unwrap();
        let state = ActorConfig::load_private(&actor_deployment(&swap_root, swap_id).source_config)
            .unwrap()
            .role_state_db()
            .to_path_buf();
        let program = gated_lez_claim_program(
            swap_id,
            &state,
            &root.path().join(format!("{swap_id}-log")),
            &claims,
            refund_at_ms,
        );
        register_actor(&mut store, &swap_root, swap_id, program.as_bytes());
    }
    let config = MakerActorSupervisorConfig::new(Duration::from_secs(20), 5, 30, 8_192).unwrap();
    let owner = MakerActorLeaseOwner::new([0x77; 16]).unwrap();
    let run = |store: &mut SqliteSwapStore, now: u64| {
        let outcome = supervise_one_due_maker_actor(store, owner, now, &config)
            .unwrap()
            .expect("a due actor runs");
        let class = store
            .list_maker_actor_processes()
            .unwrap()
            .into_iter()
            .find(|record| record.swap_id() == outcome.swap_id())
            .unwrap()
            .last_failure_class()
            .map(str::to_owned);
        (
            outcome.swap_id().as_str().to_owned(),
            outcome.resolution(),
            class,
        )
    };

    // Both claims wait behind the holder's.
    for expected in ["m5-order-a-later-refund", "m5-order-b-earlier-refund"] {
        let (swap, resolution, class) = run(&mut store, 10);
        assert_eq!(swap, expected);
        assert_eq!(resolution, MakerActorSupervisorResolution::Backoff);
        assert_eq!(class.as_deref(), Some("lez_revealing_claim_in_flight"));
    }
    // The holder's claim is final.
    fs::remove_file(&holder_marker).unwrap();

    // `a` is due first and waits for `b`, whose refund time is earlier.
    let (swap, resolution, class) = run(&mut store, 40);
    assert_eq!(swap, "m5-order-a-later-refund");
    assert_eq!(resolution, MakerActorSupervisorResolution::Backoff);
    assert_eq!(class.as_deref(), Some("lez_claim_waits_for_earlier_refund"));
    let (swap, resolution, _) = run(&mut store, 40);
    assert_eq!(swap, "m5-order-b-earlier-refund");
    assert_eq!(resolution, MakerActorSupervisorResolution::Requeued);
    // `b`'s claim is in flight: `a` keeps waiting, now behind the nonce.
    let (swap, resolution, _) = run(&mut store, 45);
    assert_eq!(swap, "m5-order-b-earlier-refund");
    assert_eq!(resolution, MakerActorSupervisorResolution::Terminal);
    let (swap, resolution, _) = run(&mut store, 70);
    assert_eq!(swap, "m5-order-a-later-refund");
    assert_eq!(resolution, MakerActorSupervisorResolution::Requeued);
    let (swap, resolution, _) = run(&mut store, 75);
    assert_eq!(swap, "m5-order-a-later-refund");
    assert_eq!(resolution, MakerActorSupervisorResolution::Terminal);

    assert_eq!(
        fs::read_to_string(&claims).unwrap(),
        "m5-order-b-earlier-refund\nm5-order-a-later-refund\n",
        "the earlier refund's claim is submitted first, one at a time"
    );
    let finished_ms = u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap();
    for (swap_id, refund_at_ms) in swaps {
        assert!(
            finished_ms < refund_at_ms,
            "{swap_id} completed before its LEZ refund time"
        );
    }
}

#[test]
fn manual_refund_invokes_only_recover_and_atomically_completes_action() {
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let swap_id = "m5-supervisor-manual-refund";
    let invocation_log = root.path().join("refund-invocations");
    let program = format!(
        "#!/bin/sh\n\
         test \"$1\" = \"--config-fd\" || exit 91\n\
         test \"$2\" = \"196\" || exit 92\n\
         printf '%s\\n' \"$3\" >> '{}'\n\
         case \"$3\" in\n\
           status) printf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"state\":\"active\",\"phase\":\"both_legs_locked\",\"revision\":3,\"next_action\":\"wait\"}}' ;;\n\
           recover) printf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"command\":\"recover\",\"outcome\":\"refunded\",\"phase\":\"refunded\",\"revision\":5,\"next_action\":\"complete\"}}' ;;\n\
           *) exit 95 ;;\n\
         esac\n",
        invocation_log.display()
    );
    let mut store = registered_store(root.path(), swap_id, program.as_bytes());
    let id = SwapId::new(swap_id).unwrap();
    store
        .queue_maker_actor_manual_action(
            &RequestId::new("m5-refund-001").unwrap(),
            &id,
            MakerActorManualAction::Refund,
            0,
            10,
        )
        .unwrap();
    let config = MakerActorSupervisorConfig::new(Duration::from_secs(20), 5, 30, 8_192).unwrap();

    let outcome = supervise_one_due_maker_actor(
        &mut store,
        MakerActorLeaseOwner::new([0x72; 16]).unwrap(),
        10,
        &config,
    )
    .unwrap()
    .unwrap();

    assert_eq!(
        outcome.resolution(),
        MakerActorSupervisorResolution::Terminal
    );
    assert_eq!(
        fs::read_to_string(invocation_log).unwrap(),
        "status\nrecover\n"
    );
    assert_eq!(
        store
            .maker_actor_manual_action(&id)
            .unwrap()
            .unwrap()
            .state(),
        MakerActorManualActionState::Completed
    );
    assert_eq!(
        store
            .list_maker_actor_processes()
            .unwrap()
            .remove(0)
            .schedule_state(),
        MakerActorScheduleState::Terminal
    );
    assert_eq!(
        store
            .maker_actor_progress(&id)
            .unwrap()
            .unwrap()
            .observation(),
        &MakerActorProgressObservationV1::active("refunded", 5, "complete").unwrap()
    );
}

#[test]
fn all_pair_manual_actions_execute_semantic_commands_and_replay_after_restart() {
    run_all_pair_manual_action_matrix();
}

#[derive(Clone, Copy)]
struct ManualActionMatrixCase {
    label: &'static str,
    seed: u8,
    kind: MakerActorKindV1,
    action: MakerActorManualAction,
    expected_command: &'static str,
    terminal_phase: &'static str,
}

fn run_all_pair_manual_action_matrix() {
    let root = tempdir().expect("isolated all-pair manual-action root");
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let cases = [
        ManualActionMatrixCase {
            label: "btc-claim",
            seed: 0x81,
            kind: MakerActorKindV1::Bitcoin,
            action: MakerActorManualAction::Claim,
            expected_command: "drive",
            terminal_phase: "completed",
        },
        ManualActionMatrixCase {
            label: "btc-refund",
            seed: 0x82,
            kind: MakerActorKindV1::Bitcoin,
            action: MakerActorManualAction::Refund,
            expected_command: "recover",
            terminal_phase: "refunded",
        },
        ManualActionMatrixCase {
            label: "xmr-claim",
            seed: 0x83,
            kind: MakerActorKindV1::Monero,
            action: MakerActorManualAction::Claim,
            expected_command: "claim",
            terminal_phase: "completed",
        },
        ManualActionMatrixCase {
            label: "xmr-refund",
            seed: 0x84,
            kind: MakerActorKindV1::Monero,
            action: MakerActorManualAction::Refund,
            expected_command: "recover",
            terminal_phase: "refunded",
        },
        ManualActionMatrixCase {
            label: "zec-claim",
            seed: 0x85,
            kind: MakerActorKindV1::Zcash,
            action: MakerActorManualAction::Claim,
            expected_command: "claim",
            terminal_phase: "completed",
        },
        ManualActionMatrixCase {
            label: "zec-refund",
            seed: 0x86,
            kind: MakerActorKindV1::Zcash,
            action: MakerActorManualAction::Refund,
            expected_command: "recover",
            terminal_phase: "refunded",
        },
    ];

    for case in cases {
        assert_manual_action_case(root.path(), case);
    }
}

#[allow(clippy::too_many_lines)] // Keep one user-shaped CLI, restart, supervisor, and replay journey visible.
fn assert_manual_action_case(root: &Path, case: ManualActionMatrixCase) {
    let case_root = root.join(case.label);
    fs::create_dir(&case_root).unwrap();
    fs::set_permissions(&case_root, fs::Permissions::from_mode(0o700)).unwrap();
    let invocation_log = case_root.join("invocations");
    let program_path = case_root.join("actor");
    let program = actor_script(matrix_actor_program(case, &invocation_log));
    write_private(&program_path, program.as_bytes(), 0o700);

    let database = case_root.join("maker.sqlite3");
    let mut store = SqliteSwapStore::open(&database).unwrap();
    let swap_id = register_matrix_actor(&mut store, &case_root, &program_path, &program, case);
    let request_id = RequestId::new(format!("m7-{}-request", case.label)).unwrap();
    drop(store);

    let runtime = case_root.join("runtime");
    fs::create_dir(&runtime).unwrap();
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).unwrap();
    let socket = runtime.join("maker.sock");
    let ready = runtime.join("ready");
    let mut daemon = spawn_matrix_daemon(&database, &socket, &ready, false);
    wait_for_matrix_daemon(&mut daemon, &ready, case.label);

    let action_name = match case.action {
        MakerActorManualAction::Claim => "claim",
        MakerActorManualAction::Refund => "refund",
    };
    let action_arguments = [
        action_name,
        "--id",
        swap_id.as_str(),
        "--request-id",
        request_id.as_str(),
        "--expected-generation",
        "0",
    ];
    let first = matrix_maker_cli(&socket, &action_arguments);
    assert_matrix_cli_success(&first, case.label);
    let first: Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(first["swap_id"], swap_id.as_str());
    assert_eq!(first["action"], action_name);
    assert_eq!(first["was_replay"], false);

    kill_process(Pid::from_child(&daemon), Signal::TERM).unwrap();
    let status = daemon.wait().unwrap();
    assert!(
        status.success(),
        "{} admission daemon did not stop cleanly",
        case.label
    );
    daemon = spawn_matrix_daemon(&database, &socket, &ready, true);
    wait_for_matrix_daemon(&mut daemon, &ready, case.label);

    let deadline = Instant::now() + Duration::from_secs(15);
    let terminal = loop {
        let monitor = matrix_maker_cli(&socket, &["monitor", "--id", swap_id.as_str()]);
        assert_matrix_cli_success(&monitor, case.label);
        let monitor: Value = serde_json::from_slice(&monitor.stdout).unwrap();
        if monitor["schedule_state"] == "terminal"
            && monitor["manual_action"]["state"] == "completed"
        {
            break monitor;
        }
        if let Some(status) = daemon.try_wait().unwrap() {
            panic!(
                "{} daemon exited before terminal action: {status}",
                case.label
            );
        }
        assert!(
            Instant::now() < deadline,
            "{} did not terminalize through CLI/daemon supervision: {monitor}",
            case.label
        );
        thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(terminal["lease_generation"], 1);
    assert_eq!(terminal["attempt_count"], 1);
    assert_eq!(
        terminal["progress"]["observation"]["phase"],
        case.terminal_phase
    );
    assert_eq!(terminal["progress"]["observation"]["revision"], 4);
    assert_eq!(
        terminal["progress"]["observation"]["next_action"],
        "complete"
    );
    let expected_invocations = format!("status\n{}\n", case.expected_command);
    assert_eq!(
        fs::read_to_string(&invocation_log).unwrap(),
        expected_invocations,
        "{} invoked the wrong pair-semantic command",
        case.label
    );
    let replay = matrix_maker_cli(&socket, &action_arguments);
    assert_matrix_cli_success(&replay, case.label);
    let replay: Value = serde_json::from_slice(&replay.stdout).unwrap();
    assert_eq!(replay["was_replay"], true);
    assert_eq!(
        fs::read_to_string(&invocation_log).unwrap(),
        expected_invocations,
        "{} replay invoked a second actor effect",
        case.label
    );
    let new_request = RequestId::new(format!("m7-{}-post-terminal", case.label)).unwrap();
    let rejected = matrix_maker_cli(
        &socket,
        &[
            action_name,
            "--id",
            swap_id.as_str(),
            "--request-id",
            new_request.as_str(),
            "--expected-generation",
            "1",
        ],
    );
    assert!(!rejected.status.success());

    kill_process(Pid::from_child(&daemon), Signal::TERM).unwrap();
    let status = daemon.wait().unwrap();
    assert!(
        status.success(),
        "{} daemon did not stop cleanly",
        case.label
    );
    let reopened = SqliteSwapStore::open(&database).unwrap();
    let action = reopened
        .maker_actor_manual_action(&swap_id)
        .unwrap()
        .unwrap();
    assert_eq!(action.state(), MakerActorManualActionState::Completed);
    assert_eq!(action.lease_generation(), None);
    let record = reopened
        .list_maker_actor_processes()
        .unwrap()
        .into_iter()
        .find(|record| record.swap_id() == &swap_id)
        .unwrap();
    assert_eq!(record.schedule_state(), MakerActorScheduleState::Terminal);
    assert_eq!(record.attempt_count(), 1);
    assert_eq!(record.child_identity(), None);
    assert_eq!(
        reopened
            .maker_actor_progress(&swap_id)
            .unwrap()
            .unwrap()
            .observation(),
        &MakerActorProgressObservationV1::active(case.terminal_phase, 4, "complete").unwrap()
    );
}

fn wait_for_matrix_daemon(daemon: &mut Child, ready: &Path, label: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if ready.exists() {
            return;
        }
        if let Some(status) = daemon.try_wait().unwrap() {
            panic!("{label} daemon exited before readiness: {status}");
        }
        assert!(
            Instant::now() < deadline,
            "{label} daemon readiness timed out"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn spawn_matrix_daemon(database: &Path, socket: &Path, ready: &Path, supervise: bool) -> Child {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lez-maker-node"));
    command
        .arg("--socket")
        .arg(socket)
        .arg("--database")
        .arg(database)
        .arg("--ready-file")
        .arg(ready);
    if supervise {
        command
            .arg("--actor-supervisor")
            .arg("--actor-worker-count")
            .arg("1")
            .arg("--actor-poll-milliseconds")
            .arg("10")
            .arg("--actor-attempt-timeout-milliseconds")
            .arg("5000");
    }
    command.spawn().unwrap()
}

fn matrix_maker_cli(socket: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lez-maker-cli"))
        .arg("--socket")
        .arg(socket)
        .args(arguments)
        .output()
        .unwrap()
}

fn assert_matrix_cli_success(output: &Output, label: &str) {
    assert!(
        output.status.success(),
        "{label} CLI failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn register_matrix_actor(
    store: &mut SqliteSwapStore,
    root: &Path,
    program_path: &Path,
    program: &str,
    case: ManualActionMatrixCase,
) -> SwapId {
    let (swap_id, coordinator, config_path, state_path) = match case.kind {
        MakerActorKindV1::Bitcoin => {
            let bytes = [case.seed; 32];
            let id = hex::encode(bytes);
            let fixture = BtcAuthorityFixture::new(root, case.label, bytes);
            let config = BtcActorConfig::load_private(&fixture.maker_source_config).unwrap();
            (
                SwapId::new(id.clone()).unwrap(),
                btc_swap(&id),
                fixture.maker_source_config,
                config.state_db().to_path_buf(),
            )
        }
        MakerActorKindV1::Monero => {
            let bytes = [case.seed; 32];
            let id = hex::encode(bytes);
            let fixture = XmrChatFixture::new(root, bytes, 1_000_000, 25_000, program_path);
            (
                SwapId::new(id.clone()).unwrap(),
                xmr_swap(&id),
                fixture.maker_actor_config,
                fixture.maker_actor_state,
            )
        }
        MakerActorKindV1::Zcash => {
            let id = format!("m7-{}", case.label);
            let deployment = actor_deployment(root, &id);
            let config = ActorConfig::load_private(&deployment.source_config).unwrap();
            (
                SwapId::new(id.clone()).unwrap(),
                swap(&id),
                deployment.source_config,
                config.role_state_db().to_path_buf(),
            )
        }
    };
    store.save(&coordinator).unwrap();
    store
        .register_maker_actor(
            &MakerActorManifestV1::new(
                swap_id.clone(),
                case.kind,
                config_path.clone(),
                Sha256::digest(fs::read(config_path).unwrap()).into(),
                program_path.to_path_buf(),
                Sha256::digest(program.as_bytes()).into(),
                state_path,
            )
            .unwrap(),
            10,
        )
        .unwrap();
    let record = store
        .list_maker_actor_processes()
        .unwrap()
        .into_iter()
        .find(|record| record.swap_id() == &swap_id)
        .unwrap();
    prepare_maker_actor(&record).expect("matrix actor deployment preflight");
    swap_id
}

fn matrix_actor_program(case: ManualActionMatrixCase, invocation_log: &Path) -> String {
    let status = match case.kind {
        MakerActorKindV1::Bitcoin => {
            r#"{"schema_version":1,"role":"maker","state":"active","phase":"offered","revision":0,"next_action":"observe_taker_first_lock"}"#
        }
        MakerActorKindV1::Monero => {
            r#"{"schema_version":1,"actor_program":"lez-xmr-maker-actor","actor_abi":"lez_maker_xmr_pre_effect_v1","role":"maker","state":"active","phase":"offered","revision":0,"next_action":"xmr_chain_effects_not_yet_composed","chain_effect_executed":false}"#
        }
        MakerActorKindV1::Zcash => {
            r#"{"schema_version":1,"role":"maker","state":"active","phase":"both_legs_locked","revision":3,"next_action":"wait"}"#
        }
    };
    let outcome = match case.kind {
        MakerActorKindV1::Bitcoin => "observed_then_projected",
        MakerActorKindV1::Monero | MakerActorKindV1::Zcash => case.terminal_phase,
    };
    format!(
        "#!/bin/sh\n\
         test \"$1\" = \"--config-fd\" || exit 91\n\
         test \"$2\" = \"196\" || exit 92\n\
         test -r /proc/self/fd/196 || exit 93\n\
         printf '%s\\n' \"$3\" >> '{}'\n\
         case \"$3\" in\n\
           status) printf '%s\\n' '{}' ;;\n\
           {}) printf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"command\":\"{}\",\"outcome\":\"{}\",\"phase\":\"{}\",\"revision\":4,\"next_action\":\"complete\"}}' ;;\n\
           *) exit 95 ;;\n\
         esac\n",
        invocation_log.display(),
        status,
        case.expected_command,
        case.expected_command,
        outcome,
        case.terminal_phase,
    )
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
          activate) printf '%s\\n' '{\"schema_version\":1,\"role\":\"maker\",\"command\":\"activate\",\"outcome\":\"activated\",\"phase\":\"offered\",\"revision\":0,\"next_action\":\"create_and_fund_lez\"}' ;;\n\
          *) exit 95 ;;\n\
        esac\n";
    let program: &[u8] = &actor_script_bytes(program);
    let mut store = registered_store(root.path(), swap_id, program);
    let swap_id = SwapId::new(swap_id).unwrap();
    let stale_owner = MakerActorLeaseOwner::new([0x61; 16]).unwrap();
    let stale_lease = store
        .claim_maker_actor(&swap_id, stale_owner, 10)
        .unwrap()
        .unwrap();
    assert_eq!(stale_lease.generation(), 1);

    let config = MakerActorSupervisorConfig::new(Duration::from_secs(20), 5, 30, 8_192)
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
    let old_process_lock = ActorHeldLock::acquire(&locked_record).unwrap();
    let new_owner = MakerActorLeaseOwner::new([0x64; 16]).unwrap();
    let config = MakerActorSupervisorConfig::new(Duration::from_secs(20), 5, 30, 8_192)
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
fn a_lease_another_worker_just_claimed_is_not_recovered_as_abandoned() {
    // Workers of one Node share its lease owner. A worker claims the row and
    // then takes the actor's lock; in between, the lock is free. Another
    // worker's recovery pass took that lease for abandoned, transferred it and
    // ran the actor, and the claiming worker, finding the lock taken, could no
    // longer resolve its own lease ("lease conflicts with durable owner"):
    // its error stopped the supervisor and the Node.
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let swap_id = "m5-supervisor-sibling-claim";
    let invoked = root.path().join("invoked");
    let program = format!(
        "#!/bin/sh\n\
         printf '%s\\n' \"$3\" >> '{}'\n\
         exit 95\n",
        invoked.display()
    );
    let mut store = registered_store(root.path(), swap_id, program.as_bytes());
    let swap_id = SwapId::new(swap_id).unwrap();
    let node = MakerActorLeaseOwner::new([0x65; 16]).unwrap();
    let config = MakerActorSupervisorConfig::new(Duration::from_secs(20), 5, 30, 8_192).unwrap();

    // One worker has claimed the row and not yet taken the lock.
    let claimed = store
        .claim_maker_actor(&swap_id, node, 10)
        .unwrap()
        .unwrap();

    // Another worker's recovery pass leaves it alone.
    assert!(
        supervise_one_abandoned_maker_actor(&mut store, node, 10, &config)
            .unwrap()
            .is_none(),
        "a lease of this Node's own owner is a live worker's, not abandoned"
    );
    assert!(!invoked.exists(), "no actor ran for the recovery pass");
    let record = store.list_maker_actor_processes().unwrap().remove(0);
    assert_eq!(record.schedule_state(), MakerActorScheduleState::Leased);
    assert_eq!(record.lease_generation(), claimed.generation());

    // The claiming worker still owns its lease and resolves it.
    store
        .resolve_maker_actor_attempt(
            &claimed,
            MakerActorAttemptResolution::Requeue { not_before: 15 },
            10,
        )
        .expect("the claiming worker resolves its own lease");

    // A lease another Node left is still recovered.
    let stale = store
        .claim_maker_actor(&swap_id, MakerActorLeaseOwner::new([0x66; 16]).unwrap(), 20)
        .unwrap()
        .unwrap();
    let recovered = supervise_one_abandoned_maker_actor(&mut store, node, 20, &config)
        .unwrap()
        .expect("an abandoned lease of another owner is recovered");
    assert_eq!(recovered.generation(), stale.generation() + 1);
}

#[test]
fn expired_effect_cutoff_leaves_due_actor_untouched_and_spawns_nothing() {
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let swap_id = "m5-supervisor-expired-cutoff";
    let invocation_marker = root.path().join("invoked");
    let program = format!(
        "#!/bin/sh\nprintf invoked > '{}'\n",
        invocation_marker.display()
    );
    let mut store = registered_store(root.path(), swap_id, program.as_bytes());
    let config = MakerActorSupervisorConfig::new(Duration::from_secs(40), 5, 30, 8_192)
        .unwrap()
        .with_effect_cutoff_boottime_milliseconds(boottime_milliseconds().saturating_add(50))
        .expect("bounded effect cutoff");
    std::thread::sleep(Duration::from_millis(80));

    let outcome = supervise_one_due_maker_actor(
        &mut store,
        MakerActorLeaseOwner::new([0x59; 16]).unwrap(),
        10,
        &config,
    )
    .expect("expired cutoff cycle");

    assert!(outcome.is_none());
    assert!(!invocation_marker.exists());
    let record = store.list_maker_actor_processes().unwrap().remove(0);
    assert_eq!(record.schedule_state(), MakerActorScheduleState::Queued);
    assert_eq!(record.attempt_count(), 0);
}

#[test]
fn effect_cutoff_kills_inflight_actor_and_clears_child_identity() {
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let swap_id = "m5-supervisor-inflight-cutoff";
    let started_marker = root.path().join("effect-started");
    let completed_marker = root.path().join("effect-completed");
    let program = format!(
        "#!/bin/sh\n\
         test \"$1\" = \"--config-fd\" || exit 91\n\
         test \"$2\" = \"196\" || exit 92\n\
         case \"$3\" in\n\
           status) printf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"state\":\"active\",\"phase\":\"claim_evidence_available\",\"revision\":3,\"next_action\":\"claim_lez\"}}' ;;\n\
           claim) printf started > '{}'; sleep 30; printf completed > '{}'; printf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"command\":\"claim\",\"outcome\":\"completed\",\"phase\":\"completed\",\"revision\":4,\"next_action\":\"complete\"}}' ;;\n\
           *) exit 93 ;;\n\
         esac\n",
        started_marker.display(),
        completed_marker.display()
    );
    let mut store = registered_store(root.path(), swap_id, program.as_bytes());
    let config = MakerActorSupervisorConfig::new(Duration::from_secs(20), 5, 30, 8_192)
        .unwrap()
        .with_effect_cutoff_boottime_milliseconds(boottime_milliseconds().saturating_add(8_000))
        .expect("bounded in-flight effect cutoff");
    let started = Instant::now();

    let outcome = supervise_one_due_maker_actor(
        &mut store,
        MakerActorLeaseOwner::new([0x5a; 16]).unwrap(),
        10,
        &config,
    )
    .expect("in-flight cutoff cycle")
    .expect("one due actor");

    // The child sleeps 30 s; stopped at the 8 s cutoff (room for a loaded
    // runner to start it first) it never completes.
    assert!(started.elapsed() < Duration::from_secs(25));
    assert_eq!(
        outcome.resolution(),
        MakerActorSupervisorResolution::Backoff
    );
    assert!(started_marker.exists());
    assert!(!completed_marker.exists());
    let record = store.list_maker_actor_processes().unwrap().remove(0);
    assert_eq!(record.schedule_state(), MakerActorScheduleState::Backoff);
    assert_eq!(record.child_identity(), None);
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
    let program: &[u8] = &actor_script_bytes(program);
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
    let config = MakerActorSupervisorConfig::new(Duration::from_secs(20), 5, 30, 8_192)
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

    // The descendant never exits: a supervisor that waited for it would end
    // only at the 20 s timeout. Half of it leaves a loaded host room.
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(
        outcome.resolution(),
        MakerActorSupervisorResolution::Terminal
    );
    let record = store.list_maker_actor_processes().unwrap().remove(0);
    assert_eq!(record.schedule_state(), MakerActorScheduleState::Terminal);
    assert_eq!(record.child_identity(), None);
    assert_eq!(
        store
            .maker_actor_progress(&SwapId::new(swap_id).unwrap())
            .unwrap()
            .unwrap()
            .observation(),
        &MakerActorProgressObservationV1::active("completed", 4, "complete").unwrap()
    );
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
    let program: &[u8] = &actor_script_bytes(program);
    let mut store = registered_store(root.path(), swap_id, program);
    let config = MakerActorSupervisorConfig::new(Duration::from_secs(20), 5, 30, 8_192)
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
        printf '%0300d' 0\n";
    let mut store = registered_store(root.path(), swap_id, program);
    // The 300 bytes go out in one write, so the cap, not the timeout, ends
    // the actor however loaded the host is; the timeout only bounds a hang.
    let config = MakerActorSupervisorConfig::new(Duration::from_secs(20), 5, 30, 256)
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
          activate) printf '%s\\n' '{\"schema_version\":1,\"role\":\"maker\",\"command\":\"activate\",\"outcome\":\"future_untrusted_outcome\",\"phase\":\"offered\",\"revision\":0,\"next_action\":\"create_and_fund_lez\"}' ;;\n\
          *) exit 93 ;;\n\
        esac\n";
    let mut store = registered_store(root.path(), swap_id, program);
    let config = MakerActorSupervisorConfig::new(Duration::from_secs(20), 5, 30, 8_192)
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
    assert_eq!(
        store
            .maker_actor_progress(&SwapId::new(swap_id).unwrap())
            .unwrap()
            .unwrap()
            .observation(),
        &MakerActorProgressObservationV1::NotActivated
    );
}

#[test]
fn regressing_effect_revision_fails_and_preserves_valid_status_progress() {
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let swap_id = "m5-supervisor-regressing-effect";
    let program = b"#!/bin/sh\n\
        test \"$1\" = \"--config-fd\" || exit 91\n\
        test \"$2\" = \"196\" || exit 92\n\
        case \"$3\" in\n\
          status) printf '%s\\n' '{\"schema_version\":1,\"role\":\"maker\",\"state\":\"active\",\"phase\":\"claim_evidence_available\",\"revision\":3,\"next_action\":\"claim_lez\"}' ;;\n\
          claim) printf '%s\\n' '{\"schema_version\":1,\"role\":\"maker\",\"command\":\"claim\",\"outcome\":\"submitted\",\"phase\":\"claim_evidence_available\",\"revision\":2,\"next_action\":\"claim_lez\"}' ;;\n\
          *) exit 93 ;;\n\
        esac\n";
    let mut store = registered_store(root.path(), swap_id, program);
    let config = MakerActorSupervisorConfig::new(Duration::from_secs(20), 5, 30, 8_192)
        .expect("bounded regressing-output config");

    let outcome = supervise_one_due_maker_actor(
        &mut store,
        MakerActorLeaseOwner::new([0x58; 16]).unwrap(),
        10,
        &config,
    )
    .expect("regressing-output cycle")
    .expect("one due actor");

    assert_eq!(outcome.resolution(), MakerActorSupervisorResolution::Failed);
    let record = store.list_maker_actor_processes().unwrap().remove(0);
    assert_eq!(record.schedule_state(), MakerActorScheduleState::Failed);
    assert_eq!(
        store
            .maker_actor_progress(&SwapId::new(swap_id).unwrap())
            .unwrap()
            .unwrap()
            .observation(),
        &MakerActorProgressObservationV1::active("claim_evidence_available", 3, "claim_lez")
            .unwrap()
    );
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
    let config = MakerActorSupervisorConfig::new(Duration::from_secs(20), 5, 30, 8_192)
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
    assert_eq!(
        store
            .maker_actor_progress(&SwapId::new(swap_id).unwrap())
            .unwrap()
            .unwrap()
            .observation(),
        &MakerActorProgressObservationV1::active("completed", 4, "complete").unwrap()
    );
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

/// Test actor scripts probe their inherited descriptors through the platform's
/// descriptor directory (`/proc/self/fd` on Linux, `/dev/fd` on macOS) and
/// call `sleep` where the platform keeps it.
fn actor_script(script: String) -> String {
    if cfg!(target_os = "macos") {
        script
            .replace("/proc/self/fd/", "/dev/fd/")
            .replace("/usr/bin/sleep", "/bin/sleep")
    } else {
        script
    }
}

/// [`actor_script`] for a script given as bytes.
fn actor_script_bytes(script: &[u8]) -> Vec<u8> {
    actor_script(String::from_utf8(script.to_vec()).expect("UTF-8 actor script")).into_bytes()
}

#[test]
fn a_submitted_maker_lez_claim_is_followed_while_an_earlier_refund_waits() {
    // Found on the devnet (zec-concurrent selling LEZ, 2026-11-18): the swap
    // with the later LEZ refund submitted its claim first; the swap with the
    // earlier refund then announced itself as waiting. The submitted swap's
    // next attempt (to see its own claim final and release the nonce) was held
    // behind the waiting one, which was held behind the submitted claim: no
    // claim moved until the earlier refund time had passed. A swap whose own
    // claim is in flight is never held; the waiting one goes after it.
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut store = SqliteSwapStore::open(root.path().join("order-maker.sqlite3")).unwrap();
    let claims = root.path().join("lez-claims-submitted");
    let now_ms = u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap();
    let swaps = [
        ("m5-held-a-later-refund", now_ms + 3_600_000),
        ("m5-held-b-earlier-refund", now_ms + 1_800_000),
    ];
    for (swap_id, refund_at_ms) in swaps {
        let swap_root = root.path().join(swap_id);
        fs::create_dir(&swap_root).unwrap();
        fs::set_permissions(&swap_root, fs::Permissions::from_mode(0o700)).unwrap();
        let state = ActorConfig::load_private(&actor_deployment(&swap_root, swap_id).source_config)
            .unwrap()
            .role_state_db()
            .to_path_buf();
        let program = gated_lez_claim_program(
            swap_id,
            &state,
            &root.path().join(format!("{swap_id}-log")),
            &claims,
            refund_at_ms,
        );
        register_actor(&mut store, &swap_root, swap_id, program.as_bytes());
    }
    let config = MakerActorSupervisorConfig::new(Duration::from_secs(20), 5, 30, 8_192).unwrap();
    let owner = MakerActorLeaseOwner::new([0x78; 16]).unwrap();
    let run = |store: &mut SqliteSwapStore, now: u64| {
        let outcome = supervise_one_due_maker_actor(store, owner, now, &config)
            .unwrap()
            .expect("a due actor runs");
        (outcome.swap_id().as_str().to_owned(), outcome.resolution())
    };
    // `a` (later refund) is due first, nothing waits yet: it submits.
    assert_eq!(
        run(&mut store, 10),
        (
            "m5-held-a-later-refund".to_owned(),
            MakerActorSupervisorResolution::Requeued
        )
    );
    // `b` (earlier refund) announces itself and waits behind `a`'s nonce.
    let (swap, resolution) = run(&mut store, 15);
    assert_eq!(swap, "m5-held-b-earlier-refund");
    assert_eq!(resolution, MakerActorSupervisorResolution::Backoff);
    // `a` follows its own submitted claim to final although `b` refunds earlier.
    assert_eq!(
        run(&mut store, 20),
        (
            "m5-held-a-later-refund".to_owned(),
            MakerActorSupervisorResolution::Terminal
        )
    );
    // Then `b` claims and completes.
    let (swap, resolution) = run(&mut store, 60);
    assert_eq!(swap, "m5-held-b-earlier-refund");
    assert_eq!(resolution, MakerActorSupervisorResolution::Requeued);
    let (swap, resolution) = run(&mut store, 65);
    assert_eq!(swap, "m5-held-b-earlier-refund");
    assert_eq!(resolution, MakerActorSupervisorResolution::Terminal);
}

fn unix_now_ms() -> u64 {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

/// Registers a Zcash Maker actor `swap_id` under `root` running `program`
/// (given the swap's role state database) and returns that database.
fn register_gated_actor(
    store: &mut SqliteSwapStore,
    root: &Path,
    swap_id: &str,
    program: impl FnOnce(&Path) -> String,
) -> std::path::PathBuf {
    let swap_root = root.join(swap_id);
    fs::create_dir(&swap_root).unwrap();
    fs::set_permissions(&swap_root, fs::Permissions::from_mode(0o700)).unwrap();
    let state = ActorConfig::load_private(&actor_deployment(&swap_root, swap_id).source_config)
        .unwrap()
        .role_state_db()
        .to_path_buf();
    register_actor(store, &swap_root, swap_id, program(&state).as_bytes());
    state
}

fn run_due(
    store: &mut SqliteSwapStore,
    owner: MakerActorLeaseOwner,
    now: u64,
) -> (String, MakerActorSupervisorResolution, Option<String>) {
    // Long enough that a stub actor on a loaded runner never hits it (the gate
    // tests flaked at 2 s under parallel load).
    run_due_with_timeout(store, owner, now, Duration::from_secs(20))
}

fn run_due_with_timeout(
    store: &mut SqliteSwapStore,
    owner: MakerActorLeaseOwner,
    now: u64,
    attempt_timeout: Duration,
) -> (String, MakerActorSupervisorResolution, Option<String>) {
    let config = MakerActorSupervisorConfig::new(attempt_timeout, 5, 30, 8_192).unwrap();
    let outcome = supervise_one_due_maker_actor(store, owner, now, &config)
        .unwrap()
        .expect("a due actor runs");
    let class = store
        .list_maker_actor_processes()
        .unwrap()
        .into_iter()
        .find(|record| record.swap_id() == outcome.swap_id())
        .unwrap()
        .last_failure_class()
        .map(str::to_owned);
    (
        outcome.swap_id().as_str().to_owned(),
        outcome.resolution(),
        class,
    )
}

#[test]
fn a_maker_lez_claim_in_flight_past_its_refund_time_holds_no_other_claim() {
    // Review item 2: a Maker LEZ claim that never lands (its escrow refunded,
    // or its refund time passed) kept its in-flight marker while its swap
    // stayed at claim_evidence_available, and every later follow-up LEZ claim
    // waited behind it forever. Past the holder's LEZ refund time its claim
    // can no longer be included (the escrow claim is valid only before it),
    // so it takes no nonce and holds nothing. A holder whose refund time is
    // ahead still holds.
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let claims = root.path().join("lez-claims-submitted");
    let now_ms = unix_now_ms();
    let owner = MakerActorLeaseOwner::new([0x79; 16]).unwrap();

    for (holder, holder_refund_at_ms, expected_resolution, expected_class) in [
        (
            "m5-expired-holder-past",
            now_ms - 1_000,
            MakerActorSupervisorResolution::Requeued,
            None,
        ),
        (
            "m5-expired-holder-ahead",
            now_ms + 3_600_000,
            MakerActorSupervisorResolution::Backoff,
            Some("lez_revealing_claim_in_flight"),
        ),
    ] {
        let case = root.path().join(holder);
        fs::create_dir(&case).unwrap();
        fs::set_permissions(&case, fs::Permissions::from_mode(0o700)).unwrap();
        let mut store = SqliteSwapStore::open(case.join("maker.sqlite3")).unwrap();
        let holder_state = register_gated_actor(&mut store, &case, holder, |_| {
            claim_logging_program(
                &case.join("holder-log"),
                "claim_evidence_available",
                "claim_lez",
            )
        });
        fs::write(
            zec_reference_actor::revealing_claim_in_flight_marker(&holder_state).unwrap(),
            format!("follow-up LEZ claim submitted\nlez_refund_at_ms={holder_refund_at_ms}\n"),
        )
        .unwrap();
        // The holder is mid-attempt elsewhere: leased, not terminal, not due here.
        store
            .claim_maker_actor(
                &SwapId::new(holder).unwrap(),
                MakerActorLeaseOwner::new([0x7c; 16]).unwrap(),
                10,
            )
            .unwrap()
            .expect("holder lease");
        let follower = format!("{holder}-follower");
        register_gated_actor(&mut store, &case, &follower, |state| {
            gated_lez_claim_program(
                &follower,
                state,
                &case.join("follower-log"),
                &claims,
                now_ms + 1_800_000,
            )
        });
        let (swap, resolution, class) = run_due(&mut store, owner, 10);
        assert_eq!(swap, follower);
        assert_eq!(resolution, expected_resolution, "{holder}");
        assert_eq!(class.as_deref(), expected_class, "{holder}");
    }
    assert_eq!(
        fs::read_to_string(&claims).unwrap(),
        "m5-expired-holder-past-follower\n",
        "only the claim behind an expired holder is submitted"
    );
}

#[test]
fn a_maker_lez_claim_that_cannot_land_stops_by_name_and_frees_the_gate() {
    // Review item 2: the Maker had no exit from claim_evidence_available once
    // its LEZ claim could no longer land (the Taker's LEZ refund on the chain,
    // or the refund time passed with no claim in a block). The actor now
    // refuses that claim by name (exit 8) after removing its in-flight
    // marker; the supervisor stops the swap with the failure class
    // lez_claim_cannot_land, and the next claim goes ahead.
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut store = SqliteSwapStore::open(root.path().join("cannot-land.sqlite3")).unwrap();
    let claims = root.path().join("lez-claims-submitted");
    let now_ms = unix_now_ms();
    let owner = MakerActorLeaseOwner::new([0x7a; 16]).unwrap();

    let stuck = "m5-cannot-land-a-stuck";
    let stuck_log = root.path().join("stuck-log");
    let stuck_state = register_gated_actor(&mut store, root.path(), stuck, |state| {
        let in_flight = zec_reference_actor::revealing_claim_in_flight_marker(state).unwrap();
        format!(
            "#!/bin/sh\n\
             test \"$1\" = \"--config-fd\" || exit 91\n\
             test \"$2\" = \"196\" || exit 92\n\
             printf '%s\\n' \"$3\" >> '{log}'\n\
             case \"$3\" in\n\
               status) printf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"state\":\"active\",\"phase\":\"claim_evidence_available\",\"revision\":4,\"next_action\":\"claim_lez\",\"lez_refund_at_ms\":{refund},\"revealing_claim_hold_attention_ms\":60000}}' ;;\n\
               claim) rm -f '{in_flight}'; exit 8 ;;\n\
               *) exit 95 ;;\n\
             esac\n",
            log = stuck_log.display(),
            refund = now_ms - 120_000,
            in_flight = in_flight.display(),
        )
    });
    // Its claim was submitted before the refund time, by an actor that did
    // not record the time in the marker.
    fs::write(
        zec_reference_actor::revealing_claim_in_flight_marker(&stuck_state).unwrap(),
        b"follow-up LEZ claim submitted\n",
    )
    .unwrap();
    let next = "m5-cannot-land-b-next";
    register_gated_actor(&mut store, root.path(), next, |state| {
        gated_lez_claim_program(
            next,
            state,
            &root.path().join("next-log"),
            &claims,
            now_ms + 1_800_000,
        )
    });

    let (swap, resolution, class) = run_due(&mut store, owner, 10);
    assert_eq!(swap, stuck);
    assert_eq!(resolution, MakerActorSupervisorResolution::Failed);
    assert_eq!(class.as_deref(), Some("lez_claim_cannot_land"));
    let record = store
        .list_maker_actor_processes()
        .unwrap()
        .into_iter()
        .find(|record| record.swap_id().as_str() == stuck)
        .unwrap();
    assert_eq!(record.schedule_state(), MakerActorScheduleState::Failed);

    let (swap, resolution, _) = run_due(&mut store, owner, 10);
    assert_eq!(swap, next);
    assert_eq!(resolution, MakerActorSupervisorResolution::Requeued);
    assert_eq!(fs::read_to_string(&claims).unwrap(), format!("{next}\n"));
    assert_eq!(fs::read_to_string(&stuck_log).unwrap(), "status\nclaim\n");
}

#[test]
fn a_waiting_maker_lez_claim_whose_attempt_failed_holds_no_other_claim() {
    // Review item 6: with equal LEZ refund times the swap id breaks the tie.
    // The first swap's claim failed before it was submitted (its attempt
    // backed off for its own reason), yet its waiting marker kept ordering
    // the second behind it until its refund time. A swap whose last attempt
    // failed is not waiting at the gate and orders nothing; it is ordered
    // again when its next attempt reaches the gate.
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut store = SqliteSwapStore::open(root.path().join("tie-maker.sqlite3")).unwrap();
    let claims = root.path().join("lez-claims-submitted");
    let refund_at_ms = unix_now_ms() + 1_800_000;
    let owner = MakerActorLeaseOwner::new([0x7b; 16]).unwrap();

    let failing = "m5-tie-a-failing";
    let failing_log = root.path().join("failing-log");
    register_gated_actor(&mut store, root.path(), failing, |_| {
        format!(
            "#!/bin/sh\n\
             test \"$1\" = \"--config-fd\" || exit 91\n\
             test \"$2\" = \"196\" || exit 92\n\
             printf '%s\\n' \"$3\" >> '{log}'\n\
             case \"$3\" in\n\
               status) printf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"state\":\"active\",\"phase\":\"claim_evidence_available\",\"revision\":4,\"next_action\":\"claim_lez\",\"lez_refund_at_ms\":{refund_at_ms},\"revealing_claim_hold_attention_ms\":60000}}' ;;\n\
               claim) exit 2 ;;\n\
               *) exit 95 ;;\n\
             esac\n",
            log = failing_log.display(),
        )
    });
    let other = "m5-tie-b-other";
    register_gated_actor(&mut store, root.path(), other, |state| {
        gated_lez_claim_program(
            other,
            state,
            &root.path().join("other-log"),
            &claims,
            refund_at_ms,
        )
    });

    // `a` wins the tie, is admitted and fails before submitting.
    let (swap, resolution, class) = run_due(&mut store, owner, 10);
    assert_eq!(swap, failing);
    assert_eq!(resolution, MakerActorSupervisorResolution::Backoff);
    assert_eq!(class.as_deref(), Some("actor_exit_failed"));
    // `b` is not held behind the failed one.
    let (swap, resolution, class) = run_due(&mut store, owner, 10);
    assert_eq!(swap, other);
    assert_eq!(
        (resolution, class.as_deref()),
        (MakerActorSupervisorResolution::Requeued, None)
    );
    assert_eq!(fs::read_to_string(&claims).unwrap(), format!("{other}\n"));
    // `b` follows its claim to final.
    let (swap, resolution, _) = run_due(&mut store, owner, 40);
    assert_eq!(swap, other);
    assert_eq!(resolution, MakerActorSupervisorResolution::Terminal);
    assert_eq!(fs::read_to_string(&failing_log).unwrap(), "status\nclaim\n");
}

#[test]
fn a_failed_swaps_lez_claim_in_flight_holds_no_other_claim() {
    // Review 3, item 3: the gate's peers excluded only terminal swaps, and a
    // swap stopped as failed (its re-plans exhausted) kept its in-flight
    // marker, so it held every other Maker LEZ claim until its own LEZ refund
    // time; an old-format marker without a time held them forever. A failed
    // swap's claim holds nothing, and the actor removes its marker on that
    // failure.
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let claims = root.path().join("lez-claims-submitted");
    let now_ms = unix_now_ms();
    let owner = MakerActorLeaseOwner::new([0x7a; 16]).unwrap();
    let mut store = SqliteSwapStore::open(root.path().join("maker.sqlite3")).unwrap();
    let holder = "m5-failed-holder";
    let holder_state = register_gated_actor(&mut store, root.path(), holder, |_| {
        format!(
            "#!/bin/sh\n\
             test \"$1\" = \"--config-fd\" || exit 91\n\
             case \"$3\" in\n\
               status) printf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"state\":\"active\",\"phase\":\"claim_evidence_available\",\"revision\":4,\"next_action\":\"claim_lez\",\"lez_refund_at_ms\":{at},\"revealing_claim_hold_attention_ms\":60000}}' ;;\n\
               claim) printf '%s\\n' 'not an effect' ;;\n\
               *) exit 95 ;;\n\
             esac\n",
            at = now_ms + 3_600_000,
        )
    });
    // An old-format marker (no refund time) left by a claim that never landed.
    fs::write(
        zec_reference_actor::revealing_claim_in_flight_marker(&holder_state).unwrap(),
        "follow-up LEZ claim submitted\n",
    )
    .unwrap();
    let (swap, resolution, class) = run_due(&mut store, owner, 10);
    assert_eq!(swap, holder);
    assert_eq!(resolution, MakerActorSupervisorResolution::Failed);
    assert_eq!(class.as_deref(), Some("actor_output_invalid"));

    let follower = "m5-failed-holder-follower";
    register_gated_actor(&mut store, root.path(), follower, |state| {
        gated_lez_claim_program(
            follower,
            state,
            &root.path().join("follower-log"),
            &claims,
            now_ms + 1_800_000,
        )
    });
    let (swap, resolution, class) = run_due(&mut store, owner, 20);
    assert_eq!(swap, follower);
    assert_eq!(
        (resolution, class.as_deref()),
        (MakerActorSupervisorResolution::Requeued, None),
        "a failed swap's claim holds no other"
    );
}

/// A swap whose claim was submitted (its timed in-flight marker written)
/// and whose attempt then failed (`actor_output_invalid`) at scheduler time
/// `failed_at` (Unix seconds), then the next swap's claim at the gate.
fn failed_after_submit_then_next(
    root: &Path,
    store: &mut SqliteSwapStore,
    names: (&str, &str),
    failed_at: u64,
) -> (MakerActorSupervisorResolution, Option<String>) {
    let now_ms = unix_now_ms();
    let (holder, follower) = names;
    let holder_refund_at = now_ms + 3_600_000;
    let holder_state = register_gated_actor(store, root, holder, |_| {
        format!(
            "#!/bin/sh\n\
             test \"$1\" = \"--config-fd\" || exit 91\n\
             case \"$3\" in\n\
               status) printf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"state\":\"active\",\"phase\":\"claim_evidence_available\",\"revision\":4,\"next_action\":\"claim_lez\",\"lez_refund_at_ms\":{holder_refund_at},\"revealing_claim_hold_attention_ms\":60000}}' ;;\n\
               claim) printf '%s\\n' 'not an effect' ;;\n\
               *) exit 95 ;;\n\
             esac\n",
        )
    });
    fs::write(
        zec_reference_actor::revealing_claim_in_flight_marker(&holder_state).unwrap(),
        format!("follow-up LEZ claim submitted\nlez_refund_at_ms={holder_refund_at}\n"),
    )
    .unwrap();
    let (swap, resolution, class) = run_due(
        store,
        MakerActorLeaseOwner::new([0x7c; 16]).unwrap(),
        failed_at,
    );
    assert_eq!(swap, holder);
    assert_eq!(resolution, MakerActorSupervisorResolution::Failed);
    assert_eq!(class.as_deref(), Some("actor_output_invalid"));
    let claims = root.join("lez-claims-submitted");
    register_gated_actor(store, root, follower, |state| {
        gated_lez_claim_program(
            follower,
            state,
            &root.join("follower-log"),
            &claims,
            now_ms + 1_800_000,
        )
    });
    let (swap, resolution, class) = run_due(
        store,
        MakerActorLeaseOwner::new([0x7c; 16]).unwrap(),
        unix_now_ms() / 1000 + 1,
    );
    assert_eq!(swap, follower);
    (resolution, class)
}

#[test]
fn a_failed_swaps_claim_in_flight_holds_the_next_for_one_admission_bound_after_its_failure() {
    // Review 5, item 1: after review 4 (L3) a failed swap's timed in-flight
    // marker held every other claim until that swap's refund time. A failed
    // swap is driven no more, so nothing clears its marker once its claim
    // lands: the next claim (with an earlier refund time) never went out. A
    // failed swap's marker now holds for one admission bound after the
    // failure, long enough for a claim it left pending to be included; a claim
    // still pending after that costs the next claim one re-plan, never funds.
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut store = SqliteSwapStore::open(root.path().join("maker.sqlite3")).unwrap();
    // Failed long ago (scheduler time 10 s): it holds nothing.
    assert_eq!(
        failed_after_submit_then_next(
            root.path(),
            &mut store,
            ("m5-failed-long-ago", "m5-failed-long-ago-next"),
            10,
        ),
        (MakerActorSupervisorResolution::Requeued, None),
        "a failed swap's claim holds the next one no longer than one admission bound"
    );
}

#[test]
fn a_swap_that_just_failed_after_submitting_holds_the_next_claim() {
    // Review 4, L3: within the bound the failed swap's pending claim still
    // holds the nonce.
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut store = SqliteSwapStore::open(root.path().join("maker.sqlite3")).unwrap();
    assert_eq!(
        failed_after_submit_then_next(
            root.path(),
            &mut store,
            ("m5-just-failed", "m5-just-failed-next"),
            unix_now_ms() / 1000,
        ),
        (
            MakerActorSupervisorResolution::Backoff,
            Some("lez_revealing_claim_in_flight".to_owned())
        ),
    );
}

/// One swap whose claim was admitted `admitted_ago_ms` ago by another worker
/// (or a Node that crashed since), and the next swap's claim at the gate.
fn admitted_then_next(
    root: &Path,
    store: &mut SqliteSwapStore,
    names: (&str, &str),
    admitted_ago_ms: u64,
) -> PathBuf {
    let now_ms = unix_now_ms();
    let (admitted, next) = names;
    let admitted_state = register_gated_actor(store, root, admitted, |_| {
        claim_logging_program(
            &root.join("admitted-log"),
            "claim_evidence_available",
            "claim_lez",
        )
    });
    fs::write(
        zec_reference_actor::lez_claim_admitted_marker(&admitted_state).unwrap(),
        format!(
            "lez_refund_at_ms={}\nadmitted_at_ms={}\n",
            now_ms + 3_600_000,
            now_ms - admitted_ago_ms
        ),
    )
    .unwrap();
    store
        .claim_maker_actor(
            &SwapId::new(admitted).unwrap(),
            MakerActorLeaseOwner::new([0x7d; 16]).unwrap(),
            10,
        )
        .unwrap()
        .expect("admitted swap's lease");
    let claims = root.join("lez-claims-submitted");
    register_gated_actor(store, root, next, |state| {
        gated_lez_claim_program(
            next,
            state,
            &root.join("next-log"),
            &claims,
            now_ms + 1_800_000,
        )
    });
    admitted_state
}

#[test]
fn a_stale_lez_claim_admission_holds_no_longer_than_one_claim_attempt() {
    // Review 4, L2: an admission marker held every other claim for 300 s
    // whatever the attempt's own bound (2 s here); after a Node crash that
    // is more than the devnet profiles' whole margin (120 s). It now holds for
    // at most the attempt's bound (and half the profile's margin) plus the
    // time to stop the child.
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let owner = MakerActorLeaseOwner::new([0x7e; 16]).unwrap();
    let mut store = SqliteSwapStore::open(root.path().join("maker.sqlite3")).unwrap();
    admitted_then_next(
        root.path(),
        &mut store,
        ("m5-stale-admission", "m5-stale-admission-next"),
        60_000,
    );
    let (swap, resolution, class) = run_due(&mut store, owner, 10);
    assert_eq!(swap, "m5-stale-admission-next");
    assert_eq!(
        (resolution, class.as_deref()),
        (MakerActorSupervisorResolution::Requeued, None),
        "an admission older than one attempt holds nothing"
    );
}

#[test]
fn a_starting_supervisor_clears_every_lez_claim_admission() {
    // Review 4, L2: no claim attempt runs when the supervisor starts, so an
    // admission marker then is a crashed Node's and holds nothing.
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut store = SqliteSwapStore::open(root.path().join("maker.sqlite3")).unwrap();
    let admitted_state = admitted_then_next(
        root.path(),
        &mut store,
        ("m5-crashed-admission", "m5-crashed-admission-next"),
        0,
    );
    clear_maker_lez_claim_admissions(&store);
    assert!(
        !zec_reference_actor::lez_claim_admitted_marker(&admitted_state)
            .unwrap()
            .exists(),
        "the crashed Node's admission is gone"
    );
}

/// A Maker LEZ claim whose `claim` takes `claim_seconds` and then reports
/// `submitted`, under a profile margin of `margin_ms` (its admission bound is
/// half of it).
fn slow_lez_claim_program(margin_ms: u64, claim_seconds: &str) -> String {
    let at = unix_now_ms() + 3_600_000;
    format!(
        "#!/bin/sh\n\
         test \"$1\" = \"--config-fd\" || exit 91\n\
         case \"$3\" in\n\
           status) printf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"state\":\"active\",\"phase\":\"claim_evidence_available\",\"revision\":4,\"next_action\":\"claim_lez\",\"lez_refund_at_ms\":{at},\"revealing_claim_hold_attention_ms\":60000,\"required_margin_ms\":{margin_ms}}}' ;;\n\
           claim) sleep {claim_seconds}; printf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"command\":\"claim\",\"outcome\":\"submitted\",\"phase\":\"claim_evidence_available\",\"revision\":5,\"next_action\":\"claim_lez\"}}' ;;\n\
           *) exit 95 ;;\n\
         esac\n"
    )
}

#[test]
fn an_admitted_lez_claim_nobody_waits_behind_runs_past_its_admission_bound() {
    // Review 5, item 3: the admission bound (half the margin) stopped an
    // admitted claim attempt even with no other swap at the gate, and a stop
    // between the bridge submit and the in-flight marker leaves a pending
    // claim unmarked. The bound applies only while another swap waits.
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut store = SqliteSwapStore::open(root.path().join("maker.sqlite3")).unwrap();
    // Bound 0.5 s (margin 1 s), the claim takes 1.2 s, the attempt timeout 20 s.
    register_gated_actor(&mut store, root.path(), "m5-slow-admitted", |_| {
        slow_lez_claim_program(1_000, "1.2")
    });
    let (_, resolution, class) = run_due(
        &mut store,
        MakerActorLeaseOwner::new([0x81; 16]).unwrap(),
        10,
    );
    assert_eq!(
        (resolution, class.as_deref()),
        (MakerActorSupervisorResolution::Requeued, None),
        "nobody waits: the attempt runs to its own timeout"
    );
}

#[test]
fn a_timed_out_admitted_lez_claim_keeps_its_admission_for_its_hold() {
    // Review 5, item 3: an admitted attempt stopped at its timeout may have
    // sent its claim without writing the in-flight marker; its admission now
    // stays for its recorded hold instead of being released at once.
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut store = SqliteSwapStore::open(root.path().join("maker.sqlite3")).unwrap();
    let state = register_gated_actor(&mut store, root.path(), "m5-timed-out-admitted", |_| {
        slow_lez_claim_program(600_000, "12")
    });
    let (_, resolution, class) = run_due_with_timeout(
        &mut store,
        MakerActorLeaseOwner::new([0x82; 16]).unwrap(),
        10,
        Duration::from_secs(6),
    );
    assert_eq!(
        (resolution, class.as_deref()),
        (
            MakerActorSupervisorResolution::Backoff,
            Some("actor_timeout")
        )
    );
    let admission = zec_reference_actor::lez_claim_admission(&state).expect("admission kept");
    assert!(
        admission.admitted_at_ms + admission.hold_ms.expect("hold recorded") > unix_now_ms(),
        "the admission holds after the stop"
    );
}

#[test]
fn an_admission_marker_holds_for_its_own_recorded_hold() {
    // Review 5, item 5: a marker's own hold_ms, not the reader's bound.
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut store = SqliteSwapStore::open(root.path().join("maker.sqlite3")).unwrap();
    let state = admitted_then_next(
        root.path(),
        &mut store,
        ("m5-own-hold", "m5-own-hold-next"),
        60_000,
    );
    let mut admission = zec_reference_actor::lez_claim_admission(&state).unwrap();
    admission.hold_ms = Some(600_000);
    zec_reference_actor::record_lez_claim_admitted(&state, Some(admission));
    let (swap, resolution, class) = run_due(
        &mut store,
        MakerActorLeaseOwner::new([0x83; 16]).unwrap(),
        10,
    );
    assert_eq!(swap, "m5-own-hold-next");
    assert_eq!(
        (resolution, class.as_deref()),
        (
            MakerActorSupervisorResolution::Backoff,
            Some("lez_revealing_claim_in_flight")
        ),
        "admitted a minute ago with a ten-minute hold: it still holds"
    );
}

#[test]
fn a_superseded_lez_claim_past_the_old_replan_limit_is_retried_not_failed() {
    // Issue #172: the actor's re-plan exhaustion was turned into Fail and the
    // scheduler never picked the swap up again, while its claim could still
    // land. It is a backoff now: the next attempt re-plans.
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut store = SqliteSwapStore::open(root.path().join("maker.sqlite3")).unwrap();
    let at = unix_now_ms() + 3_600_000;
    register_gated_actor(&mut store, root.path(), "m5-replans-past-old-limit", |_| {
        format!(
            "#!/bin/sh\n\
             test \"$1\" = \"--config-fd\" || exit 91\n\
             case \"$3\" in\n\
               status) printf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"state\":\"active\",\"phase\":\"claim_evidence_available\",\"revision\":4,\"next_action\":\"claim_lez\",\"lez_refund_at_ms\":{at},\"revealing_claim_hold_attention_ms\":60000}}' ;;\n\
               claim) exit {code} ;;\n\
               *) exit 95 ;;\n\
             esac\n",
            code = zec_reference_actor::LEZ_REVEALING_CLAIM_REPLAN_EXHAUSTED_EXIT_CODE,
        )
    });
    let (_, resolution, class) = run_due(
        &mut store,
        MakerActorLeaseOwner::new([0x84; 16]).unwrap(),
        10,
    );
    assert_eq!(
        (resolution, class.as_deref()),
        (
            MakerActorSupervisorResolution::Backoff,
            Some("lez_revealing_claim_replan_exhausted")
        )
    );
}

/// The Maker's Zcash refund after its LEZ claim was lost, as the actor's
/// claim command reports it (`lost_claim_refund_output`): outcome
/// `refunded`, phase `refunded`, next action `complete`.
fn lost_claim_refund_program(at: u64) -> String {
    format!(
        "#!/bin/sh\n\
         test \"$1\" = \"--config-fd\" || exit 91\n\
         case \"$3\" in\n\
           status) printf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"state\":\"active\",\"phase\":\"claim_evidence_available\",\"revision\":4,\"next_action\":\"claim_lez\",\"lez_refund_at_ms\":{at},\"revealing_claim_hold_attention_ms\":60000}}' ;;\n\
           claim) printf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"command\":\"claim\",\"outcome\":\"refunded\",\"phase\":\"refunded\",\"revision\":5,\"next_action\":\"complete\"}}' ;;\n\
           *) exit 95 ;;\n\
         esac\n"
    )
}

#[test]
fn a_maker_zcash_refund_after_a_lost_lez_claim_ends_the_swap_refunded() {
    // Issue #178, item 1: the claim command reported the Maker's Zcash refund
    // after a lost LEZ claim (the Taker's LEZ refund final, or the escrow open
    // past the refund time; one output for both) as `completed` with phase
    // `refunded`, which the supervisor's parser refused: the swap ended
    // failed (`actor_output_invalid`) instead of refunded.
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut store = SqliteSwapStore::open(root.path().join("maker.sqlite3")).unwrap();
    let id = "m5-lost-claim-refunded";
    register_gated_actor(&mut store, root.path(), id, |_| {
        lost_claim_refund_program(unix_now_ms() - 60_000)
    });
    let (swap, resolution, class) = run_due(
        &mut store,
        MakerActorLeaseOwner::new([0x85; 16]).unwrap(),
        10,
    );
    assert_eq!(swap, id);
    assert_eq!(
        (resolution, class.as_deref()),
        (MakerActorSupervisorResolution::Terminal, None)
    );
    let swap_id = SwapId::new(id).unwrap();
    assert_eq!(
        store
            .maker_actor_progress(&swap_id)
            .unwrap()
            .unwrap()
            .observation(),
        &MakerActorProgressObservationV1::active("refunded", 5, "complete").unwrap()
    );
}

#[test]
fn a_maker_lez_claim_admitted_elsewhere_holds_the_next_one() {
    // Review 3, item 4: the gate admitted a claim and its in-flight marker was
    // written only once the claim was submitted, so a second supervisor worker
    // could admit another claim in between and both were signed at one nonce
    // (one superseded, a re-plan burnt). An admitted claim now announces
    // itself (`lez-claim-admitted`, its refund time and admission time) and
    // holds others like a claim in flight, for a bounded time.
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let claims = root.path().join("lez-claims-submitted");
    let now_ms = unix_now_ms();
    let owner = MakerActorLeaseOwner::new([0x7b; 16]).unwrap();
    let mut store = SqliteSwapStore::open(root.path().join("maker.sqlite3")).unwrap();
    let admitted = "m5-admitted-elsewhere";
    let admitted_state = register_gated_actor(&mut store, root.path(), admitted, |_| {
        claim_logging_program(
            &root.path().join("admitted-log"),
            "claim_evidence_available",
            "claim_lez",
        )
    });
    fs::write(
        zec_reference_actor::lez_claim_waiting_marker(&admitted_state)
            .unwrap()
            .with_file_name("lez-claim-admitted"),
        format!(
            "lez_refund_at_ms={}\nadmitted_at_ms={now_ms}\n",
            now_ms + 3_600_000
        ),
    )
    .unwrap();
    // Its attempt runs on another worker: leased here.
    store
        .claim_maker_actor(
            &SwapId::new(admitted).unwrap(),
            MakerActorLeaseOwner::new([0x7d; 16]).unwrap(),
            10,
        )
        .unwrap()
        .expect("admitted swap's lease");
    let next = "m5-admitted-elsewhere-next";
    register_gated_actor(&mut store, root.path(), next, |state| {
        gated_lez_claim_program(
            next,
            state,
            &root.path().join("next-log"),
            &claims,
            now_ms + 1_800_000,
        )
    });
    let (swap, resolution, class) = run_due(&mut store, owner, 10);
    assert_eq!(swap, next);
    assert_eq!(resolution, MakerActorSupervisorResolution::Backoff);
    assert_eq!(class.as_deref(), Some("lez_revealing_claim_in_flight"));
    assert!(
        !claims.exists(),
        "nothing signed while another claim is admitted"
    );
}

/// A Zcash Maker actor script at `taker_lock_confirmed` whose status names
/// `leg` as the Maker's pending lock and whose drive sends `operation`
/// (`submitted`); each command is appended to `log`.
fn maker_lock_program(log: &Path, leg: &str, operation: &str) -> String {
    format!(
        "#!/bin/sh\n\
         test \"$1\" = \"--config-fd\" || exit 91\n\
         test \"$2\" = \"196\" || exit 92\n\
         printf '%s\\n' \"$3\" >> '{log}'\n\
         case \"$3\" in\n\
           status) printf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"state\":\"active\",\"phase\":\"taker_lock_confirmed\",\"revision\":1,\"next_action\":\"wait\",\"maker_lock\":\"{leg}\"}}' ;;\n\
           drive) printf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"command\":\"drive\",\"outcome\":\"submitted\",\"operation\":\"{operation}\",\"phase\":\"taker_lock_confirmed\",\"revision\":1,\"next_action\":\"wait\"}}' ;;\n\
           *) exit 95 ;;\n\
         esac\n",
        log = log.display()
    )
}

fn drives(log: &Path) -> usize {
    fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter(|command| *command == "drive")
        .count()
}

/// The text of a LEZ claim's in-flight marker as the actor writes it.
fn in_flight_marker(lez_refund_at_ms: u64, submitted_at_ms: u64) -> String {
    format!(
        "follow-up LEZ claim submitted\nlez_refund_at_ms={lez_refund_at_ms}\nsubmitted_at_ms={submitted_at_ms}\n"
    )
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one journey: lock behind lock, locks behind a claim, a claim past a lock"
)]
fn the_makers_lez_locks_wait_for_a_pending_lez_claim_and_for_each_other_and_no_claim_waits_for_a_lock()
 {
    // Issue #176: the Maker's LEZ locks (a Zcash swap the Taker sells Zcash
    // in: the escrow's initialization and funding) are signed by the account
    // that signs its follow-up LEZ claims, and went past the one-claim gate.
    // A lock planned while a claim was pending took the claim's nonce. Now
    // no lock is planned or sent while a claim waits, is admitted or is
    // submitted and within its hold; locks go one at a time; and a claim
    // never waits for a lock.
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut store = SqliteSwapStore::open(root.path().join("maker.sqlite3")).unwrap();
    let owner = MakerActorLeaseOwner::new([0x91; 16]).unwrap();
    let now = unix_now_ms() / 1000;
    let now_ms = unix_now_ms();

    // The first lock is admitted and sends its initialization.
    let first_log = root.path().join("first-lock-log");
    register_gated_actor(&mut store, root.path(), "m5-lock-first", |_| {
        maker_lock_program(&first_log, "lez", "lez_initialize")
    });
    assert_eq!(
        run_due(&mut store, owner, now),
        (
            "m5-lock-first".to_owned(),
            MakerActorSupervisorResolution::Requeued,
            None
        )
    );
    assert_eq!(drives(&first_log), 1);

    // A second swap's lock waits while the first is in flight, and is looked
    // at again after the requeue delay (5 s), not the failure backoff (30 s).
    let second_log = root.path().join("second-lock-log");
    register_gated_actor(&mut store, root.path(), "m5-lock-second", |_| {
        maker_lock_program(&second_log, "lez", "lez_initialize")
    });
    assert_eq!(
        run_due(&mut store, owner, now),
        (
            "m5-lock-second".to_owned(),
            MakerActorSupervisorResolution::Backoff,
            Some("lez_lock_waits_for_lock".to_owned())
        )
    );
    assert_eq!(drives(&second_log), 0, "nothing planned beside a lock");
    let due = |at| {
        store
            .list_due_maker_actor_ids(at, 8)
            .unwrap()
            .iter()
            .any(|id| id.as_str() == "m5-lock-second")
    };
    assert!(!due(now + 4) && due(now + 5));

    // A follow-up LEZ claim of a third swap is submitted (its attempt runs on
    // another worker: leased). Both locks wait for it, the one in flight too.
    let claim_state = register_gated_actor(&mut store, root.path(), "m5-lock-claim", |_| {
        claim_logging_program(
            &root.path().join("claim-log"),
            "claim_evidence_available",
            "claim_lez",
        )
    });
    let claim_marker = zec_reference_actor::revealing_claim_in_flight_marker(&claim_state).unwrap();
    fs::write(&claim_marker, in_flight_marker(now_ms + 3_600_000, now_ms)).unwrap();
    store
        .claim_maker_actor(
            &SwapId::new("m5-lock-claim").unwrap(),
            MakerActorLeaseOwner::new([0x92; 16]).unwrap(),
            now,
        )
        .unwrap()
        .expect("the claiming swap's lease");
    let mut held = Vec::new();
    for _ in 0..2 {
        let (swap, resolution, class) = run_due(&mut store, owner, now + 6);
        assert_eq!(
            resolution,
            MakerActorSupervisorResolution::Backoff,
            "{swap}"
        );
        assert_eq!(class.as_deref(), Some("lez_lock_waits_for_claim"), "{swap}");
        held.push(swap);
    }
    held.sort();
    assert_eq!(held, ["m5-lock-first", "m5-lock-second"]);
    assert_eq!((drives(&first_log), drives(&second_log)), (1, 0));

    // The claim was submitted longer ago than its hold: it holds no lock any
    // more, though it is not final and its refund time is an hour away. One
    // lock goes, the other waits behind it.
    fs::write(
        &claim_marker,
        in_flight_marker(now_ms + 3_600_000, now_ms - 600_000),
    )
    .unwrap();
    let mut after = Vec::new();
    for _ in 0..2 {
        let (swap, resolution, class) = run_due(&mut store, owner, now + 12);
        after.push((swap, resolution, class));
    }
    after.sort_by(|left, right| left.0.cmp(&right.0));
    assert_eq!(
        after,
        [
            (
                "m5-lock-first".to_owned(),
                MakerActorSupervisorResolution::Requeued,
                None
            ),
            (
                "m5-lock-second".to_owned(),
                MakerActorSupervisorResolution::Backoff,
                Some("lez_lock_waits_for_lock".to_owned())
            ),
        ]
    );
    assert_eq!((drives(&first_log), drives(&second_log)), (2, 0));

    // A claim arriving while that lock is in flight does not wait for it.
    let claims = root.path().join("lez-claims-submitted");
    let late_claim = "m5-lock-late-claim";
    register_gated_actor(&mut store, root.path(), late_claim, |state| {
        gated_lez_claim_program(
            late_claim,
            state,
            &root.path().join("late-claim-log"),
            &claims,
            now_ms + 1_800_000,
        )
    });
    assert_eq!(
        run_due(&mut store, owner, now + 12),
        (
            late_claim.to_owned(),
            MakerActorSupervisorResolution::Requeued,
            None
        )
    );
    assert_eq!(
        fs::read_to_string(&claims).unwrap(),
        format!("{late_claim}\n")
    );
}

#[test]
fn a_makers_zcash_funding_drive_passes_the_lez_account_gate() {
    // Selling LEZ the Maker's lock is its Zcash funding: no LEZ transaction,
    // so a pending LEZ claim does not hold it.
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut store = SqliteSwapStore::open(root.path().join("maker.sqlite3")).unwrap();
    let now = unix_now_ms() / 1000;
    let claim_state = register_gated_actor(&mut store, root.path(), "m5-zfund-claim", |_| {
        claim_logging_program(
            &root.path().join("claim-log"),
            "claim_evidence_available",
            "claim_lez",
        )
    });
    fs::write(
        zec_reference_actor::revealing_claim_in_flight_marker(&claim_state).unwrap(),
        in_flight_marker(unix_now_ms() + 3_600_000, unix_now_ms()),
    )
    .unwrap();
    store
        .claim_maker_actor(
            &SwapId::new("m5-zfund-claim").unwrap(),
            MakerActorLeaseOwner::new([0x94; 16]).unwrap(),
            now,
        )
        .unwrap()
        .expect("the claiming swap's lease");
    let log = root.path().join("funding-log");
    register_gated_actor(&mut store, root.path(), "m5-zfund", |_| {
        maker_lock_program(&log, "zcash", "zcash_fund")
    });
    assert_eq!(
        run_due(
            &mut store,
            MakerActorLeaseOwner::new([0x93; 16]).unwrap(),
            now
        ),
        (
            "m5-zfund".to_owned(),
            MakerActorSupervisorResolution::Requeued,
            None
        )
    );
    assert_eq!(drives(&log), 1);
}

#[test]
fn a_submitted_lez_claim_holds_the_next_claim_for_its_hold_not_until_it_is_final() {
    // Issue #176, related point: a claim submitted in a swap that is neither
    // failed nor ended held every other follow-up LEZ claim until its LEZ
    // refund time. It holds them for one admission bound from its
    // submission: time to be included, which is what takes the nonce.
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let now = unix_now_ms() / 1000;
    let now_ms = unix_now_ms();
    let next_after = |name: &str, submitted_ago_ms: u64| {
        let mut store = SqliteSwapStore::open(root.path().join(format!("{name}.sqlite3"))).unwrap();
        let holder = format!("{name}-holder");
        let holder_state = register_gated_actor(&mut store, root.path(), &holder, |_| {
            claim_logging_program(
                &root.path().join(format!("{name}-holder-log")),
                "claim_evidence_available",
                "claim_lez",
            )
        });
        fs::write(
            zec_reference_actor::revealing_claim_in_flight_marker(&holder_state).unwrap(),
            in_flight_marker(now_ms + 3_600_000, now_ms - submitted_ago_ms),
        )
        .unwrap();
        // Mid-attempt on another worker: neither failed nor ended.
        store
            .claim_maker_actor(
                &SwapId::new(holder).unwrap(),
                MakerActorLeaseOwner::new([0x96; 16]).unwrap(),
                now,
            )
            .unwrap()
            .expect("the holder's lease");
        let next = format!("{name}-next");
        let claims = root.path().join(format!("{name}-claims"));
        register_gated_actor(&mut store, root.path(), &next, |state| {
            gated_lez_claim_program(
                &next,
                state,
                &root.path().join(format!("{name}-next-log")),
                &claims,
                now_ms + 1_800_000,
            )
        });
        let (swap, resolution, class) = run_due(
            &mut store,
            MakerActorLeaseOwner::new([0x95; 16]).unwrap(),
            now,
        );
        assert_eq!(swap, next);
        (resolution, class)
    };
    assert_eq!(
        next_after("m5-hold-fresh", 1_000),
        (
            MakerActorSupervisorResolution::Backoff,
            Some("lez_revealing_claim_in_flight".to_owned())
        ),
        "inside its hold a submitted claim holds the next"
    );
    // The attempt bound here is 20 s and the stop grace 2 s.
    assert_eq!(
        next_after("m5-hold-old", 23_000),
        (MakerActorSupervisorResolution::Requeued, None),
        "past its hold it holds nothing, an hour before its refund time"
    );
}

/// Records the Maker's acceptance of `swap_id`'s agreement (the one its
/// deployment under `swap_root` was built with) at `accepted_at` in the
/// Node's store, as the Chat completion does.
fn accept_maker_agreement(database: &Path, swap_root: &Path, swap_id: &str, accepted_at: u64) {
    let config: Value = serde_json::from_slice(
        &fs::read(actor_deployment(swap_root, swap_id).source_config).unwrap(),
    )
    .unwrap();
    let wire = fs::read(config["signed_agreement_file"].as_str().unwrap()).unwrap();
    let accepted = lez_zec_swap_sdk::AcceptedZecAgreementV1::accept_wire_at(
        &wire,
        lez_swap_core::UnixSeconds::new(accepted_at),
        lez_swap_core::Participant::Maker,
        0,
    )
    .unwrap();
    assert_eq!(accepted.agreement().coordinator().id().as_str(), swap_id);
    let agreements =
        lez_swap_store::SqliteZecRecoveryStore::open(database, lez_swap_core::Participant::Maker)
            .unwrap();
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(lez_zec_swap_sdk::RecoveryStore::create_agreement(
            &agreements,
            &accepted.durable_envelope().unwrap(),
        ))
        .unwrap();
}

#[test]
fn a_taker_lock_confirmed_after_its_window_is_funded_only_once_a_place_is_free() {
    // Issue #178, item 2: an accepted swap whose Taker has not locked gives
    // its place up after the Taker-lock window (45 s under
    // deterministic-local-v3, capacity 1). Its Taker locks late, while the
    // place is another swap's: the Node does not run the funding drive, so no
    // Zcash of the Maker's is committed beyond the capacity; once the other
    // swap ends, the late lock is funded.
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let database = root.path().join("maker.sqlite3");
    let mut store = SqliteSwapStore::open(&database).unwrap();
    let owner = MakerActorLeaseOwner::new([0x97; 16]).unwrap();
    let now = unix_now_ms() / 1000;
    assert_eq!(
        lez_maker_node::zec_taker_sells_lez_capacity(
            lez_zec_swap_sdk::ZecProfileId::DeterministicLocalV3
        ),
        1
    );

    // The current swap: accepted now, its Taker's lock confirmed inside the
    // window. Its funding is admitted and it holds the one place.
    let current = "m5-fund-current";
    let current_log = root.path().join("current-log");
    register_gated_actor(&mut store, root.path(), current, |_| {
        maker_lock_program(&current_log, "zcash", "zcash_fund")
    });
    accept_maker_agreement(&database, &root.path().join(current), current, now);
    assert_eq!(
        run_due(&mut store, owner, now),
        (
            current.to_owned(),
            MakerActorSupervisorResolution::Requeued,
            None
        )
    );
    assert_eq!(drives(&current_log), 1);
    assert!(
        store
            .maker_zec_funding_admitted(&SwapId::new(current).unwrap())
            .unwrap()
    );

    // The late swap: accepted a minute ago, its Taker's lock confirmed only
    // now, past its window.
    let late = "m5-fund-late";
    let late_log = root.path().join("late-log");
    register_gated_actor(&mut store, root.path(), late, |_| {
        maker_lock_program(&late_log, "zcash", "zcash_fund")
    });
    accept_maker_agreement(&database, &root.path().join(late), late, now - 60);
    assert_eq!(
        run_due(&mut store, owner, now),
        (
            late.to_owned(),
            MakerActorSupervisorResolution::Backoff,
            Some("zec_funding_waits_for_free_place".to_owned())
        )
    );
    assert_eq!(drives(&late_log), 0, "no Zcash funded beyond the capacity");
    assert!(
        !store
            .maker_zec_funding_admitted(&SwapId::new(late).unwrap())
            .unwrap()
    );

    // The current swap ends: the place is free and the late lock is served.
    let lease = store
        .claim_maker_actor(&SwapId::new(current).unwrap(), owner, now + 6)
        .unwrap()
        .expect("the current swap is due again");
    store
        .resolve_maker_actor_attempt(&lease, MakerActorAttemptResolution::Terminal, now + 6)
        .unwrap();
    assert_eq!(
        run_due(&mut store, owner, now + 31),
        (
            late.to_owned(),
            MakerActorSupervisorResolution::Requeued,
            None
        )
    );
    assert_eq!(drives(&late_log), 1);
    assert!(
        store
            .maker_zec_funding_admitted(&SwapId::new(late).unwrap())
            .unwrap()
    );
}
