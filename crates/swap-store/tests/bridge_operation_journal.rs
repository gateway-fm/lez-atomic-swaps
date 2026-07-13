use lez_bridge_protocol::{DiscoveryWindow, RequestId, RunId};
use lez_swap_core::{Participant, SwapId};
use lez_swap_store::{
    BridgeObservationOutcome, BridgeOperationKey, BridgeOperationKind, BridgeRequestSpec,
    SqliteBridgeOperationJournal, StoreError,
};
use rusqlite::Connection;
use tempfile::tempdir;

fn key(
    run: &str,
    swap: &str,
    role: Participant,
    operation: BridgeOperationKind,
) -> BridgeOperationKey {
    BridgeOperationKey::new(
        RunId::new(run).expect("run id"),
        SwapId::new(swap).expect("swap id"),
        role,
        operation,
    )
}

fn request(request_id: &str, window: Option<DiscoveryWindow>) -> BridgeRequestSpec {
    BridgeRequestSpec::new(RequestId::new(request_id).expect("request id"), window)
}

fn window(start_height: u64) -> DiscoveryWindow {
    DiscoveryWindow::new(start_height, 16).expect("bounded discovery window")
}

#[test]
fn ambiguous_prepare_and_submit_resume_exact_caller_context_after_restart() {
    let directory = tempdir().expect("temporary directory");
    let path = directory.path().join("bridge-context.sqlite");
    let prepare_key = key(
        "run-prepare-0001",
        "swap-prepare-0001",
        Participant::Maker,
        BridgeOperationKind::NativeEscrowPrepare,
    );
    let submit_key = key(
        "run-submit-0001",
        "swap-submit-0001",
        Participant::Taker,
        BridgeOperationKind::RevealingClaimSubmit,
    );
    let prepare_request = request("prepare-caller-0001", None);
    let submit_request = request("submit-caller-0001", None);

    let prepare_context = {
        let mut journal = SqliteBridgeOperationJournal::open(&path).expect("open journal");
        let commit = journal
            .begin_or_resume(&prepare_key, &prepare_request)
            .expect("persist prepare request");
        assert!(!commit.was_replay());
        assert_eq!(commit.context().poll_sequence(), 0);
        assert_eq!(commit.context().request_id(), prepare_request.request_id());
        assert_eq!(commit.context().discovery_window(), None);
        commit.context().clone()
    };

    let mut journal = SqliteBridgeOperationJournal::open(&path).expect("reopen journal");
    assert_eq!(
        journal
            .resume_after_ambiguous(&prepare_key, &prepare_context)
            .expect("resume exact ambiguous prepare"),
        prepare_context
    );
    assert!(
        journal
            .begin_or_resume(&prepare_key, &prepare_request)
            .expect("exact prepare replay")
            .was_replay()
    );
    assert!(matches!(
        journal.begin_or_resume(&prepare_key, &request("prepare-caller-0002", None)),
        Err(StoreError::BridgeOperationContextConflict)
    ));

    let submit_context = journal
        .begin_or_resume(&submit_key, &submit_request)
        .expect("persist submit request")
        .context()
        .clone();
    drop(journal);
    let journal = SqliteBridgeOperationJournal::open(&path).expect("reopen journal again");
    assert_eq!(
        journal
            .resume_after_ambiguous(&submit_key, &submit_context)
            .expect("resume exact ambiguous submit"),
        submit_context
    );
}

