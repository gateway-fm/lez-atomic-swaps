//! Exercises the XMR reference actor; compiled only with `pair-xmr`.
#![cfg(feature = "pair-xmr")]

#[allow(dead_code)]
#[path = "support/xmr_chat_fixture.rs"]
mod xmr_chat_fixture;
#[path = "support/xmr_maker_effect_fixture.rs"]
mod xmr_maker_effect_fixture;

use std::{fs, os::unix::fs::PermissionsExt as _, path::Path, time::Duration};

use lez_bridge_protocol::RequestId;
use lez_maker_node::{
    MakerActorSupervisorConfig, MakerActorSupervisorResolution, supervise_one_due_maker_actor,
};
use lez_swap_core::{
    Chain, ChainPosition, ConfirmationPolicy, Pair, RecoverySchedule, SwapCoordinator,
    SwapDirection, SwapId,
};
use lez_swap_store::{
    MakerActorKindV1, MakerActorLeaseOwner, MakerActorManifestV1, MakerActorManualAction,
    MakerActorManualActionState, MakerActorProgressObservationV1, MakerActorScheduleState,
    SqliteSwapStore,
};
#[cfg(feature = "test-crash-hooks")]
use rustix::process::{Pid, Signal, kill_process_group};
use sha2::{Digest as _, Sha256};
use tempfile::tempdir;

