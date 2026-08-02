use std::{
    fs,
    os::unix::fs::PermissionsExt as _,
    sync::{Arc, Barrier},
    thread,
};

use lez_swap_core::{Participant, SwapId};
use lez_swap_store::{
    SqliteXmrWorkflowJournal, StoreError, XmrWorkflowBranch, XmrWorkflowDecision,
    XmrWorkflowIdentityV1, XmrWorkflowReconciliationSource, XmrWorkflowReconciliationV2,
    XmrWorkflowStep,
};
use tempfile::tempdir;

const CONTENDERS: usize = 8;

fn private_root() -> tempfile::TempDir {
    let root = tempdir().expect("isolated workflow root");
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700))
        .expect("owner-private workflow root");
    root
}

fn identity() -> XmrWorkflowIdentityV1 {
    XmrWorkflowIdentityV1::new(
        SwapId::new("55".repeat(32)).expect("canonical XMR swap ID"),
        Participant::Maker,
        "m5-xmr-concurrency-run".into(),
        [0x66; 32],
        [0x77; 32],
        [0x88; 32],
    )
    .expect("valid workflow identity")
}

#[test]
fn exclusive_create_has_one_winner_and_never_reopens_existing_authority() {
    let root = private_root();
    let path = root.path().join("exclusive.sqlite3");
    let barrier = Arc::new(Barrier::new(CONTENDERS + 1));
    let joins: Vec<_> = (0..CONTENDERS)
        .map(|_| {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                match SqliteXmrWorkflowJournal::create_new(path) {
                    Ok(journal) => {
                        drop(journal);
                        true
                    }
                    Err(StoreError::XmrWorkflowDatabaseAlreadyExists) => false,
                    Err(error) => panic!("unexpected create result: {error}"),
                }
            })
        })
        .collect();
    barrier.wait();
    let winners: usize = joins
        .into_iter()
        .map(|join| usize::from(join.join().expect("creator joins")))
        .sum();
    assert_eq!(winners, 1);
    drop(SqliteXmrWorkflowJournal::open_existing(path).expect("winner journal is complete"));
}

#[test]
fn concurrent_authorization_returns_exactly_one_invoke_once() {
    let root = private_root();
    let path = root.path().join("authorize.sqlite3");
    let identity = identity();
    let mut setup = SqliteXmrWorkflowJournal::create_new(&path).expect("create workflow authority");
    setup.initialize(&identity).expect("bind identity");
    setup
        .prepare_step(&identity, XmrWorkflowStep::FundMonero)
        .expect("prepare common funding");
    assert_eq!(
        setup
            .authorize_once(&identity, XmrWorkflowStep::FundMonero)
            .expect("authorize common funding"),
        XmrWorkflowDecision::InvokeOnce
    );
    setup
        .reconcile_succeeded(
            &identity,
            XmrWorkflowStep::FundMonero,
            &XmrWorkflowReconciliationV2::new(
                [0x91; 32],
                [0x92; 32],
                XmrWorkflowReconciliationSource::MoneroWalletTransaction,
            )
            .expect("exact common evidence"),
        )
        .expect("complete common funding");
    setup
        .select_branch(&identity, XmrWorkflowBranch::Claim)
        .expect("select claim");
    setup
        .prepare_step(&identity, XmrWorkflowStep::ClaimLezTag15)
        .expect("prepare exact step");
    drop(setup);

    let barrier = Arc::new(Barrier::new(CONTENDERS + 1));
    let joins: Vec<_> = (0..CONTENDERS)
        .map(|_| {
            let path = path.clone();
            let identity = identity.clone();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                let mut journal =
                    SqliteXmrWorkflowJournal::open_existing(path).expect("open contender");
                barrier.wait();
                journal
                    .authorize_once(&identity, XmrWorkflowStep::ClaimLezTag15)
                    .expect("reconcile contender")
            })
        })
        .collect();
    barrier.wait();
    let decisions: Vec<_> = joins
        .into_iter()
        .map(|join| join.join().expect("authorizer joins"))
        .collect();
    assert_eq!(
        decisions
            .iter()
            .filter(|decision| **decision == XmrWorkflowDecision::InvokeOnce)
            .count(),
        1
    );
    assert_eq!(
        decisions
            .iter()
            .filter(|decision| **decision == XmrWorkflowDecision::ObserveOnly)
            .count(),
        CONTENDERS - 1
    );
}
