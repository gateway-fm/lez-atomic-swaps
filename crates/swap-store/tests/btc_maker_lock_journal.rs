use lez_swap_core::{
    Chain, ChainPosition, ConfirmationPolicy, Pair, Participant, RecoverySchedule, SwapCoordinator,
    SwapDirection, SwapId, TimelockSafety,
};
use lez_swap_sdk_core::{
    ExactPublicEffectBytes, ExactPublicEffectPlanV1, ExpectedPublicEffectId, PublicEffectStepId,
    PublicEffectStepV1,
};
use lez_swap_store::{
    BtcAgreementAcceptance, BtcLifecycleEvidenceV1, BtcMakerLockIntentCreateOutcome,
    BtcMakerLockIntentV1, BtcMakerLockStepDecision, BtcMakerLockStepObservation,
    BtcMakerLockStepState, BtcMakerLockSubmissionResult, BtcRecoveryError,
    SqliteBtcMakerLockJournal, SqliteBtcRecoveryStore, StoreError,
};
use rusqlite::Connection;
use tempfile::tempdir;

fn step(id: &str, expected_id: &str, bytes: &[u8]) -> PublicEffectStepV1 {
    PublicEffectStepV1::new(
        PublicEffectStepId::new(id).expect("valid step ID"),
        ExpectedPublicEffectId::new(expected_id).expect("valid expected ID"),
        ExactPublicEffectBytes::new(bytes.to_vec()).expect("valid public bytes"),
    )
}

fn lez_plan() -> ExactPublicEffectPlanV1 {
    ExactPublicEffectPlanV1::new(vec![
        step("lez.initialize", "lez-init-id", &[0x10, 0x11]),
        step("lez.fund", "lez-fund-id", &[0x20, 0x21]),
    ])
    .expect("valid LEZ plan")
}

fn bitcoin_plan() -> ExactPublicEffectPlanV1 {
    ExactPublicEffectPlanV1::new(vec![step(
        "bitcoin.funding",
        "bitcoin-funding-txid",
        &[0x30, 0x31],
    )])
    .expect("valid Bitcoin plan")
}

fn intent(swap: &str, plan: ExactPublicEffectPlanV1) -> BtcMakerLockIntentV1 {
    BtcMakerLockIntentV1::new(
        SwapId::new(swap).expect("valid swap ID"),
        [0x42; 32],
        Participant::Maker,
        1,
        plan,
    )
    .expect("valid maker intent")
}

fn exact(step: &PublicEffectStepV1) -> BtcMakerLockStepObservation {
    BtcMakerLockStepObservation::PresentExact {
        expected_public_id: step.expected_public_id().as_str().into(),
        exact_public_bytes: step.exact_bytes().as_slice().to_vec(),
    }
}

#[test]
fn lez_plan_is_durable_in_order_and_exact_reopen_is_idempotent() {
    let directory = tempdir().expect("temporary store");
    let path = directory.path().join("maker.sqlite3");
    let candidate = intent("btc-maker-lez-order", lez_plan());
    let mut journal = SqliteBtcMakerLockJournal::open(&path).expect("open journal");

    assert_eq!(
        journal.create_intent(&candidate).expect("create intent"),
        BtcMakerLockIntentCreateOutcome::Created
    );
    assert_eq!(
        journal.create_intent(&candidate).expect("replay intent"),
        BtcMakerLockIntentCreateOutcome::ExistingSame
    );

    let fund = candidate.plan().steps()[1].step();
    let BtcMakerLockStepDecision::ObserveOnly(blocked) = journal
        .reconcile_step(&candidate, fund, BtcMakerLockStepObservation::Absent)
        .expect("later step stays blocked")
    else {
        panic!("LEZ fund must not start before initialization is accepted");
    };
    assert_eq!(blocked.state(), BtcMakerLockStepState::Prepared);

    let initialize = candidate.plan().steps()[0].step();
    assert!(matches!(
        journal
            .reconcile_step(&candidate, initialize, BtcMakerLockStepObservation::Absent,)
            .expect("initialize decision"),
        BtcMakerLockStepDecision::SubmitOnce(_)
    ));
    let _ = journal
        .record_submission_result(
            &candidate,
            initialize,
            &BtcMakerLockSubmissionResult::Accepted("lez-init-id".into()),
        )
        .expect("initialize accepted");
    let initialize_step = candidate.plan().steps()[0].clone();
    let BtcMakerLockStepDecision::ObserveOnly(initialize_confirmed) = journal
        .reconcile_step(&candidate, initialize, exact(&initialize_step))
        .expect("observe confirmed initialization")
    else {
        panic!("node admission must remain observe-only until exact evidence");
    };
    assert_eq!(
        initialize_confirmed.state(),
        BtcMakerLockStepState::Accepted
    );
    assert!(matches!(
        journal
            .reconcile_step(&candidate, fund, BtcMakerLockStepObservation::Absent)
            .expect("fund decision"),
        BtcMakerLockStepDecision::SubmitOnce(_)
    ));
    drop(journal);

    let reopened = SqliteBtcMakerLockJournal::open(&path).expect("reopen journal");
    let durable = reopened
        .load_intent(candidate.swap_id())
        .expect("load durable intent")
        .expect("intent exists");
    assert_eq!(durable.intent(), &candidate);
    assert_eq!(durable.steps()[0].state(), BtcMakerLockStepState::Accepted);
    assert_eq!(durable.steps()[1].state(), BtcMakerLockStepState::Started);
}

