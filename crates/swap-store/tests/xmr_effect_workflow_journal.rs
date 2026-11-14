use std::{fs, os::unix::fs::PermissionsExt as _};

use lez_swap_core::{Participant, SwapId};
use lez_swap_store::{
    SqliteXmrWorkflowJournal, XmrWorkflowBranch, XmrWorkflowDecision, XmrWorkflowIdentityV1,
    XmrWorkflowReconciliationSource, XmrWorkflowReconciliationV2, XmrWorkflowStep,
};
use tempfile::tempdir;

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one workflow's whole claim life, its reopen and its replacement"
)]
fn started_or_unknown_xmr_workflow_step_is_never_reauthorized_after_reopen() {
    let root = tempdir().expect("isolated XMR workflow root");
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700))
        .expect("owner-private XMR workflow root");
    let database = root.path().join("xmr-workflow.sqlite3");
    let identity = XmrWorkflowIdentityV1::new(
        SwapId::new("11".repeat(32)).expect("valid swap ID"),
        Participant::Maker,
        "m5-xmr-workflow-run-001".into(),
        [0x22; 32],
        [0x33; 32],
        [0x44; 32],
    )
    .expect("valid workflow identity");

    let mut journal =
        SqliteXmrWorkflowJournal::create_new(&database).expect("create workflow journal");
    journal
        .initialize(&identity)
        .expect("initialize exact workflow identity");
    journal
        .validate_initialized(&identity)
        .expect("read-only validation accepts exact durable identity");
    let crossed = XmrWorkflowIdentityV1::new(
        SwapId::new("11".repeat(32)).expect("valid swap ID"),
        Participant::Maker,
        "m5-xmr-workflow-run-crossed".into(),
        [0x22; 32],
        [0x33; 32],
        [0x44; 32],
    )
    .expect("valid crossed identity");
    assert!(
        journal.validate_initialized(&crossed).is_err(),
        "read-only validation must reject any durable identity drift"
    );
    journal
        .prepare_step(&identity, XmrWorkflowStep::FundMonero)
        .expect("prepare Maker common Monero funding");
    assert_eq!(
        journal
            .authorize_once(&identity, XmrWorkflowStep::FundMonero)
            .expect("authorize common funding"),
        XmrWorkflowDecision::InvokeOnce
    );
    journal
        .reconcile_succeeded(
            &identity,
            XmrWorkflowStep::FundMonero,
            &XmrWorkflowReconciliationV2::new(
                [0x55; 32],
                [0x56; 32],
                XmrWorkflowReconciliationSource::MoneroWalletTransaction,
            )
            .expect("exact common funding evidence"),
        )
        .expect("complete common funding");
    journal
        .select_branch(&identity, XmrWorkflowBranch::Claim)
        .expect("claim wins the branch CAS");
    journal
        .prepare_step(&identity, XmrWorkflowStep::ClaimLezTag15)
        .expect("prepare the role-correct claim step");
    assert_eq!(
        journal
            .authorize_once(&identity, XmrWorkflowStep::ClaimLezTag15)
            .expect("consume the only invocation authority"),
        XmrWorkflowDecision::InvokeOnce
    );
    drop(journal);

    let mut journal =
        SqliteXmrWorkflowJournal::open_existing(&database).expect("reopen started workflow");
    assert_eq!(
        journal
            .authorize_once(&identity, XmrWorkflowStep::ClaimLezTag15)
            .expect("started step becomes observation-only"),
        XmrWorkflowDecision::ObserveOnly
    );
    journal
        .mark_unknown(&identity, XmrWorkflowStep::ClaimLezTag15)
        .expect("record ambiguous process or transport outcome");
    drop(journal);

    let mut journal =
        SqliteXmrWorkflowJournal::open_existing(&database).expect("reopen unknown workflow");
    assert_eq!(
        journal
            .authorize_once(&identity, XmrWorkflowStep::ClaimLezTag15)
            .expect("unknown step remains observation-only"),
        XmrWorkflowDecision::ObserveOnly
    );
    assert!(
        journal
            .prepare_step(&identity, XmrWorkflowStep::RefundLezTag16)
            .is_err(),
        "Maker authority must not prepare the Taker-only tag-16 refund step"
    );
    // The chain proved the Taker's refund: the unreconciled claim gives way,
    // once, and nothing brings it back.
    journal
        .select_branch(&identity, XmrWorkflowBranch::Refund)
        .expect("an unreconciled Maker claim gives way to the proved refund");
    assert_eq!(
        journal.selected_branch(&identity).unwrap(),
        Some(XmrWorkflowBranch::Refund)
    );
    journal
        .prepare_step(&identity, XmrWorkflowStep::SweepMoneroRefund)
        .expect("the refund sweep follows the funding");
    for branch in [XmrWorkflowBranch::Claim, XmrWorkflowBranch::Punish] {
        assert!(
            journal.select_branch(&identity, branch).is_err(),
            "the refund branch never gives way"
        );
    }
    assert!(
        journal
            .authorize_once(&identity, XmrWorkflowStep::ClaimLezTag15)
            .is_err(),
        "the replaced claim step is out of the workflow's scope"
    );
    drop(journal);
    SqliteXmrWorkflowJournal::open_existing(&database)
        .expect("a replaced branch reopens")
        .validate_initialized(&identity)
        .expect("a replaced branch validates");
}