use xmr_chat_fixture::XmrChatFixture;
#[cfg(feature = "test-crash-hooks")]
use xmr_maker_effect_fixture::provision_maker_claim;
use xmr_maker_effect_fixture::{provision_maker_refund, provision_maker_tag17};

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one test keeps the two-cycle exact-effect assertions visible"
)]
fn real_maker_actor_executes_both_recovery_branches_once_then_reconciles() {
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let actor_program = root.path().join("lez-xmr-maker-actor");
    fs::copy(
        Path::new(env!("CARGO_BIN_EXE_lez-xmr-maker-actor")),
        &actor_program,
    )
    .unwrap();
    fs::set_permissions(&actor_program, fs::Permissions::from_mode(0o700)).unwrap();
    let swap_bytes = [0x5c; 32];
    let fixture = XmrChatFixture::new(root.path(), swap_bytes, 1_000_000, 25_000, &actor_program);
    let effect = provision_maker_tag17(&fixture, root.path());
    assert!(effect.workflow.exists());

    let mut store = SqliteSwapStore::open(root.path().join("maker.sqlite3")).unwrap();
    store.save(&xmr_swap(&fixture.swap_id)).unwrap();
    store
        .register_maker_actor(
            &MakerActorManifestV1::new(
                fixture.swap_id.clone(),
                MakerActorKindV1::Monero,
                effect.config.clone(),
                Sha256::digest(fs::read(&effect.config).unwrap()).into(),
                actor_program.clone(),
                Sha256::digest(fs::read(&actor_program).unwrap()).into(),
                fixture.maker_actor_state.clone(),
            )
            .unwrap(),
            10,
        )
        .unwrap();
    store
        .queue_maker_actor_manual_action(
            &RequestId::new("m7-real-tag17-recover-001").unwrap(),
            &fixture.swap_id,
            MakerActorManualAction::Refund,
            0,
            10,
        )
        .unwrap();
    let config = MakerActorSupervisorConfig::new(Duration::from_secs(10), 5, 30, 8_192).unwrap();

    let submitted = supervise_one_due_maker_actor(
        &mut store,
        MakerActorLeaseOwner::new([0x74; 16]).unwrap(),
        10,
        &config,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        submitted.resolution(),
        MakerActorSupervisorResolution::Requeued,
        "records={:?} effect_log={:?}",
        store.list_maker_actor_processes(),
        fs::read_to_string(&effect.effect_log)
    );
    assert_eq!(
        fs::read_to_string(&effect.effect_log).unwrap(),
        "preflight\ninvoke\n"
    );
    assert_eq!(
        store
            .maker_actor_manual_action(&fixture.swap_id)
            .unwrap()
            .unwrap()
            .state(),
        MakerActorManualActionState::Queued
    );

    let reconciled = supervise_one_due_maker_actor(
        &mut store,
        MakerActorLeaseOwner::new([0x75; 16]).unwrap(),
        15,
        &config,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        reconciled.resolution(),
        MakerActorSupervisorResolution::Terminal
    );
    assert_eq!(
        fs::read_to_string(&effect.effect_log).unwrap(),
        "preflight\ninvoke\nobserve\n",
        "restart observation must not repeat preflight or Tag17 submission"
    );
    assert_eq!(
        store
            .maker_actor_manual_action(&fixture.swap_id)
            .unwrap()
            .unwrap()
            .state(),
        MakerActorManualActionState::Completed
    );
    let record = store.list_maker_actor_processes().unwrap().remove(0);
    assert_eq!(record.schedule_state(), MakerActorScheduleState::Terminal);
    assert_eq!(
        store
            .maker_actor_progress(&fixture.swap_id)
            .unwrap()
            .unwrap()
            .observation(),
        &MakerActorProgressObservationV1::active("refunded", 2, "complete").unwrap()
    );

    let refund = provision_maker_refund(&fixture, root.path());
    assert!(refund.workflow.exists());
    let mut refund_store = SqliteSwapStore::open(root.path().join("maker-refund.sqlite3")).unwrap();
    refund_store.save(&xmr_swap(&fixture.swap_id)).unwrap();
    refund_store
        .register_maker_actor(
            &MakerActorManifestV1::new(
                fixture.swap_id.clone(),
                MakerActorKindV1::Monero,
                refund.config.clone(),
                Sha256::digest(fs::read(&refund.config).unwrap()).into(),
                actor_program.clone(),
                Sha256::digest(fs::read(&actor_program).unwrap()).into(),
                fixture.maker_actor_state.clone(),
            )
            .unwrap(),
            20,
        )
        .unwrap();
    refund_store
        .queue_maker_actor_manual_action(
            &RequestId::new("m7-real-monero-refund-001").unwrap(),
            &fixture.swap_id,
            MakerActorManualAction::Refund,
            0,
            20,
        )
        .unwrap();

    let refund_submitted = supervise_one_due_maker_actor(
        &mut refund_store,
        MakerActorLeaseOwner::new([0x76; 16]).unwrap(),
        20,
        &config,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        refund_submitted.resolution(),
        MakerActorSupervisorResolution::Requeued,
        "records={:?} effect_log={:?}",
        refund_store.list_maker_actor_processes(),
        fs::read_to_string(&refund.effect_log)
    );
    assert_eq!(fs::read_to_string(&refund.effect_log).unwrap(), "invoke\n");

    let refund_reconciled = supervise_one_due_maker_actor(
        &mut refund_store,
        MakerActorLeaseOwner::new([0x77; 16]).unwrap(),
        25,
        &config,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        refund_reconciled.resolution(),
        MakerActorSupervisorResolution::Terminal
    );
    assert_eq!(
        fs::read_to_string(&refund.effect_log).unwrap(),
        "invoke\nobserve\n",
        "restart observation must not repeat the Monero refund sweep"
    );
    assert_eq!(
        refund_store
            .maker_actor_manual_action(&fixture.swap_id)
            .unwrap()
            .unwrap()
            .state(),
        MakerActorManualActionState::Completed
    );
    let refund_record = refund_store.list_maker_actor_processes().unwrap().remove(0);
    assert_eq!(
        refund_record.schedule_state(),
        MakerActorScheduleState::Terminal
    );
    assert_eq!(
        refund_store
            .maker_actor_progress(&fixture.swap_id)
            .unwrap()
            .unwrap()
            .observation(),
        &MakerActorProgressObservationV1::active("refunded", 2, "complete").unwrap()
    );
}