#[test]
fn bitcoin_single_step_records_expected_id_and_exact_bytes() {
    let directory = tempdir().expect("temporary store");
    let path = directory.path().join("maker.sqlite3");
    let candidate = intent("btc-maker-bitcoin", bitcoin_plan());
    let step = &candidate.plan().steps()[0];
    let mut journal = SqliteBtcMakerLockJournal::open(&path).expect("open journal");
    assert_eq!(
        journal.create_intent(&candidate).expect("create intent"),
        BtcMakerLockIntentCreateOutcome::Created
    );
    let decision = journal
        .reconcile_step(&candidate, step.step(), exact(step))
        .expect("observe exact completion");
    let BtcMakerLockStepDecision::ObserveOnly(accepted) = decision else {
        panic!("exact observed completion must not authorize a send");
    };
    assert_eq!(accepted.state(), BtcMakerLockStepState::Accepted);
    assert_eq!(accepted.attempt_count(), 0);
    assert_eq!(accepted.revision(), 1);
}

#[test]
fn immutable_agreement_plan_id_or_bytes_drift_conflicts() {
    let directory = tempdir().expect("temporary store");
    let path = directory.path().join("maker.sqlite3");
    let candidate = intent("btc-maker-conflict", bitcoin_plan());
    let mut journal = SqliteBtcMakerLockJournal::open(&path).expect("open journal");
    assert_eq!(
        journal.create_intent(&candidate).expect("create intent"),
        BtcMakerLockIntentCreateOutcome::Created
    );

    let changed_agreement = BtcMakerLockIntentV1::new(
        candidate.swap_id().clone(),
        [0x43; 32],
        Participant::Maker,
        1,
        bitcoin_plan(),
    )
    .expect("valid changed intent");
    let changed_id = intent(
        "btc-maker-conflict",
        ExactPublicEffectPlanV1::new(vec![step("bitcoin.funding", "changed-txid", &[0x30, 0x31])])
            .expect("valid plan"),
    );
    let changed_bytes = intent(
        "btc-maker-conflict",
        ExactPublicEffectPlanV1::new(vec![step(
            "bitcoin.funding",
            "bitcoin-funding-txid",
            &[0x30, 0xff],
        )])
        .expect("valid plan"),
    );
    for changed in [&changed_agreement, &changed_id, &changed_bytes] {
        assert_eq!(
            journal.create_intent(changed).expect("conflict outcome"),
            BtcMakerLockIntentCreateOutcome::Conflict
        );
    }
}