#[test]
fn native_escrow_initialize_and_fund_submissions_have_distinct_contexts() {
    let directory = tempdir().expect("temporary directory");
    let path = directory.path().join("bridge-context.sqlite");
    let initialize_key = key(
        "run-escrow-submit",
        "swap-escrow-submit",
        Participant::Taker,
        BridgeOperationKind::NativeEscrowInitializeSubmit,
    );
    let fund_key = key(
        "run-escrow-submit",
        "swap-escrow-submit",
        Participant::Taker,
        BridgeOperationKind::NativeEscrowFundSubmit,
    );
    let initialize_request = request("initialize-submit-0001", None);
    let fund_request = request("fund-submit-0001", None);
    let mut journal = SqliteBridgeOperationJournal::open(&path).expect("open journal");

    let initialize = journal
        .begin_or_resume(&initialize_key, &initialize_request)
        .expect("persist initialize submission")
        .context()
        .clone();
    assert!(matches!(
        journal.begin_or_resume(&fund_key, &initialize_request),
        Err(StoreError::BridgeRequestIdReused)
    ));
    let fund = journal
        .begin_or_resume(&fund_key, &fund_request)
        .expect("persist independent fund submission")
        .context()
        .clone();

    assert_eq!(
        journal.current(&initialize_key).expect("initialize row"),
        Some(initialize.clone())
    );
    assert_eq!(
        journal.current(&fund_key).expect("fund row"),
        Some(fund.clone())
    );
    assert!(matches!(
        journal.resume_after_ambiguous(&initialize_key, &fund),
        Err(StoreError::BridgeOperationContextConflict)
    ));
    assert!(matches!(
        journal.resume_after_ambiguous(&fund_key, &initialize),
        Err(StoreError::BridgeOperationContextConflict)
    ));
}

#[test]
fn native_refund_eligibility_and_exact_observation_coexist_for_one_owner() {
    let directory = tempdir().expect("temporary directory");
    let path = directory.path().join("bridge-context.sqlite");
    let eligibility_key = key(
        "run-refund-owner",
        "swap-refund-owner",
        Participant::Maker,
        BridgeOperationKind::NativeRefundEligibilityObserve,
    );
    let exact_key = key(
        "run-refund-owner",
        "swap-refund-owner",
        Participant::Maker,
        BridgeOperationKind::NativeRefundExactObserve,
    );
    let eligibility_request = request("refund-eligibility-0001", None);
    let exact_request = request("refund-exact-observe-0001", Some(window(64)));
    let mut journal = SqliteBridgeOperationJournal::open(&path).expect("open journal");

    let eligibility = journal
        .begin_or_resume(&eligibility_key, &eligibility_request)
        .expect("persist eligibility observation")
        .context()
        .clone();
    let exact = journal
        .begin_or_resume(&exact_key, &exact_request)
        .expect("persist later exact observation")
        .context()
        .clone();

    assert_eq!(
        journal.current(&eligibility_key).expect("eligibility row"),
        Some(eligibility.clone())
    );
    assert_eq!(
        journal.current(&exact_key).expect("exact row"),
        Some(exact.clone())
    );
    assert!(matches!(
        journal.resume_after_ambiguous(&eligibility_key, &exact),
        Err(StoreError::BridgeOperationContextConflict)
    ));
    assert!(matches!(
        journal.resume_after_ambiguous(&exact_key, &eligibility),
        Err(StoreError::BridgeOperationContextConflict)
    ));
}

#[test]
fn every_logical_operation_rejects_an_unusable_request_window_shape() {
    let directory = tempdir().expect("temporary directory");
    let path = directory.path().join("bridge-context.sqlite");
    let mut journal = SqliteBridgeOperationJournal::open(&path).expect("open journal");
    let bounded = window(320);
    let invalid_shapes = [
        (BridgeOperationKind::NativeEscrowPrepare, Some(bounded)),
        (BridgeOperationKind::NativeEscrowExactObserve, Some(bounded)),
        (BridgeOperationKind::NativeEscrowDiscoveryObserve, None),
        (
            BridgeOperationKind::NativeEscrowInitializeSubmit,
            Some(bounded),
        ),
        (BridgeOperationKind::NativeEscrowFundSubmit, Some(bounded)),
        (BridgeOperationKind::RevealingClaimPrepare, Some(bounded)),
        (
            BridgeOperationKind::RevealingClaimExactObserve,
            Some(bounded),
        ),
        (BridgeOperationKind::RevealingClaimDiscoveryObserve, None),
        (BridgeOperationKind::RevealingClaimSubmit, Some(bounded)),
        (BridgeOperationKind::NativeRefundPrepare, Some(bounded)),
        (
            BridgeOperationKind::NativeRefundEligibilityObserve,
            Some(bounded),
        ),
        (BridgeOperationKind::NativeRefundExactObserve, None),
        (BridgeOperationKind::NativeRefundDiscoveryObserve, None),
        (BridgeOperationKind::NativeRefundSubmit, Some(bounded)),
    ];

    for (index, (operation, discovery_window)) in invalid_shapes.into_iter().enumerate() {
        let operation_key = key(
            "run-invalid-shape",
            &format!("swap-invalid-shape-{index}"),
            Participant::Maker,
            operation,
        );
        let caller_request = request(&format!("invalid-shape-{index:02}"), discovery_window);
        assert!(matches!(
            journal.begin_or_resume(&operation_key, &caller_request),
            Err(StoreError::InvalidBridgeOperationContext)
        ));
        assert_eq!(
            journal.current(&operation_key).expect("query invalid key"),
            None
        );
    }
}

