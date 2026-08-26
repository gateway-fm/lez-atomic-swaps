use std::{fs, os::unix::fs::PermissionsExt as _};

use lez_swap_core::{Participant, SwapId};
use lez_swap_store::{
    SqliteXmrWorkflowJournal, XmrWorkflowBranch, XmrWorkflowDecision, XmrWorkflowIdentityV1,
    XmrWorkflowReconciliationSource, XmrWorkflowReconciliationV2, XmrWorkflowStep,
    XmrWorkflowStepScope,
};
use tempfile::tempdir;

fn identity() -> XmrWorkflowIdentityV1 {
    XmrWorkflowIdentityV1::new(
        SwapId::new("91".repeat(32)).expect("valid swap ID"),
        Participant::Maker,
        "m7-xmr-punishment-workflow".into(),
        [0x92; 32],
        [0x93; 32],
        [0x94; 32],
    )
    .expect("valid Maker workflow identity")
}

fn reconcile(
    journal: &mut SqliteXmrWorkflowJournal,
    identity: &XmrWorkflowIdentityV1,
    step: XmrWorkflowStep,
    evidence: u8,
    plan: u8,
    source: XmrWorkflowReconciliationSource,
) {
    assert_eq!(
        journal.authorize_once(identity, step).unwrap(),
        XmrWorkflowDecision::InvokeOnce
    );
    journal
        .reconcile_succeeded(
            identity,
            step,
            &XmrWorkflowReconciliationV2::new([evidence; 32], [plan; 32], source).unwrap(),
        )
        .unwrap();
}

fn assert_wrong_chain_reconciliation_is_rejected(
    journal: &mut SqliteXmrWorkflowJournal,
    identity: &XmrWorkflowIdentityV1,
) {
    let wrong_chain = XmrWorkflowReconciliationV2::new(
        [0xbe; 32],
        [0xbf; 32],
        XmrWorkflowReconciliationSource::MoneroWalletTransaction,
    )
    .unwrap();
    assert!(
        journal
            .reconcile_succeeded(identity, XmrWorkflowStep::PunishLezTag17, &wrong_chain)
            .is_err(),
        "Tag17 completion requires finalized LEZ evidence"
    );
}

#[test]
fn punishment_is_a_distinct_maker_branch_with_exactly_once_restart_semantics() {
    assert_eq!(XmrWorkflowStep::PunishLezTag17.role(), Participant::Maker);
    assert_eq!(
        XmrWorkflowStep::PunishLezTag17.scope(),
        XmrWorkflowStepScope::Punish
    );
    assert!(
        XmrWorkflowStep::ALL.contains(&XmrWorkflowStep::PunishLezTag17),
        "the durable catalog must include the punishment transition"
    );

    let root = tempdir().expect("isolated punishment workflow root");
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let database = root.path().join("workflow.sqlite3");
    let identity = identity();
    let mut journal = SqliteXmrWorkflowJournal::create_new(&database).unwrap();
    journal.initialize(&identity).unwrap();

    assert!(
        journal
            .select_branch(&identity, XmrWorkflowBranch::Punish)
            .is_err(),
        "punishment cannot be selected before Maker Monero funding is prepared"
    );
    journal
        .prepare_step(&identity, XmrWorkflowStep::FundMonero)
        .unwrap();
    reconcile(
        &mut journal,
        &identity,
        XmrWorkflowStep::FundMonero,
        0xa1,
        0xb1,
        XmrWorkflowReconciliationSource::MoneroWalletTransaction,
    );
    journal
        .select_branch(&identity, XmrWorkflowBranch::Punish)
        .unwrap();
    journal
        .prepare_step(&identity, XmrWorkflowStep::PunishLezTag17)
        .unwrap();

    assert!(
        journal
            .select_branch(&identity, XmrWorkflowBranch::Claim)
            .is_err(),
        "claim cannot replace the durable punishment branch"
    );
    assert!(
        journal
            .select_branch(&identity, XmrWorkflowBranch::Refund)
            .is_err(),
        "refund cannot replace the durable punishment branch"
    );
    assert!(
        journal
            .prepare_step(&identity, XmrWorkflowStep::ClaimLezTag15)
            .is_err(),
        "the losing claim step cannot enter a punishment workflow"
    );
    assert_eq!(
        journal
            .authorize_once(&identity, XmrWorkflowStep::PunishLezTag17)
            .unwrap(),
        XmrWorkflowDecision::InvokeOnce
    );
    assert_wrong_chain_reconciliation_is_rejected(&mut journal, &identity);
    drop(journal);

    let mut reopened = SqliteXmrWorkflowJournal::open_existing(&database).unwrap();
    assert_eq!(
        reopened
            .authorize_once(&identity, XmrWorkflowStep::PunishLezTag17)
            .unwrap(),
        XmrWorkflowDecision::ObserveOnly,
        "restart must never rearm an attempted punishment"
    );
    reopened
        .mark_unknown(&identity, XmrWorkflowStep::PunishLezTag17)
        .unwrap();
    let finality = XmrWorkflowReconciliationV2::new(
        [0xc1; 32],
        [0xd1; 32],
        XmrWorkflowReconciliationSource::LezFinalizedEvent,
    )
    .unwrap();
    reopened
        .reconcile_succeeded(&identity, XmrWorkflowStep::PunishLezTag17, &finality)
        .unwrap();
    assert_eq!(
        reopened
            .authorize_once(&identity, XmrWorkflowStep::PunishLezTag17)
            .unwrap(),
        XmrWorkflowDecision::Complete
    );
    assert_eq!(
        reopened
            .load_reconciliation(&identity, XmrWorkflowStep::PunishLezTag17)
            .unwrap(),
        Some(finality)
    );
}

#[test]
fn taker_cannot_prepare_the_maker_punishment_step() {
    let root = tempdir().expect("isolated crossed-role root");
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut journal = SqliteXmrWorkflowJournal::create_new(root.path().join("workflow.sqlite3"))
        .expect("create crossed-role workflow");
    let identity = XmrWorkflowIdentityV1::new(
        SwapId::new("95".repeat(32)).unwrap(),
        Participant::Taker,
        "m7-xmr-crossed-punishment".into(),
        [0x96; 32],
        [0x97; 32],
        [0x98; 32],
    )
    .unwrap();
    journal.initialize(&identity).unwrap();
    assert!(
        journal
            .prepare_step(&identity, XmrWorkflowStep::PunishLezTag17)
            .is_err()
    );
}
