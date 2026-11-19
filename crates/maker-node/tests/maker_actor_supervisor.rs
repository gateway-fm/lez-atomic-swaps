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
    path::Path,
    process::{Child, Command, Output},
    thread,
    time::{Duration, Instant},
};

use btc_fixture::BtcAuthorityFixture;
use btc_reference_actor::ActorConfig as BtcActorConfig;
use lez_bridge_protocol::RequestId;
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
    ActorHeldLock, MakerActorKindV1, MakerActorLeaseOwner, MakerActorManifestV1,
    MakerActorManualAction, MakerActorManualActionState, MakerActorProgressObservationV1,
    MakerActorScheduleState, SqliteSwapStore, validate_maker_actor_program,
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

    let config = MakerActorSupervisorConfig::new(Duration::from_secs(2), 5, 30, 8_192).unwrap();
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
    let config = MakerActorSupervisorConfig::new(Duration::from_secs(2), 5, 30, 8_192).unwrap();

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
    let config = MakerActorSupervisorConfig::new(Duration::from_secs(2), 5, 30, 8_192).unwrap();

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
    let config = MakerActorSupervisorConfig::new(Duration::from_secs(2), 5, 30, 8_192).unwrap();

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
        let config = MakerActorSupervisorConfig::new(Duration::from_secs(2), 5, 30, 8_192).unwrap();
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
    let config = MakerActorSupervisorConfig::new(Duration::from_secs(2), 5, 30, 8_192).unwrap();
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
    let config = MakerActorSupervisorConfig::new(Duration::from_secs(2), 5, 30, 8_192).unwrap();

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
    let old_process_lock = ActorHeldLock::acquire(&locked_record).unwrap();
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
    let config = MakerActorSupervisorConfig::new(Duration::from_secs(2), 5, 30, 8_192)
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
           claim) printf started > '{}'; sleep 5; printf completed > '{}'; printf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"command\":\"claim\",\"outcome\":\"completed\",\"phase\":\"completed\",\"revision\":4,\"next_action\":\"complete\"}}' ;;\n\
           *) exit 93 ;;\n\
         esac\n",
        started_marker.display(),
        completed_marker.display()
    );
    let mut store = registered_store(root.path(), swap_id, program.as_bytes());
    let config = MakerActorSupervisorConfig::new(Duration::from_secs(5), 5, 30, 8_192)
        .unwrap()
        .with_effect_cutoff_boottime_milliseconds(boottime_milliseconds().saturating_add(1_000))
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

    assert!(started.elapsed() < Duration::from_secs(3));
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
          activate) printf '%s\\n' '{\"schema_version\":1,\"role\":\"maker\",\"command\":\"activate\",\"outcome\":\"future_untrusted_outcome\",\"phase\":\"offered\",\"revision\":0,\"next_action\":\"create_and_fund_lez\"}' ;;\n\
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
    let config = MakerActorSupervisorConfig::new(Duration::from_secs(2), 5, 30, 8_192)
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
    let config = MakerActorSupervisorConfig::new(Duration::from_secs(2), 5, 30, 8_192).unwrap();
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
    let config = MakerActorSupervisorConfig::new(Duration::from_secs(2), 5, 30, 8_192).unwrap();
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
               claim) exit {code} ;;\n\
               *) exit 95 ;;\n\
             esac\n",
            at = now_ms + 3_600_000,
            code = zec_reference_actor::LEZ_REVEALING_CLAIM_REPLAN_EXHAUSTED_EXIT_CODE,
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
    assert_eq!(
        class.as_deref(),
        Some("lez_revealing_claim_replan_exhausted")
    );

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

#[test]
fn a_failed_swaps_timed_claim_in_flight_holds_the_next_until_its_refund_time() {
    // Review 4, L3: 3bf22848 let a failed swap's in-flight marker hold nothing
    // whatever the failure, including one after the claim was submitted (its
    // output unreadable): that claim may still be pending at the nonce, and the
    // next swap's claim would compete with it. A timed marker of a failed swap
    // holds until its refund time like any other; only an old-format marker
    // with no time is ignored (it held forever), and the actor removes its
    // marker itself on the failures where its claim can no longer land.
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let claims = root.path().join("lez-claims-submitted");
    let now_ms = unix_now_ms();
    let owner = MakerActorLeaseOwner::new([0x7c; 16]).unwrap();
    let mut store = SqliteSwapStore::open(root.path().join("maker.sqlite3")).unwrap();
    let holder = "m5-failed-after-submit";
    let holder_refund_at = now_ms + 3_600_000;
    let holder_state = register_gated_actor(&mut store, root.path(), holder, |_| {
        format!(
            "#!/bin/sh\n\
             test \"$1\" = \"--config-fd\" || exit 91\n\
             case \"$3\" in\n\
               status) printf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"state\":\"active\",\"phase\":\"claim_evidence_available\",\"revision\":4,\"next_action\":\"claim_lez\",\"lez_refund_at_ms\":{at},\"revealing_claim_hold_attention_ms\":60000}}' ;;\n\
               claim) printf '%s\\n' 'not an effect' ;;\n\
               *) exit 95 ;;\n\
             esac\n",
            at = holder_refund_at,
        )
    });
    // The claim was submitted (its marker records the refund time) before the
    // attempt's output failed.
    fs::write(
        zec_reference_actor::revealing_claim_in_flight_marker(&holder_state).unwrap(),
        format!("follow-up LEZ claim submitted\nlez_refund_at_ms={holder_refund_at}\n"),
    )
    .unwrap();
    let (swap, resolution, class) = run_due(&mut store, owner, 10);
    assert_eq!(swap, holder);
    assert_eq!(resolution, MakerActorSupervisorResolution::Failed);
    assert_eq!(class.as_deref(), Some("actor_output_invalid"));

    let follower = "m5-failed-after-submit-follower";
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
        (
            MakerActorSupervisorResolution::Backoff,
            Some("lez_revealing_claim_in_flight")
        ),
        "a failed swap's submitted claim holds the nonce until its refund time"
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