#[test]
fn started_or_unknown_step_never_rearms_after_restart() {
    let directory = tempdir().expect("temporary store");
    let path = directory.path().join("maker.sqlite3");
    let candidate = intent("btc-maker-unknown", bitcoin_plan());
    let durable_step = candidate.plan().steps()[0].clone();
    let mut journal = SqliteBtcMakerLockJournal::open(&path).expect("open journal");
    let _ = journal.create_intent(&candidate).expect("create intent");
    assert!(matches!(
        journal
            .reconcile_step(
                &candidate,
                durable_step.step(),
                BtcMakerLockStepObservation::Absent,
            )
            .expect("start once"),
        BtcMakerLockStepDecision::SubmitOnce(_)
    ));
    drop(journal);

    let mut restarted = SqliteBtcMakerLockJournal::open(&path).expect("restart journal");
    for observation in [
        BtcMakerLockStepObservation::Absent,
        BtcMakerLockStepObservation::Uncertain,
    ] {
        assert!(matches!(
            restarted
                .reconcile_step(&candidate, durable_step.step(), observation)
                .expect("observe only after crash"),
            BtcMakerLockStepDecision::ObserveOnly(_)
        ));
    }
    let _ = restarted
        .record_submission_result(
            &candidate,
            durable_step.step(),
            &BtcMakerLockSubmissionResult::Unknown,
        )
        .expect("record ambiguous send");
    assert!(matches!(
        restarted
            .reconcile_step(
                &candidate,
                durable_step.step(),
                BtcMakerLockStepObservation::Absent,
            )
            .expect("unknown remains observe-only"),
        BtcMakerLockStepDecision::ObserveOnly(_)
    ));

    let BtcMakerLockStepDecision::ObserveOnly(accepted) = restarted
        .reconcile_step(&candidate, durable_step.step(), exact(&durable_step))
        .expect("exact observation resolves ambiguity")
    else {
        panic!("exact observation must remain observe-only");
    };
    assert_eq!(accepted.state(), BtcMakerLockStepState::Accepted);
    assert_eq!(accepted.attempt_count(), 1);
    assert_eq!(accepted.revision(), 3);
}

#[test]
fn submission_result_replay_is_exact_and_changed_classification_conflicts() {
    let directory = tempdir().expect("temporary stores");
    for (case, first, changed) in [
        (
            "accepted-then-unknown",
            BtcMakerLockSubmissionResult::Accepted("bitcoin-funding-txid".into()),
            BtcMakerLockSubmissionResult::Unknown,
        ),
        (
            "unknown-then-accepted",
            BtcMakerLockSubmissionResult::Unknown,
            BtcMakerLockSubmissionResult::Accepted("bitcoin-funding-txid".into()),
        ),
    ] {
        let path = directory.path().join(format!("{case}.sqlite3"));
        let candidate = intent(&format!("btc-maker-{case}"), bitcoin_plan());
        let step = candidate.plan().steps()[0].step();
        let mut journal = SqliteBtcMakerLockJournal::open(&path).expect("open journal");
        let _ = journal.create_intent(&candidate).expect("create intent");
        assert!(matches!(
            journal
                .reconcile_step(&candidate, step, BtcMakerLockStepObservation::Absent)
                .expect("consume send authority"),
            BtcMakerLockStepDecision::SubmitOnce(_)
        ));

        let committed = journal
            .record_submission_result(&candidate, step, &first)
            .expect("record first result");
        assert!(!committed.was_replay());
        assert_eq!(committed.snapshot().submission_result(), Some(&first));
        drop(journal);

        let mut reopened = SqliteBtcMakerLockJournal::open(&path).expect("reopen journal");
        let replay = reopened
            .record_submission_result(&candidate, step, &first)
            .expect("exact result replay");
        assert!(replay.was_replay());
        assert_eq!(replay.snapshot().submission_result(), Some(&first));
        assert!(matches!(
            reopened.record_submission_result(&candidate, step, &changed),
            Err(StoreError::BtcMakerLockConflict)
        ));
    }
}

#[test]
fn only_maker_revision_one_can_stage_the_second_lock() {
    let swap_id = SwapId::new("btc-maker-wrong-context").expect("valid swap ID");
    assert!(matches!(
        BtcMakerLockIntentV1::new(
            swap_id.clone(),
            [0x42; 32],
            Participant::Taker,
            1,
            bitcoin_plan(),
        ),
        Err(StoreError::InvalidBtcMakerLockIntent)
    ));
    assert!(matches!(
        BtcMakerLockIntentV1::new(swap_id, [0x42; 32], Participant::Maker, 2, bitcoin_plan(),),
        Err(StoreError::InvalidBtcMakerLockIntent)
    ));
}