#[test]
fn a_reconciled_maker_claim_and_a_taker_claim_never_give_way() {
    let root = tempdir().expect("isolated XMR workflow root");
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    for (role, step) in [
        (Participant::Maker, XmrWorkflowStep::ClaimLezTag15),
        (Participant::Taker, XmrWorkflowStep::AuthorizeLezTag14),
    ] {
        let identity = XmrWorkflowIdentityV1::new(
            SwapId::new("12".repeat(32)).unwrap(),
            role,
            "m5-xmr-workflow-run-002".into(),
            [0x22; 32],
            [0x33; 32],
            [0x44; 32],
        )
        .unwrap();
        let database = root.path().join(format!("{role:?}.sqlite3"));
        let mut journal = SqliteXmrWorkflowJournal::create_new(&database).unwrap();
        journal.initialize(&identity).unwrap();
        let common: &[XmrWorkflowStep] = match role {
            Participant::Maker => &[XmrWorkflowStep::FundMonero],
            Participant::Taker => &[
                XmrWorkflowStep::InitializeLezTag13,
                XmrWorkflowStep::FundLezTag13,
            ],
        };
        for (index, common) in common.iter().enumerate() {
            journal.prepare_step(&identity, *common).unwrap();
            let _ = journal.authorize_once(&identity, *common).unwrap();
            let source = if *common == XmrWorkflowStep::FundMonero {
                XmrWorkflowReconciliationSource::MoneroWalletTransaction
            } else {
                XmrWorkflowReconciliationSource::LezFinalizedEvent
            };
            let evidence = u8::try_from(index).unwrap() + 0x55;
            journal
                .reconcile_succeeded(
                    &identity,
                    *common,
                    &XmrWorkflowReconciliationV2::new([evidence; 32], [0x56; 32], source).unwrap(),
                )
                .unwrap();
        }
        journal
            .select_branch(&identity, XmrWorkflowBranch::Claim)
            .unwrap();
        journal.prepare_step(&identity, step).unwrap();
        let _ = journal.authorize_once(&identity, step).unwrap();
        if role == Participant::Maker {
            journal
                .reconcile_succeeded(
                    &identity,
                    step,
                    &XmrWorkflowReconciliationV2::new(
                        [0x57; 32],
                        [0x58; 32],
                        XmrWorkflowReconciliationSource::LezFinalizedEvent,
                    )
                    .unwrap(),
                )
                .unwrap();
        }
        for branch in [XmrWorkflowBranch::Refund, XmrWorkflowBranch::Punish] {
            assert!(
                journal.select_branch(&identity, branch).is_err(),
                "{role:?}: the claim branch keeps its place"
            );
        }
        assert_eq!(
            journal.selected_branch(&identity).unwrap(),
            Some(XmrWorkflowBranch::Claim)
        );
    }
}
