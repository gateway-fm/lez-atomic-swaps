use std::{
    fs::{self, OpenOptions},
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _},
    path::Path,
};

use lez_swap_core::{Participant, SwapId};
use lez_swap_store::{
    SqliteXmrWorkflowJournal, XmrWorkflowBranch, XmrWorkflowDecision, XmrWorkflowIdentityV1,
    XmrWorkflowReconciliationSource, XmrWorkflowReconciliationV2, XmrWorkflowStep,
    XmrWorkflowStepScope,
};
use rusqlite::Connection;
use tempfile::{TempDir, tempdir};

fn private_root() -> TempDir {
    let root = tempdir().expect("isolated XMR workflow-v2 root");
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700))
        .expect("owner-private workflow-v2 root");
    root
}

fn identity(role: Participant, byte: u8) -> XmrWorkflowIdentityV1 {
    XmrWorkflowIdentityV1::new(
        SwapId::new(format!("{byte:02x}").repeat(32)).expect("valid swap ID"),
        role,
        format!("m5-xmr-workflow-v2-{byte:02x}").into(),
        [byte.wrapping_add(1); 32],
        [byte.wrapping_add(2); 32],
        [byte.wrapping_add(3); 32],
    )
    .expect("valid workflow-v2 identity")
}

fn evidence(byte: u8, source: XmrWorkflowReconciliationSource) -> XmrWorkflowReconciliationV2 {
    XmrWorkflowReconciliationV2::new([byte; 32], [byte.wrapping_add(1); 32], source)
        .expect("nonzero effect evidence and tool-plan identity")
}

fn create(
    root: &Path,
    role: Participant,
    byte: u8,
) -> (SqliteXmrWorkflowJournal, XmrWorkflowIdentityV1) {
    let path = root.join(format!("workflow-{byte:02x}.sqlite3"));
    let mut journal =
        SqliteXmrWorkflowJournal::create_new(path).expect("create schema-v2 workflow");
    let identity = identity(role, byte);
    journal
        .initialize(&identity)
        .expect("initialize schema-v2 workflow");
    (journal, identity)
}