#[test]
fn exact_and_discovery_observations_use_distinct_logical_keys() {
    let directory = tempdir().expect("temporary directory");
    let path = directory.path().join("bridge-context.sqlite");
    let mut journal = SqliteBridgeOperationJournal::open(&path).expect("open journal");
    let contexts = [
        (
            key(
                "run-observe-kinds",
                "swap-observe-kinds",
                Participant::Maker,
                BridgeOperationKind::NativeEscrowExactObserve,
            ),
            request("escrow-exact-0001", None),
        ),
        (
            key(
                "run-observe-kinds",
                "swap-observe-kinds",
                Participant::Maker,
                BridgeOperationKind::NativeEscrowDiscoveryObserve,
            ),
            request("escrow-discovery-0001", Some(window(80))),
        ),
        (
            key(
                "run-observe-kinds",
                "swap-observe-kinds",
                Participant::Maker,
                BridgeOperationKind::RevealingClaimExactObserve,
            ),
            request("claim-exact-0001", None),
        ),
        (
            key(
                "run-observe-kinds",
                "swap-observe-kinds",
                Participant::Maker,
                BridgeOperationKind::RevealingClaimDiscoveryObserve,
            ),
            request("claim-discovery-0001", Some(window(96))),
        ),
    ];

    for (operation_key, caller_request) in &contexts {
        let commit = journal
            .begin_or_resume(operation_key, caller_request)
            .expect("persist independent logical observation");
        assert!(!commit.was_replay());
    }
    for (operation_key, caller_request) in &contexts {
        let durable = journal
            .current(operation_key)
            .expect("query logical observation")
            .expect("active logical observation");
        assert_eq!(durable.request_id(), caller_request.request_id());
        assert_eq!(
            durable.discovery_window(),
            caller_request.discovery_window()
        );
    }
}

#[test]
fn successful_and_typed_error_observations_advance_fresh_caller_polls() {
    let directory = tempdir().expect("temporary directory");
    let path = directory.path().join("bridge-context.sqlite");
    let operation_key = key(
        "run-observe-0001",
        "swap-observe-0001",
        Participant::Maker,
        BridgeOperationKind::NativeEscrowDiscoveryObserve,
    );
    let first_request = request("observe-caller-0001", Some(window(100)));
    let second_request = request("observe-caller-0002", Some(window(116)));
    let third_request = request("observe-caller-0003", Some(window(132)));

    let (first_context, second_context) = {
        let mut journal = SqliteBridgeOperationJournal::open(&path).expect("open journal");
        let first = journal
            .begin_or_resume(&operation_key, &first_request)
            .expect("persist first poll")
            .context()
            .clone();
        let second = journal
            .advance_observation(
                &operation_key,
                &first,
                BridgeObservationOutcome::Succeeded,
                &second_request,
            )
            .expect("advance successful observation");
        assert!(!second.was_replay());
        assert_eq!(second.context().poll_sequence(), 1);
        assert_eq!(second.context().request_id(), second_request.request_id());
        assert_eq!(
            second.context().discovery_window(),
            second_request.discovery_window()
        );
        (first, second.context().clone())
    };

    let mut journal = SqliteBridgeOperationJournal::open(&path).expect("reopen journal");
    assert_eq!(
        journal.current(&operation_key).expect("current context"),
        Some(second_context.clone())
    );
    let third = journal
        .advance_observation(
            &operation_key,
            &second_context,
            BridgeObservationOutcome::TypedError,
            &third_request,
        )
        .expect("advance typed-error observation");
    assert!(!third.was_replay());
    assert_eq!(third.context().poll_sequence(), 2);
    let replay = journal
        .advance_observation(
            &operation_key,
            &second_context,
            BridgeObservationOutcome::TypedError,
            &third_request,
        )
        .expect("recover unknown successful journal commit");
    assert!(replay.was_replay());
    assert_eq!(replay.context(), third.context());
    assert_ne!(first_context.request_id(), second_context.request_id());
}

