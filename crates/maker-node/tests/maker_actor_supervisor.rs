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
    let now = clock_gettime(ClockId::Boottime);
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
           status) printf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"state\":\"active\",\"phase\":\"both_legs_locked\",\"revision\":3,\"next_action\":\"claim_lez\"}}' ;;\n\
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
           status) printf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"state\":\"active\",\"phase\":\"both_legs_locked\",\"revision\":3,\"next_action\":\"refund_zcash\"}}' ;;\n\
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
    let program = matrix_actor_program(case, &invocation_log);
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
           status) printf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"state\":\"active\",\"phase\":\"both_legs_locked\",\"revision\":3,\"next_action\":\"claim_lez\"}}' ;;\n\
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
          status) printf '%s\\n' '{\"schema_version\":1,\"role\":\"maker\",\"state\":\"active\",\"phase\":\"both_legs_locked\",\"revision\":3,\"next_action\":\"claim_lez\"}' ;;\n\
          claim) printf '%s\\n' '{\"schema_version\":1,\"role\":\"maker\",\"command\":\"claim\",\"outcome\":\"submitted\",\"phase\":\"both_legs_locked\",\"revision\":2,\"next_action\":\"claim_lez\"}' ;;\n\
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
        &MakerActorProgressObservationV1::active("both_legs_locked", 3, "claim_lez").unwrap()
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