fn complete_prepared(
    journal: &mut SqliteXmrWorkflowJournal,
    identity: &XmrWorkflowIdentityV1,
    step: XmrWorkflowStep,
    proof: &XmrWorkflowReconciliationV2,
) {
    assert_eq!(
        journal
            .authorize_once(identity, step)
            .expect("consume one invocation authority"),
        XmrWorkflowDecision::InvokeOnce
    );
    journal
        .reconcile_succeeded(identity, step, proof)
        .expect("persist exact external-effect reconciliation");
    assert_eq!(
        journal
            .authorize_once(identity, step)
            .expect("completed step never rearms"),
        XmrWorkflowDecision::Complete
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn schema_v2_catalog_has_exact_scopes_roles_and_role_local_predecessors() {
    use XmrWorkflowStep::{
        AuthorizeLezTag14, ClaimLezTag15, FundLezTag13, FundMonero, InitializeLezTag13,
        RefundLezTag16, SweepMoneroClaim, SweepMoneroRefund,
    };

    let expected = [
        (
            InitializeLezTag13,
            XmrWorkflowStepScope::Common,
            Participant::Taker,
        ),
        (
            FundLezTag13,
            XmrWorkflowStepScope::Common,
            Participant::Taker,
        ),
        (FundMonero, XmrWorkflowStepScope::Common, Participant::Maker),
        (
            AuthorizeLezTag14,
            XmrWorkflowStepScope::Claim,
            Participant::Taker,
        ),
        (
            ClaimLezTag15,
            XmrWorkflowStepScope::Claim,
            Participant::Maker,
        ),
        (
            SweepMoneroClaim,
            XmrWorkflowStepScope::Claim,
            Participant::Taker,
        ),
        (
            RefundLezTag16,
            XmrWorkflowStepScope::Refund,
            Participant::Taker,
        ),
        (
            SweepMoneroRefund,
            XmrWorkflowStepScope::Refund,
            Participant::Maker,
        ),
    ];
    assert_eq!(XmrWorkflowStep::ALL, expected.map(|(step, _, _)| step));
    for (step, scope, role) in expected {
        assert_eq!(step.scope(), scope);
        assert_eq!(step.role(), role);
    }

    let root = private_root();
    let (mut taker, taker_id) = create(root.path(), Participant::Taker, 0x10);
    assert!(taker.prepare_step(&taker_id, AuthorizeLezTag14).is_err());
    assert!(
        taker
            .select_branch(&taker_id, XmrWorkflowBranch::Claim)
            .is_err()
    );
    taker
        .prepare_step(&taker_id, InitializeLezTag13)
        .expect("first Taker common step prepares before branch selection");
    assert!(taker.prepare_step(&taker_id, FundLezTag13).is_err());
    assert!(
        taker
            .select_branch(&taker_id, XmrWorkflowBranch::Claim)
            .is_err()
    );
    complete_prepared(
        &mut taker,
        &taker_id,
        InitializeLezTag13,
        &evidence(0x31, XmrWorkflowReconciliationSource::LezFinalizedEvent),
    );
    taker
        .prepare_step(&taker_id, FundLezTag13)
        .expect("second Taker common step follows init tag 13");
    taker
        .select_branch(&taker_id, XmrWorkflowBranch::Claim)
        .expect("all role-local common steps were prepared before branch selection");
    assert!(taker.prepare_step(&taker_id, AuthorizeLezTag14).is_err());
    complete_prepared(
        &mut taker,
        &taker_id,
        FundLezTag13,
        &evidence(0x32, XmrWorkflowReconciliationSource::LezFinalizedEvent),
    );
    taker
        .prepare_step(&taker_id, AuthorizeLezTag14)
        .expect("claim tag 14 follows durable common funding");
    assert!(taker.prepare_step(&taker_id, SweepMoneroClaim).is_err());
    complete_prepared(
        &mut taker,
        &taker_id,
        AuthorizeLezTag14,
        &evidence(0x33, XmrWorkflowReconciliationSource::LezFinalizedEvent),
    );
    taker
        .prepare_step(&taker_id, SweepMoneroClaim)
        .expect("Taker claim sweep follows role-local tag-14 evidence");
    assert!(taker.prepare_step(&taker_id, RefundLezTag16).is_err());
    assert!(taker.prepare_step(&taker_id, ClaimLezTag15).is_err());

    let (mut taker_refund, taker_refund_id) = create(root.path(), Participant::Taker, 0x20);
    taker_refund
        .prepare_step(&taker_refund_id, InitializeLezTag13)
        .unwrap();
    complete_prepared(
        &mut taker_refund,
        &taker_refund_id,
        InitializeLezTag13,
        &evidence(0x41, XmrWorkflowReconciliationSource::LezFinalizedEvent),
    );
    taker_refund
        .prepare_step(&taker_refund_id, FundLezTag13)
        .unwrap();
    taker_refund
        .select_branch(&taker_refund_id, XmrWorkflowBranch::Refund)
        .unwrap();
    complete_prepared(
        &mut taker_refund,
        &taker_refund_id,
        FundLezTag13,
        &evidence(0x42, XmrWorkflowReconciliationSource::LezFinalizedEvent),
    );
    taker_refund
        .prepare_step(&taker_refund_id, RefundLezTag16)
        .expect("Taker refund tag 16 is the first role-local refund step");
    assert!(
        taker_refund
            .prepare_step(&taker_refund_id, SweepMoneroClaim)
            .is_err()
    );

    let (mut maker_claim, maker_claim_id) = create(root.path(), Participant::Maker, 0x30);
    assert!(
        maker_claim
            .select_branch(&maker_claim_id, XmrWorkflowBranch::Claim)
            .is_err()
    );
    maker_claim
        .prepare_step(&maker_claim_id, FundMonero)
        .expect("Maker Monero funding prepares before branch selection");
    maker_claim
        .select_branch(&maker_claim_id, XmrWorkflowBranch::Claim)
        .unwrap();
    assert!(
        maker_claim
            .prepare_step(&maker_claim_id, ClaimLezTag15)
            .is_err()
    );
    complete_prepared(
        &mut maker_claim,
        &maker_claim_id,
        FundMonero,
        &evidence(
            0x51,
            XmrWorkflowReconciliationSource::MoneroWalletTransaction,
        ),
    );
    maker_claim
        .prepare_step(&maker_claim_id, ClaimLezTag15)
        .expect("Maker claim tag 15 follows role-local Monero funding");
    assert!(
        maker_claim
            .prepare_step(&maker_claim_id, AuthorizeLezTag14)
            .is_err()
    );

    let (mut maker_refund, maker_refund_id) = create(root.path(), Participant::Maker, 0x40);
    maker_refund
        .prepare_step(&maker_refund_id, FundMonero)
        .unwrap();
    maker_refund
        .select_branch(&maker_refund_id, XmrWorkflowBranch::Refund)
        .unwrap();
    complete_prepared(
        &mut maker_refund,
        &maker_refund_id,
        FundMonero,
        &evidence(
            0x61,
            XmrWorkflowReconciliationSource::MoneroWalletTransaction,
        ),
    );
    maker_refund
        .prepare_step(&maker_refund_id, SweepMoneroRefund)
        .expect("Maker refund sweep follows role-local Monero funding");
    assert!(
        maker_refund
            .prepare_step(&maker_refund_id, ClaimLezTag15)
            .is_err()
    );
}

#[test]
fn started_and_unknown_reconcile_once_with_exact_persisted_v2_evidence() {
    let root = private_root();
    for (byte, make_unknown) in [(0x51, false), (0x61, true)] {
        let (mut journal, identity) = create(root.path(), Participant::Taker, byte);
        let step = XmrWorkflowStep::InitializeLezTag13;
        journal.prepare_step(&identity, step).unwrap();
        assert_eq!(
            journal.authorize_once(&identity, step).unwrap(),
            XmrWorkflowDecision::InvokeOnce
        );
        if make_unknown {
            journal.mark_unknown(&identity, step).unwrap();
        } else {
            assert!(
                journal.mark_succeeded(&identity, step).is_err(),
                "schema v2 forbids evidence-free success"
            );
        }

        assert!(
            XmrWorkflowReconciliationV2::new(
                [0; 32],
                [0x71; 32],
                XmrWorkflowReconciliationSource::LezFinalizedEvent,
            )
            .is_err()
        );
        assert!(
            XmrWorkflowReconciliationV2::new(
                [0x70; 32],
                [0; 32],
                XmrWorkflowReconciliationSource::LezFinalizedEvent,
            )
            .is_err()
        );

        let exact = evidence(
            byte.wrapping_add(0x10),
            XmrWorkflowReconciliationSource::LezFinalizedEvent,
        );
        journal
            .reconcile_succeeded(&identity, step, &exact)
            .expect("Started or Unknown reconciles with durable external evidence");
        journal
            .reconcile_succeeded(&identity, step, &exact)
            .expect("exact reconciliation replay is idempotent");
        assert_eq!(
            journal
                .load_reconciliation(&identity, step)
                .expect("read exact reconciliation"),
            Some(exact.clone())
        );

        for drifted in [
            XmrWorkflowReconciliationV2::new(
                [0x81; 32],
                exact.tool_plan_identity_sha256(),
                exact.source(),
            )
            .unwrap(),
            XmrWorkflowReconciliationV2::new(
                exact.effect_evidence_sha256(),
                [0x82; 32],
                exact.source(),
            )
            .unwrap(),
            XmrWorkflowReconciliationV2::new(
                exact.effect_evidence_sha256(),
                exact.tool_plan_identity_sha256(),
                XmrWorkflowReconciliationSource::MoneroWalletTransaction,
            )
            .unwrap(),
        ] {
            assert!(
                journal
                    .reconcile_succeeded(&identity, step, &drifted)
                    .is_err(),
                "evidence, tool-plan, or source drift must fail closed"
            );
        }
        assert_eq!(
            journal.authorize_once(&identity, step).unwrap(),
            XmrWorkflowDecision::Complete,
            "reconciliation never restores invocation authority"
        );

        let path = root.path().join(format!("workflow-{byte:02x}.sqlite3"));
        drop(journal);
        let mut reopened =
            SqliteXmrWorkflowJournal::open_existing(path).expect("reopen schema-v2 workflow");
        assert_eq!(
            reopened.authorize_once(&identity, step).unwrap(),
            XmrWorkflowDecision::Complete
        );
        assert_eq!(
            reopened.load_reconciliation(&identity, step).unwrap(),
            Some(exact)
        );
    }
}

#[test]
fn schema_v2_rejects_legacy_headers_and_any_exact_schema_tampering() {
    let root = private_root();
    let legacy = root.path().join("legacy-v1.sqlite3");
    drop(
        OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&legacy)
            .unwrap(),
    );
    let connection = Connection::open(&legacy).unwrap();
    connection
        .execute_batch(
            "
            PRAGMA application_id = 1280857938;
            PRAGMA user_version = 1;
            CREATE TABLE xmr_workflow_identity (singleton_id INTEGER);
            CREATE TABLE xmr_workflow_steps (step TEXT);
            ",
        )
        .unwrap();
    drop(connection);
    assert!(SqliteXmrWorkflowJournal::open_existing(&legacy).is_err());

    let tampered = root.path().join("tampered-v2.sqlite3");
    drop(SqliteXmrWorkflowJournal::create_new(&tampered).unwrap());
    let connection = Connection::open(&tampered).unwrap();
    connection
        .execute_batch("ALTER TABLE xmr_workflow_steps ADD COLUMN injected TEXT;")
        .unwrap();
    drop(connection);
    assert!(SqliteXmrWorkflowJournal::open_existing(&tampered).is_err());

    let crossed = root.path().join("workflow-70.sqlite3");
    let (mut journal, identity) = create(root.path(), Participant::Maker, 0x70);
    journal
        .prepare_step(&identity, XmrWorkflowStep::FundMonero)
        .unwrap();
    drop(journal);
    let connection = Connection::open(&crossed).unwrap();
    connection
        .execute(
            "UPDATE xmr_workflow_steps SET local_role = 'taker'
             WHERE step = 'fund_monero'",
            [],
        )
        .unwrap();
    drop(connection);
    assert!(
        SqliteXmrWorkflowJournal::open_existing(&crossed).is_err(),
        "a shape-valid but role-crossed durable row must fail on open"
    );
}