#[test]
fn multi_step_creation_rolls_back_every_row_on_failure() {
    let directory = tempdir().expect("temporary store");
    let path = directory.path().join("maker.sqlite3");
    let candidate = intent("btc-maker-create-rollback", lez_plan());
    let mut journal = SqliteBtcMakerLockJournal::open(&path).expect("open journal");
    let raw = Connection::open(&path).expect("open raw connection");
    raw.execute_batch(
        "CREATE TRIGGER fail_second_maker_step
         BEFORE INSERT ON btc_maker_lock_steps
         WHEN NEW.step_id = 'lez.fund'
         BEGIN SELECT RAISE(FAIL, 'forced second step failure'); END;",
    )
    .expect("install failure trigger");

    assert!(matches!(
        journal.create_intent(&candidate),
        Err(StoreError::Sqlite(_))
    ));
    let intents: i64 = raw
        .query_row("SELECT COUNT(*) FROM btc_maker_lock_intents", [], |row| {
            row.get(0)
        })
        .expect("intent count");
    let steps: i64 = raw
        .query_row("SELECT COUNT(*) FROM btc_maker_lock_steps", [], |row| {
            row.get(0)
        })
        .expect("step count");
    assert_eq!((intents, steps), (0, 0));
}

#[test]
fn durable_exact_plan_mutations_fail_closed_after_reopen() {
    let directory = tempdir().expect("temporary stores");
    for (case, mutation) in [
        (
            "bytes",
            "UPDATE btc_maker_lock_steps
             SET exact_public_bytes = x'3032'
             WHERE step_id = 'bitcoin.funding'",
        ),
        (
            "id",
            "UPDATE btc_maker_lock_steps
             SET expected_public_id = 'different-funding-txid'
             WHERE step_id = 'bitcoin.funding'",
        ),
    ] {
        let path = directory.path().join(format!("corrupt-{case}.sqlite3"));
        let candidate = intent(&format!("btc-maker-corrupt-{case}"), bitcoin_plan());
        let mut journal = SqliteBtcMakerLockJournal::open(&path).expect("open journal");
        let _ = journal.create_intent(&candidate).expect("create intent");
        drop(journal);
        let raw = Connection::open(&path).expect("open raw connection");
        raw.execute(mutation, []).expect("mutate durable plan");
        drop(raw);

        let reopened = SqliteBtcMakerLockJournal::open(&path).expect("reopen schema");
        assert!(matches!(
            reopened.load_intent(candidate.swap_id()),
            Err(StoreError::CorruptBtcMakerLockIntent)
        ));
    }
}