#[cfg(feature = "test-crash-hooks")]
#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one test keeps the crash boundary and no-resend proof visible"
)]
fn killed_refund_actor_reconciles_durable_submission_without_resend() {
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let actor_program = root.path().join("lez-xmr-maker-actor");
    fs::copy(
        Path::new(env!("CARGO_BIN_EXE_lez-xmr-maker-actor")),
        &actor_program,
    )
    .unwrap();
    fs::set_permissions(&actor_program, fs::Permissions::from_mode(0o700)).unwrap();
    let fixture = XmrChatFixture::new(root.path(), [0x6d; 32], 1_000_000, 25_000, &actor_program);
    let refund = provision_maker_refund(&fixture, root.path());
    let database = root.path().join("maker-refund-crash.sqlite3");
    let marker = root.path().join("refund-actor-paused.json");
    let mut store = SqliteSwapStore::open(&database).unwrap();
    store.save(&xmr_swap(&fixture.swap_id)).unwrap();
    store
        .register_maker_actor(
            &MakerActorManifestV1::new(
                fixture.swap_id.clone(),
                MakerActorKindV1::Monero,
                refund.config.clone(),
                Sha256::digest(fs::read(&refund.config).unwrap()).into(),
                actor_program.clone(),
                Sha256::digest(fs::read(&actor_program).unwrap()).into(),
                fixture.maker_actor_state.clone(),
            )
            .unwrap(),
            10,
        )
        .unwrap();
    store
        .queue_maker_actor_manual_action(
            &RequestId::new("m7-killed-monero-refund-001").unwrap(),
            &fixture.swap_id,
            MakerActorManualAction::Refund,
            0,
            10,
        )
        .unwrap();
    let crash_config = MakerActorSupervisorConfig::new(Duration::from_mins(1), 5, 30, 8_192)
        .unwrap()
        .with_test_pause_after_submitted(
            fixture.swap_id.clone(),
            "sweep_monero_refund",
            marker.clone(),
        )
        .unwrap();

    let worker = std::thread::spawn(move || {
        let outcome = supervise_one_due_maker_actor(
            &mut store,
            MakerActorLeaseOwner::new([0x78; 16]).unwrap(),
            10,
            &crash_config,
        )
        .unwrap()
        .unwrap();
        (outcome, store)
    });
    let marker_deadline = std::time::Instant::now() + Duration::from_mins(1);
    while std::time::Instant::now() < marker_deadline {
        if marker.exists() || worker.is_finished() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    if !marker.exists() {
        let (outcome, _) = worker.join().unwrap();
        panic!(
            "submitted-effect pause marker is absent; resolution={:?}, effect_log={:?}",
            outcome.resolution(),
            fs::read_to_string(&refund.effect_log)
        );
    }
    let pause: serde_json::Value =
        serde_json::from_slice(&fs::read(&marker).expect("submitted-effect pause marker")).unwrap();
    assert_eq!(pause["state"], "paused_after_submitted_before_stdout");
    assert_eq!(pause["swap_id"], fixture.swap_id.as_str());
    assert_eq!(pause["operation"], "sweep_monero_refund");
    let raw_pid = pause["process_id"].as_u64().unwrap();
    let pid = Pid::from_raw(i32::try_from(raw_pid).unwrap()).unwrap();
    kill_process_group(pid, Signal::KILL).unwrap();

    let (crashed, mut store) = worker.join().unwrap();
    assert_eq!(
        crashed.resolution(),
        MakerActorSupervisorResolution::Backoff
    );
    assert_eq!(fs::read_to_string(&refund.effect_log).unwrap(), "invoke\n");
    assert_eq!(
        store
            .maker_actor_manual_action(&fixture.swap_id)
            .unwrap()
            .unwrap()
            .state(),
        MakerActorManualActionState::Queued
    );

    let reconcile_config =
        MakerActorSupervisorConfig::new(Duration::from_mins(1), 5, 30, 8_192).unwrap();
    let recovered = supervise_one_due_maker_actor(
        &mut store,
        MakerActorLeaseOwner::new([0x79; 16]).unwrap(),
        40,
        &reconcile_config,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        recovered.resolution(),
        MakerActorSupervisorResolution::Terminal
    );
    assert_eq!(
        fs::read_to_string(&refund.effect_log).unwrap(),
        "invoke\nobserve\n",
        "a killed submitted refund actor must resume with observation, not another send"
    );
    assert_eq!(
        store
            .maker_actor_manual_action(&fixture.swap_id)
            .unwrap()
            .unwrap()
            .state(),
        MakerActorManualActionState::Completed
    );
}

#[cfg(feature = "test-crash-hooks")]
#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the exact Tag15 kill boundary and no-resend proof remain visible"
)]
fn killed_tag15_actor_reconciles_durable_submission_without_resend() {
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let actor_program = root.path().join("lez-xmr-maker-actor");
    fs::copy(
        Path::new(env!("CARGO_BIN_EXE_lez-xmr-maker-actor")),
        &actor_program,
    )
    .unwrap();
    fs::set_permissions(&actor_program, fs::Permissions::from_mode(0o700)).unwrap();
    let fixture = XmrChatFixture::new(root.path(), [0x7d; 32], 1_000_000, 25_000, &actor_program);
    let claim = provision_maker_claim(&fixture, root.path());
    let database = root.path().join("maker-claim-crash.sqlite3");
    let marker = root.path().join("tag15-actor-paused.json");
    let mut store = SqliteSwapStore::open(&database).unwrap();
    store.save(&xmr_swap(&fixture.swap_id)).unwrap();
    store
        .register_maker_actor(
            &MakerActorManifestV1::new(
                fixture.swap_id.clone(),
                MakerActorKindV1::Monero,
                claim.config.clone(),
                Sha256::digest(fs::read(&claim.config).unwrap()).into(),
                actor_program.clone(),
                Sha256::digest(fs::read(&actor_program).unwrap()).into(),
                fixture.maker_actor_state.clone(),
            )
            .unwrap(),
            10,
        )
        .unwrap();
    store
        .queue_maker_actor_manual_action(
            &RequestId::new("m7-killed-tag15-claim-001").unwrap(),
            &fixture.swap_id,
            MakerActorManualAction::Claim,
            0,
            10,
        )
        .unwrap();
    let crash_config = MakerActorSupervisorConfig::new(Duration::from_mins(1), 5, 30, 8_192)
        .unwrap()
        .with_test_pause_after_submitted(fixture.swap_id.clone(), "claim_lez_tag15", marker.clone())
        .unwrap();

    let worker = std::thread::spawn(move || {
        let outcome = supervise_one_due_maker_actor(
            &mut store,
            MakerActorLeaseOwner::new([0x7a; 16]).unwrap(),
            10,
            &crash_config,
        )
        .unwrap()
        .unwrap();
        (outcome, store)
    });
    let marker_deadline = std::time::Instant::now() + Duration::from_mins(1);
    while std::time::Instant::now() < marker_deadline {
        if marker.exists() || worker.is_finished() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    if !marker.exists() {
        let (outcome, _) = worker.join().unwrap();
        panic!(
            "Tag15 pause marker is absent; resolution={:?}, effect_log={:?}",
            outcome.resolution(),
            fs::read_to_string(&claim.effect_log)
        );
    }
    let pause: serde_json::Value =
        serde_json::from_slice(&fs::read(&marker).expect("Tag15 pause marker")).unwrap();
    assert_eq!(pause["state"], "paused_after_submitted_before_stdout");
    assert_eq!(pause["swap_id"], fixture.swap_id.as_str());
    assert_eq!(pause["operation"], "claim_lez_tag15");
    let raw_pid = pause["process_id"].as_u64().unwrap();
    let pid = Pid::from_raw(i32::try_from(raw_pid).unwrap()).unwrap();
    kill_process_group(pid, Signal::KILL).unwrap();

    let (crashed, mut store) = worker.join().unwrap();
    assert_eq!(
        crashed.resolution(),
        MakerActorSupervisorResolution::Backoff
    );
    assert_eq!(fs::read_to_string(&claim.effect_log).unwrap(), "invoke\n");
    assert_eq!(
        store
            .maker_actor_manual_action(&fixture.swap_id)
            .unwrap()
            .unwrap()
            .state(),
        MakerActorManualActionState::Queued
    );

    let reconcile_config =
        MakerActorSupervisorConfig::new(Duration::from_mins(1), 5, 30, 8_192).unwrap();
    let recovered = supervise_one_due_maker_actor(
        &mut store,
        MakerActorLeaseOwner::new([0x7b; 16]).unwrap(),
        40,
        &reconcile_config,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        recovered.resolution(),
        MakerActorSupervisorResolution::Terminal
    );
    assert_eq!(
        fs::read_to_string(&claim.effect_log).unwrap(),
        "invoke\nobserve\n",
        "a killed submitted Tag15 actor must resume with observation, not another send"
    );
    assert_eq!(
        store
            .maker_actor_manual_action(&fixture.swap_id)
            .unwrap()
            .unwrap()
            .state(),
        MakerActorManualActionState::Completed
    );
    assert_eq!(
        store
            .maker_actor_progress(&fixture.swap_id)
            .unwrap()
            .unwrap()
            .observation(),
        &MakerActorProgressObservationV1::active("completed", 2, "complete").unwrap()
    );
}

fn xmr_swap(id: &SwapId) -> SwapCoordinator {
    SwapCoordinator::new_with_confirmation_policies(
        id.clone(),
        Pair::Monero,
        SwapDirection::TakerSellsLez,
        ConfirmationPolicy::new(2).unwrap(),
        ConfirmationPolicy::new(10).unwrap(),
        RecoverySchedule::xmr_lez_first(ChainPosition::timestamp(Chain::Lez, 20), 2).unwrap(),
    )
}