#[test]
fn journal_keys_isolate_run_role_swap_and_operation() {
    let directory = tempdir().expect("temporary directory");
    let path = directory.path().join("bridge-context.sqlite");
    let mut journal = SqliteBridgeOperationJournal::open(&path).expect("open journal");
    let keys_and_requests = [
        (
            key(
                "run-isolate-0001",
                "swap-isolate-0001",
                Participant::Maker,
                BridgeOperationKind::NativeEscrowDiscoveryObserve,
            ),
            request("caller-run-a-maker", Some(window(10))),
        ),
        (
            key(
                "run-isolate-0002",
                "swap-isolate-0001",
                Participant::Maker,
                BridgeOperationKind::NativeEscrowDiscoveryObserve,
            ),
            request("caller-run-b-maker", Some(window(20))),
        ),
        (
            key(
                "run-isolate-0001",
                "swap-isolate-0001",
                Participant::Taker,
                BridgeOperationKind::NativeEscrowDiscoveryObserve,
            ),
            request("caller-run-a-taker", Some(window(30))),
        ),
        (
            key(
                "run-isolate-0001",
                "swap-isolate-0002",
                Participant::Maker,
                BridgeOperationKind::NativeEscrowDiscoveryObserve,
            ),
            request("caller-swap-b-maker", Some(window(40))),
        ),
        (
            key(
                "run-isolate-0001",
                "swap-isolate-0001",
                Participant::Maker,
                BridgeOperationKind::NativeRefundDiscoveryObserve,
            ),
            request("caller-refund-maker", Some(window(50))),
        ),
    ];

    for (operation_key, caller_request) in &keys_and_requests {
        let context = journal
            .begin_or_resume(operation_key, caller_request)
            .expect("independent context")
            .context()
            .clone();
        assert_eq!(context.request_id(), caller_request.request_id());
        assert_eq!(
            context.discovery_window(),
            caller_request.discovery_window()
        );
    }
    for (operation_key, caller_request) in &keys_and_requests {
        let context = journal
            .current(operation_key)
            .expect("isolated query")
            .expect("stored context");
        assert_eq!(context.request_id(), caller_request.request_id());
    }
}

#[test]
fn observation_advance_rolls_back_old_completion_when_next_insert_fails() {
    let directory = tempdir().expect("temporary directory");
    let path = directory.path().join("bridge-context.sqlite");
    let operation_key = key(
        "run-atomic-0001",
        "swap-atomic-0001",
        Participant::Taker,
        BridgeOperationKind::RevealingClaimDiscoveryObserve,
    );
    let first_request = request("atomic-caller-0001", Some(window(200)));
    let next_request = request("atomic-caller-0002", Some(window(216)));
    let mut journal = SqliteBridgeOperationJournal::open(&path).expect("open journal");
    let first = journal
        .begin_or_resume(&operation_key, &first_request)
        .expect("persist first poll")
        .context()
        .clone();

    let control = Connection::open(&path).expect("open control connection");
    control
        .execute_batch(
            "
            CREATE TRIGGER fail_bridge_poll_insert
            BEFORE INSERT ON bridge_operation_contexts
            WHEN NEW.poll_sequence = 1
            BEGIN
                SELECT RAISE(ABORT, 'forced bridge poll insert failure');
            END;
            ",
        )
        .expect("install failure trigger");
    assert!(matches!(
        journal.advance_observation(
            &operation_key,
            &first,
            BridgeObservationOutcome::Succeeded,
            &next_request,
        ),
        Err(StoreError::Sqlite(_))
    ));
    assert_eq!(
        journal.current(&operation_key).expect("current context"),
        Some(first.clone())
    );

    control
        .execute_batch("DROP TRIGGER fail_bridge_poll_insert;")
        .expect("remove failure trigger");
    let committed = journal
        .advance_observation(
            &operation_key,
            &first,
            BridgeObservationOutcome::Succeeded,
            &next_request,
        )
        .expect("atomic retry");
    assert_eq!(committed.context().poll_sequence(), 1);
}
