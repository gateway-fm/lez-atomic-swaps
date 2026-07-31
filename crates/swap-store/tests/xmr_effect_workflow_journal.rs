use std::{fs, os::unix::fs::PermissionsExt as _};

use lez_swap_core::{Participant, SwapId};
use lez_swap_store::{
    SqliteXmrWorkflowJournal, XmrWorkflowBranch, XmrWorkflowDecision, XmrWorkflowIdentityV1,
    XmrWorkflowStep,
};
use tempfile::tempdir;

#[test]
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
        .select_branch(&identity, XmrWorkflowBranch::Claim)
        .expect("claim wins the branch CAS");
    journal
        .prepare_step(&identity, XmrWorkflowStep::SubmitLezClaimTag15)
        .expect("prepare the role-correct claim step");
    assert_eq!(
        journal
            .authorize_once(&identity, XmrWorkflowStep::SubmitLezClaimTag15)
            .expect("consume the only invocation authority"),
        XmrWorkflowDecision::InvokeOnce
    );
    drop(journal);

    let mut journal =
        SqliteXmrWorkflowJournal::open_existing(&database).expect("reopen started workflow");
    assert_eq!(
        journal
            .authorize_once(&identity, XmrWorkflowStep::SubmitLezClaimTag15)
            .expect("started step becomes observation-only"),
        XmrWorkflowDecision::ObserveOnly
    );
    journal
        .mark_unknown(&identity, XmrWorkflowStep::SubmitLezClaimTag15)
        .expect("record ambiguous process or transport outcome");
    drop(journal);

    let mut journal =
        SqliteXmrWorkflowJournal::open_existing(&database).expect("reopen unknown workflow");
    assert_eq!(
        journal
            .authorize_once(&identity, XmrWorkflowStep::SubmitLezClaimTag15)
            .expect("unknown step remains observation-only"),
        XmrWorkflowDecision::ObserveOnly
    );
    assert!(
        journal
            .select_branch(&identity, XmrWorkflowBranch::Refund)
            .is_err(),
        "the losing refund branch must not replace the durable claim branch"
    );
    assert!(
        journal
            .prepare_step(&identity, XmrWorkflowStep::SubmitLezRefundTag16)
            .is_err(),
        "Maker authority must not prepare the Taker-only tag-16 refund step"
    );
}