#[test]
fn malformed_layout_reduced_constraints_or_triggered_schema_fails_closed() {
    let directory = tempdir().expect("temporary stores");
    let malformed_schemas = [
        (
            "wrong-layout",
            "CREATE TABLE btc_maker_lock_intents (
                 swap_id TEXT NOT NULL, local_role TEXT NOT NULL,
                 predecessor_revision INTEGER NOT NULL, agreement_commitment BLOB NOT NULL,
                 plan_schema_version INTEGER NOT NULL, plan_commitment TEXT NOT NULL,
                 closed_revision INTEGER, PRIMARY KEY (swap_id, local_role)
             ) STRICT;
             CREATE TABLE btc_maker_lock_steps (
                 swap_id TEXT NOT NULL, local_role TEXT NOT NULL, step_index INTEGER NOT NULL,
                 step_id TEXT NOT NULL, expected_public_id TEXT NOT NULL,
                 exact_public_bytes BLOB NOT NULL, public_bytes_sha256 BLOB NOT NULL,
                 submission_result TEXT, state TEXT NOT NULL, attempt_count INTEGER NOT NULL,
                 revision TEXT NOT NULL, PRIMARY KEY (swap_id, local_role, step_index)
             ) STRICT;",
        ),
        (
            "reduced-constraints",
            "CREATE TABLE btc_maker_lock_intents (
                 swap_id TEXT NOT NULL, local_role TEXT NOT NULL,
                 predecessor_revision INTEGER NOT NULL, agreement_commitment BLOB NOT NULL,
                 plan_schema_version INTEGER NOT NULL, plan_commitment BLOB NOT NULL,
                 closed_revision INTEGER, PRIMARY KEY (swap_id, local_role)
             ) STRICT;
             CREATE TABLE btc_maker_lock_steps (
                 swap_id TEXT NOT NULL, local_role TEXT NOT NULL, step_index INTEGER NOT NULL,
                 step_id TEXT NOT NULL, expected_public_id TEXT NOT NULL,
                 exact_public_bytes BLOB NOT NULL, public_bytes_sha256 BLOB NOT NULL,
                 submission_result TEXT, state TEXT NOT NULL, attempt_count INTEGER NOT NULL,
                 revision INTEGER NOT NULL, PRIMARY KEY (swap_id, local_role, step_index)
             ) STRICT;",
        ),
    ];
    for (case, replacement_schema) in malformed_schemas {
        let path = directory.path().join(format!("{case}.sqlite3"));
        drop(SqliteBtcMakerLockJournal::open(&path).expect("create valid schema"));
        let raw = Connection::open(&path).expect("open raw connection");
        raw.execute_batch(
            "DROP TABLE btc_maker_lock_steps;
             DROP TABLE btc_maker_lock_intents;",
        )
        .expect("drop valid schema");
        raw.execute_batch(replacement_schema)
            .expect("install malformed same-count schema");
        drop(raw);
        assert!(matches!(
            SqliteBtcMakerLockJournal::open(&path),
            Err(StoreError::CorruptBtcMakerLockIntent)
        ));
    }

    let trigger_path = directory.path().join("trigger.sqlite3");
    drop(SqliteBtcMakerLockJournal::open(&trigger_path).expect("create valid schema"));
    let raw = Connection::open(&trigger_path).expect("open raw connection");
    raw.execute_batch(
        "CREATE TRIGGER unexpected_maker_lock_trigger
         AFTER INSERT ON btc_maker_lock_steps BEGIN SELECT 1; END;",
    )
    .expect("install unexpected trigger");
    drop(raw);
    assert!(matches!(
        SqliteBtcMakerLockJournal::open(&trigger_path),
        Err(StoreError::CorruptBtcMakerLockIntent)
    ));
}

fn coordinator(swap: &str) -> SwapCoordinator {
    let safety = TimelockSafety::between(Chain::Lez, Chain::Bitcoin, 100, 200, 10)
        .expect("safe timeout order");
    let schedule = RecoverySchedule::new(
        Pair::Bitcoin,
        SwapDirection::TakerSellsForeign,
        ChainPosition::block_height(Chain::Lez, 100),
        ChainPosition::block_height(Chain::Bitcoin, 200),
        safety,
    )
    .expect("valid schedule");
    SwapCoordinator::new_with_confirmation_policies(
        SwapId::new(swap).expect("valid swap ID"),
        Pair::Bitcoin,
        SwapDirection::TakerSellsForeign,
        ConfirmationPolicy::new(1).expect("taker confirmations"),
        ConfirmationPolicy::new(1).expect("maker confirmations"),
        schedule,
    )
}

fn acceptance(initial: &SwapCoordinator) -> BtcAgreementAcceptance {
    BtcAgreementAcceptance::new(
        initial,
        Participant::Maker,
        b"exact-bitcoin-agreement".to_vec(),
        [0x42; 32],
        1_785_000_000,
    )
    .expect("valid acceptance")
}

#[test]
fn maker_projection_and_intent_close_share_one_revision_cas_transaction() {
    let directory = tempdir().expect("temporary store");
    let path = directory.path().join("maker.sqlite3");
    let initial = coordinator("btc-maker-atomic-close");
    let accepted = acceptance(&initial);
    let mut recovery =
        SqliteBtcRecoveryStore::open(&path, &accepted, &initial).expect("activate recovery");
    let _ = recovery
        .project(
            0,
            &BtcLifecycleEvidenceV1::taker_lock(
                Chain::Bitcoin,
                "taker-lock-txid",
                1,
                b"canonical taker lock".to_vec(),
            )
            .expect("taker lock evidence"),
        )
        .expect("project taker lock");

    let candidate = intent("btc-maker-atomic-close", bitcoin_plan());
    let durable_step = candidate.plan().steps()[0].clone();
    let mut journal = SqliteBtcMakerLockJournal::open(&path).expect("open maker journal");
    let _ = journal.create_intent(&candidate).expect("create intent");
    let _ = journal
        .reconcile_step(&candidate, durable_step.step(), exact(&durable_step))
        .expect("accept exact maker effect");
    drop(journal);

    let maker_evidence = BtcLifecycleEvidenceV1::maker_lock(
        Chain::Lez,
        "maker-lock-txid",
        1,
        b"canonical maker lock".to_vec(),
    )
    .expect("maker lock evidence");
    assert!(matches!(
        recovery.project(1, &maker_evidence),
        Err(BtcRecoveryError::InvalidSequence { revision: 2 })
    ));
    assert_eq!(
        recovery
            .status()
            .expect("maker bypass rejection status")
            .revision(),
        1
    );
    let raw = Connection::open(&path).expect("open raw connection");
    raw.execute_batch(
        "CREATE TRIGGER fail_maker_intent_close
         BEFORE UPDATE OF closed_revision ON btc_maker_lock_intents
         BEGIN SELECT RAISE(FAIL, 'forced close failure'); END;",
    )
    .expect("install close trigger");
    assert!(
        recovery
            .project_maker_lock_and_close(1, &maker_evidence)
            .is_err()
    );
    assert_eq!(
        recovery.status().expect("status after rollback").revision(),
        1
    );
    let rolled_back: (i64, Option<i64>) = raw
        .query_row(
            "SELECT
                 (SELECT COUNT(*) FROM btc_actor_evidence WHERE aggregate_revision = 2),
                 (SELECT closed_revision FROM btc_maker_lock_intents
                  WHERE swap_id = 'btc-maker-atomic-close')",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("rollback state");
    assert_eq!(rolled_back, (0, None));
    raw.execute_batch("DROP TRIGGER fail_maker_intent_close;")
        .expect("remove close trigger");

    let committed = recovery
        .project_maker_lock_and_close(1, &maker_evidence)
        .expect("atomic maker projection");
    assert_eq!(committed.revision(), 2);
    assert!(!committed.was_replay());
    let replay = recovery
        .project_maker_lock_and_close(1, &maker_evidence)
        .expect("exact atomic replay");
    assert!(replay.was_replay());
    drop(recovery);

    let journal = SqliteBtcMakerLockJournal::open(&path).expect("reopen maker journal");
    assert_eq!(
        journal
            .load_intent(candidate.swap_id())
            .expect("load intent")
            .expect("intent exists")
            .closed_revision(),
        Some(2)
    );
}

#[test]
fn taker_can_project_observed_maker_lock_through_generic_recovery_api() {
    let directory = tempdir().expect("temporary store");
    let path = directory.path().join("taker.sqlite3");
    let initial = coordinator("btc-taker-observed-maker-lock");
    let accepted = BtcAgreementAcceptance::new(
        &initial,
        Participant::Taker,
        b"exact-bitcoin-agreement".to_vec(),
        [0x42; 32],
        1_785_000_000,
    )
    .expect("valid taker acceptance");
    let mut recovery =
        SqliteBtcRecoveryStore::open(&path, &accepted, &initial).expect("activate taker recovery");
    let _ = recovery
        .project(
            0,
            &BtcLifecycleEvidenceV1::taker_lock(
                Chain::Bitcoin,
                "taker-observed-first-lock",
                1,
                b"canonical taker lock".to_vec(),
            )
            .expect("taker lock evidence"),
        )
        .expect("project taker lock");
    let commit = recovery
        .project(
            1,
            &BtcLifecycleEvidenceV1::maker_lock(
                Chain::Lez,
                "taker-observed-maker-lock",
                1,
                b"canonical maker lock".to_vec(),
            )
            .expect("maker lock evidence"),
        )
        .expect("taker projects observed maker lock");
    assert_eq!(commit.revision(), 2);
    assert_eq!(recovery.status().expect("taker status").revision(), 2);
}
