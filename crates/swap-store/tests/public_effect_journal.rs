use std::thread;

use lez_swap_core::{Participant, SwapId};
use lez_swap_store::{
    PreparedPublicEffect, PublicEffectChain, PublicEffectDecision, PublicEffectKey,
    PublicEffectObservation, PublicEffectOperation, PublicEffectState,
    PublicEffectSubmissionResult, SqlitePublicEffectJournal, StoreError,
};
use rusqlite::Connection;
use sha2::{Digest as _, Sha256};
use tempfile::tempdir;

fn key() -> PublicEffectKey {
    PublicEffectKey::new(
        SwapId::new("m3-public-effect").unwrap(),
        Participant::Maker,
        PublicEffectChain::Bitcoin,
        PublicEffectOperation::Claim,
        2,
    )
}

fn prepared() -> PreparedPublicEffect {
    PreparedPublicEffect::new(
        key(),
        [0xa1; 32],
        "btc-claim-transaction-0001",
        vec![0x01, 0x02, 0x03, 0x04],
    )
    .unwrap()
}

fn prepared_refund() -> PreparedPublicEffect {
    PreparedPublicEffect::new(
        PublicEffectKey::new(
            SwapId::new("m3-public-refund-effect").unwrap(),
            Participant::Maker,
            PublicEffectChain::Lez,
            PublicEffectOperation::Refund,
            2,
        ),
        [0xb1; 32],
        "lez-refund-transaction-0001",
        vec![0x11, 0x12, 0x13, 0x14],
    )
    .unwrap()
}

fn prepared_lez_claim() -> PreparedPublicEffect {
    PreparedPublicEffect::new(
        PublicEffectKey::new(
            SwapId::new("m5-lez-claim-effect").unwrap(),
            Participant::Maker,
            PublicEffectChain::Lez,
            PublicEffectOperation::Claim,
            2,
        ),
        [0xc1; 32],
        "lez-claim-transaction-0001",
        vec![0x21, 0x22, 0x23, 0x24],
    )
    .unwrap()
}

fn exact_idempotent_lez_claim(effect: &PreparedPublicEffect) -> PublicEffectObservation {
    PublicEffectObservation::ExactIdempotentLezClaimSubmissionSafe {
        expected_effect_id: effect.expected_effect_id().to_owned().into_boxed_str(),
        exact_public_bytes: effect.exact_public_bytes().to_vec(),
    }
}

#[test]
fn prepared_effect_is_exactly_replayable_and_conflicts_on_any_immutable_drift() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("effects.sqlite3");
    let mut journal = SqlitePublicEffectJournal::open(&path).unwrap();
    let candidate = prepared();

    assert!(!journal.record_prepared(&candidate).unwrap().was_replay());
    assert!(journal.record_prepared(&candidate).unwrap().was_replay());
    let durable = journal.current(candidate.key()).unwrap().unwrap();
    assert_eq!(durable.state(), PublicEffectState::Prepared);
    assert_eq!(durable.attempt_count(), 0);
    assert_eq!(durable.revision(), 0);
    assert_eq!(durable.effect(), &candidate);
    assert_eq!(
        candidate.public_bytes_sha256(),
        <[u8; 32]>::from(Sha256::digest(candidate.exact_public_bytes()))
    );

    for changed in [
        PreparedPublicEffect::new(
            key(),
            [0xa2; 32],
            candidate.expected_effect_id(),
            candidate.exact_public_bytes().to_vec(),
        )
        .unwrap(),
        PreparedPublicEffect::new(
            key(),
            [0xa1; 32],
            "different-effect-id",
            candidate.exact_public_bytes().to_vec(),
        )
        .unwrap(),
        PreparedPublicEffect::new(
            key(),
            [0xa1; 32],
            candidate.expected_effect_id(),
            vec![0x01, 0x02, 0x03, 0xff],
        )
        .unwrap(),
    ] {
        assert!(matches!(
            journal.record_prepared(&changed),
            Err(StoreError::PublicEffectConflict)
        ));
    }
}

#[test]
fn absent_observation_commits_started_before_granting_the_only_send_authorization() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("effects.sqlite3");
    let candidate = prepared();
    let mut journal = SqlitePublicEffectJournal::open(&path).unwrap();
    let _ = journal.record_prepared(&candidate).unwrap();

    let decision = journal
        .reconcile(candidate.key(), PublicEffectObservation::Absent)
        .unwrap();
    let PublicEffectDecision::SubmitOnce(started) = decision else {
        panic!("prepared and absent must grant the sole send authorization");
    };
    assert_eq!(started.state(), PublicEffectState::Started);
    assert_eq!(started.attempt_count(), 1);
    assert_eq!(started.revision(), 1);

    drop(journal);
    let mut restarted = SqlitePublicEffectJournal::open(&path).unwrap();
    for observation in [
        PublicEffectObservation::Absent,
        PublicEffectObservation::Uncertain,
    ] {
        let PublicEffectDecision::ObserveOnly(durable) =
            restarted.reconcile(candidate.key(), observation).unwrap()
        else {
            panic!("started or uncertain work must never rearm submission");
        };
        assert_eq!(durable.state(), PublicEffectState::Started);
    }
}

#[test]
fn exact_idempotent_lez_claim_admission_starts_once_without_claiming_absence() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("lez-claim.sqlite3");
    let candidate = prepared_lez_claim();
    let mut journal = SqlitePublicEffectJournal::open(&path).unwrap();
    let _ = journal.record_prepared(&candidate).unwrap();

    let PublicEffectDecision::SubmitOnce(started) = journal
        .reconcile(candidate.key(), exact_idempotent_lez_claim(&candidate))
        .unwrap()
    else {
        panic!("exact idempotent LEZ claim admission must grant one durable send");
    };
    assert_eq!(started.state(), PublicEffectState::Started);
    assert_eq!(started.attempt_count(), 1);
    assert_eq!(started.revision(), 1);

    drop(journal);
    let mut restarted = SqlitePublicEffectJournal::open(&path).unwrap();
    let PublicEffectDecision::ObserveOnly(durable) = restarted
        .reconcile(candidate.key(), exact_idempotent_lez_claim(&candidate))
        .unwrap()
    else {
        panic!("consumed exact idempotent claim authority must never rearm");
    };
    assert_eq!(durable.state(), PublicEffectState::Started);
    assert_eq!(durable.attempt_count(), 1);
    assert_eq!(durable.revision(), 1);
}

#[test]
fn exact_idempotent_admission_is_lez_claim_only_and_payload_bound() {
    let directory = tempdir().unwrap();

    for (case, candidate) in [
        ("bitcoin-claim", prepared()),
        (
            "lez-funding",
            PreparedPublicEffect::new(
                PublicEffectKey::new(
                    SwapId::new("m5-lez-funding-effect").unwrap(),
                    Participant::Maker,
                    PublicEffectChain::Lez,
                    PublicEffectOperation::Funding,
                    1,
                ),
                [0xd1; 32],
                "lez-funding-transaction-0001",
                vec![0x31, 0x32],
            )
            .unwrap(),
        ),
        ("lez-refund", prepared_refund()),
    ] {
        let mut journal =
            SqlitePublicEffectJournal::open(directory.path().join(format!("{case}.sqlite3")))
                .unwrap();
        let _ = journal.record_prepared(&candidate).unwrap();
        assert!(matches!(
            journal.reconcile(candidate.key(), exact_idempotent_lez_claim(&candidate)),
            Err(StoreError::InvalidPublicEffect)
        ));
        let durable = journal.current(candidate.key()).unwrap().unwrap();
        assert_eq!(durable.state(), PublicEffectState::Prepared);
        assert_eq!(durable.attempt_count(), 0);
    }

    let candidate = prepared_lez_claim();
    for (case, observation) in [
        (
            "wrong-id",
            PublicEffectObservation::ExactIdempotentLezClaimSubmissionSafe {
                expected_effect_id: "different-lez-claim".into(),
                exact_public_bytes: candidate.exact_public_bytes().to_vec(),
            },
        ),
        (
            "wrong-bytes",
            PublicEffectObservation::ExactIdempotentLezClaimSubmissionSafe {
                expected_effect_id: candidate.expected_effect_id().to_owned().into_boxed_str(),
                exact_public_bytes: vec![0x21, 0x22, 0x23, 0xff],
            },
        ),
    ] {
        let mut journal =
            SqlitePublicEffectJournal::open(directory.path().join(format!("{case}.sqlite3")))
                .unwrap();
        let _ = journal.record_prepared(&candidate).unwrap();
        assert!(matches!(
            journal.reconcile(candidate.key(), observation),
            Err(StoreError::PublicEffectConflict)
        ));
        let durable = journal.current(candidate.key()).unwrap().unwrap();
        assert_eq!(durable.state(), PublicEffectState::Prepared);
        assert_eq!(durable.attempt_count(), 0);
    }
}

#[test]
fn only_explicit_refund_eligibility_can_authorize_the_one_attempt() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("refund-effects.sqlite3");
    let candidate = prepared_refund();
    let mut journal = SqlitePublicEffectJournal::open(&path).unwrap();
    let _ = journal.record_prepared(&candidate).unwrap();

    assert!(matches!(
        journal.reconcile(candidate.key(), PublicEffectObservation::Absent),
        Err(StoreError::InvalidPublicEffect)
    ));
    assert_eq!(
        journal.current(candidate.key()).unwrap().unwrap().state(),
        PublicEffectState::Prepared
    );
    let PublicEffectDecision::SubmitOnce(started) = journal
        .reconcile(candidate.key(), PublicEffectObservation::EligibleToAttempt)
        .unwrap()
    else {
        panic!("stable refund eligibility must grant one send authorization");
    };
    assert_eq!(started.state(), PublicEffectState::Started);
    assert_eq!(started.attempt_count(), 1);

    drop(journal);
    let mut restarted = SqlitePublicEffectJournal::open(&path).unwrap();
    for observation in [
        PublicEffectObservation::EligibleToAttempt,
        PublicEffectObservation::Uncertain,
    ] {
        let PublicEffectDecision::ObserveOnly(durable) =
            restarted.reconcile(candidate.key(), observation).unwrap()
        else {
            panic!("consumed refund authority must remain observe-only");
        };
        assert_eq!(durable.state(), PublicEffectState::Started);
        assert_eq!(durable.attempt_count(), 1);
    }

    let claim = prepared();
    let mut claim_journal =
        SqlitePublicEffectJournal::open(directory.path().join("claim.sqlite3")).unwrap();
    let _ = claim_journal.record_prepared(&claim).unwrap();
    assert!(matches!(
        claim_journal.reconcile(claim.key(), PublicEffectObservation::EligibleToAttempt,),
        Err(StoreError::InvalidPublicEffect)
    ));
}

#[test]
fn conflicting_presence_durably_burns_authority_without_authorizing_a_send() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("effects.sqlite3");
    let candidate = prepared();
    let mut journal = SqlitePublicEffectJournal::open(&path).unwrap();
    let _ = journal.record_prepared(&candidate).unwrap();

    let PublicEffectDecision::ObserveOnly(blocked) = journal
        .reconcile(
            candidate.key(),
            PublicEffectObservation::ConflictingPresence,
        )
        .unwrap()
    else {
        panic!("conflicting chain presence must never grant send authority");
    };
    assert_eq!(blocked.state(), PublicEffectState::Unknown);
    assert_eq!(blocked.attempt_count(), 1);
    assert_eq!(blocked.revision(), 2);

    drop(journal);
    let mut restarted = SqlitePublicEffectJournal::open(&path).unwrap();
    for observation in [
        PublicEffectObservation::Absent,
        PublicEffectObservation::Uncertain,
        PublicEffectObservation::ConflictingPresence,
    ] {
        let PublicEffectDecision::ObserveOnly(still_blocked) =
            restarted.reconcile(candidate.key(), observation).unwrap()
        else {
            panic!("burned authority must remain observe-only after restart");
        };
        assert_eq!(still_blocked.state(), PublicEffectState::Unknown);
        assert_eq!(still_blocked.attempt_count(), 1);
        assert_eq!(still_blocked.revision(), 2);
    }

    let PublicEffectDecision::ObserveOnly(accepted) = restarted
        .reconcile(
            candidate.key(),
            PublicEffectObservation::PresentExact(candidate.exact_public_bytes().to_vec()),
        )
        .unwrap()
    else {
        panic!("later exact presence is evidence, not send authority");
    };
    assert_eq!(accepted.state(), PublicEffectState::Accepted);
    assert_eq!(accepted.attempt_count(), 1);
    assert_eq!(accepted.revision(), 3);
}

#[test]
fn transient_uncertainty_does_not_burn_a_later_definitive_absence_decision() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("effects.sqlite3");
    let candidate = prepared();
    let mut journal = SqlitePublicEffectJournal::open(&path).unwrap();
    let _ = journal.record_prepared(&candidate).unwrap();

    let PublicEffectDecision::ObserveOnly(waiting) = journal
        .reconcile(candidate.key(), PublicEffectObservation::Uncertain)
        .unwrap()
    else {
        panic!("transient uncertainty must remain observe-only");
    };
    assert_eq!(waiting.state(), PublicEffectState::Prepared);

    assert!(matches!(
        journal
            .reconcile(candidate.key(), PublicEffectObservation::Absent)
            .unwrap(),
        PublicEffectDecision::SubmitOnce(_)
    ));
}

#[test]
fn exact_present_bytes_reconcile_prepared_started_or_unknown_to_accepted() {
    for prior_state in [
        PublicEffectState::Prepared,
        PublicEffectState::Started,
        PublicEffectState::Unknown,
    ] {
        let directory = tempdir().unwrap();
        let path = directory.path().join("effects.sqlite3");
        let candidate = prepared();
        let mut journal = SqlitePublicEffectJournal::open(&path).unwrap();
        let _ = journal.record_prepared(&candidate).unwrap();
        if prior_state != PublicEffectState::Prepared {
            assert!(matches!(
                journal
                    .reconcile(candidate.key(), PublicEffectObservation::Absent)
                    .unwrap(),
                PublicEffectDecision::SubmitOnce(_)
            ));
        }
        if prior_state == PublicEffectState::Unknown {
            let _ = journal
                .record_submission_result(candidate.key(), &PublicEffectSubmissionResult::Unknown)
                .unwrap();
        }

        let PublicEffectDecision::ObserveOnly(accepted) = journal
            .reconcile(
                candidate.key(),
                PublicEffectObservation::PresentExact(candidate.exact_public_bytes().to_vec()),
            )
            .unwrap()
        else {
            panic!("chain presence is evidence, never send authority");
        };
        assert_eq!(accepted.state(), PublicEffectState::Accepted);
    }

    let directory = tempdir().unwrap();
    let path = directory.path().join("effects.sqlite3");
    let candidate = prepared();
    let mut journal = SqlitePublicEffectJournal::open(&path).unwrap();
    let _ = journal.record_prepared(&candidate).unwrap();
    let error = journal.reconcile(
        candidate.key(),
        PublicEffectObservation::PresentExact(vec![0x01, 0x02, 0x03, 0x05]),
    );
    assert!(matches!(error, Err(StoreError::PublicEffectConflict)));
    assert_eq!(
        journal.current(candidate.key()).unwrap().unwrap().state(),
        PublicEffectState::Prepared
    );
}

#[test]
fn only_started_records_a_fresh_result_and_exact_terminal_replay_is_idempotent() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("effects.sqlite3");
    let candidate = prepared();
    let mut journal = SqlitePublicEffectJournal::open(&path).unwrap();
    let _ = journal.record_prepared(&candidate).unwrap();

    assert!(matches!(
        journal.record_submission_result(candidate.key(), &PublicEffectSubmissionResult::Rejected),
        Err(StoreError::PublicEffectConflict)
    ));
    let _ = journal
        .reconcile(candidate.key(), PublicEffectObservation::Absent)
        .unwrap();
    let committed = journal
        .record_submission_result(candidate.key(), &PublicEffectSubmissionResult::Unknown)
        .unwrap();
    assert!(!committed.was_replay());
    assert_eq!(committed.snapshot().state(), PublicEffectState::Unknown);
    let replay = journal
        .record_submission_result(candidate.key(), &PublicEffectSubmissionResult::Unknown)
        .unwrap();
    assert!(replay.was_replay());
    assert!(matches!(
        journal.record_submission_result(
            candidate.key(),
            &PublicEffectSubmissionResult::Accepted("wrong-effect-id".into())
        ),
        Err(StoreError::PublicEffectConflict)
    ));

    let PublicEffectDecision::ObserveOnly(snapshot) = journal
        .reconcile(candidate.key(), PublicEffectObservation::Absent)
        .unwrap()
    else {
        panic!("unknown must remain observe-only");
    };
    assert_eq!(snapshot.state(), PublicEffectState::Unknown);
}

#[test]
fn accepted_result_requires_and_replays_the_exact_expected_effect_id() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("effects.sqlite3");
    let candidate = prepared();
    let mut journal = SqlitePublicEffectJournal::open(&path).unwrap();
    let _ = journal.record_prepared(&candidate).unwrap();
    let _ = journal
        .reconcile(candidate.key(), PublicEffectObservation::Absent)
        .unwrap();

    assert!(matches!(
        journal.record_submission_result(
            candidate.key(),
            &PublicEffectSubmissionResult::Accepted("wrong-effect-id".into())
        ),
        Err(StoreError::PublicEffectConflict)
    ));
    assert_eq!(
        journal.current(candidate.key()).unwrap().unwrap().state(),
        PublicEffectState::Started
    );

    let accepted = PublicEffectSubmissionResult::Accepted(
        candidate.expected_effect_id().to_owned().into_boxed_str(),
    );
    let committed = journal
        .record_submission_result(candidate.key(), &accepted)
        .unwrap();
    assert_eq!(committed.snapshot().state(), PublicEffectState::Accepted);
    assert!(!committed.was_replay());
    assert!(
        journal
            .record_submission_result(candidate.key(), &accepted)
            .unwrap()
            .was_replay()
    );
}

#[test]
fn competing_processes_receive_exactly_one_submit_once_decision() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("effects.sqlite3");
    let candidate = prepared();
    let _ = SqlitePublicEffectJournal::open(&path)
        .unwrap()
        .record_prepared(&candidate)
        .unwrap();
    let journals = (0..8)
        .map(|_| SqlitePublicEffectJournal::open(&path).unwrap())
        .collect::<Vec<_>>();

    let workers = journals
        .into_iter()
        .map(|mut journal| {
            let key = candidate.key().clone();
            thread::spawn(move || {
                journal
                    .reconcile(&key, PublicEffectObservation::Absent)
                    .unwrap()
            })
        })
        .collect::<Vec<_>>();
    let decisions = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        decisions
            .iter()
            .filter(|decision| matches!(decision, PublicEffectDecision::SubmitOnce(_)))
            .count(),
        1
    );
    assert_eq!(
        decisions
            .iter()
            .filter(|decision| matches!(decision, PublicEffectDecision::ObserveOnly(_)))
            .count(),
        7
    );
}

#[test]
fn competing_refund_eligibility_observers_receive_exactly_one_attempt() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("refund-race.sqlite3");
    let candidate = prepared_refund();
    let _ = SqlitePublicEffectJournal::open(&path)
        .unwrap()
        .record_prepared(&candidate)
        .unwrap();
    let journals = (0..8)
        .map(|_| SqlitePublicEffectJournal::open(&path).unwrap())
        .collect::<Vec<_>>();

    let workers = journals
        .into_iter()
        .map(|mut journal| {
            let key = candidate.key().clone();
            thread::spawn(move || {
                journal
                    .reconcile(&key, PublicEffectObservation::EligibleToAttempt)
                    .unwrap()
            })
        })
        .collect::<Vec<_>>();
    let decisions = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        decisions
            .iter()
            .filter(|decision| matches!(decision, PublicEffectDecision::SubmitOnce(_)))
            .count(),
        1
    );
    assert_eq!(
        decisions
            .iter()
            .filter(|decision| matches!(decision, PublicEffectDecision::ObserveOnly(_)))
            .count(),
        7
    );
}

#[test]
fn composite_identity_isolates_every_authority_dimension() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("effects.sqlite3");
    let base = prepared();
    let keys = [
        base.key().clone(),
        PublicEffectKey::new(
            SwapId::new("other-swap").unwrap(),
            Participant::Maker,
            PublicEffectChain::Bitcoin,
            PublicEffectOperation::Claim,
            2,
        ),
        PublicEffectKey::new(
            base.key().swap_id().clone(),
            Participant::Taker,
            PublicEffectChain::Bitcoin,
            PublicEffectOperation::Claim,
            2,
        ),
        PublicEffectKey::new(
            base.key().swap_id().clone(),
            Participant::Maker,
            PublicEffectChain::Lez,
            PublicEffectOperation::Claim,
            2,
        ),
        PublicEffectKey::new(
            base.key().swap_id().clone(),
            Participant::Maker,
            PublicEffectChain::Bitcoin,
            PublicEffectOperation::Funding,
            2,
        ),
        PublicEffectKey::new(
            base.key().swap_id().clone(),
            Participant::Maker,
            PublicEffectChain::Bitcoin,
            PublicEffectOperation::Claim,
            3,
        ),
    ];
    let mut journal = SqlitePublicEffectJournal::open(&path).unwrap();
    for (index, key) in keys.iter().enumerate() {
        let _ = journal
            .record_prepared(
                &PreparedPublicEffect::new(
                    key.clone(),
                    [0xa1; 32],
                    format!("effect-{index}"),
                    vec![u8::try_from(index).unwrap().saturating_add(1)],
                )
                .unwrap(),
            )
            .unwrap();
    }
    for key in &keys {
        assert!(journal.current(key).unwrap().is_some());
    }
}

#[test]
fn one_chain_effect_id_cannot_be_crosswired_to_another_authority_key() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("effects.sqlite3");
    let candidate = prepared();
    let mut journal = SqlitePublicEffectJournal::open(&path).unwrap();
    let _ = journal.record_prepared(&candidate).unwrap();
    let crosswired = PreparedPublicEffect::new(
        PublicEffectKey::new(
            SwapId::new("crosswired-swap").unwrap(),
            Participant::Taker,
            candidate.key().chain(),
            PublicEffectOperation::Refund,
            9,
        ),
        [0xb2; 32],
        candidate.expected_effect_id(),
        candidate.exact_public_bytes().to_vec(),
    )
    .unwrap();

    assert!(matches!(
        journal.record_prepared(&crosswired),
        Err(StoreError::PublicEffectConflict)
    ));
}

#[test]
fn failed_started_cas_rolls_back_without_leaking_send_authority() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("effects.sqlite3");
    let candidate = prepared();
    let mut journal = SqlitePublicEffectJournal::open(&path).unwrap();
    let _ = journal.record_prepared(&candidate).unwrap();
    let control = Connection::open(&path).unwrap();
    control
        .execute_batch(
            "CREATE TRIGGER reject_public_effect_start
             BEFORE UPDATE ON public_effect_journal
             WHEN NEW.state = 'started'
             BEGIN SELECT RAISE(ABORT, 'injected start failure'); END;",
        )
        .unwrap();

    assert!(matches!(
        journal.reconcile(candidate.key(), PublicEffectObservation::Absent),
        Err(StoreError::Sqlite(_))
    ));
    assert_eq!(
        journal.current(candidate.key()).unwrap().unwrap().state(),
        PublicEffectState::Prepared
    );
}

#[test]
fn malformed_digest_and_transition_shape_fail_closed_as_corrupt() {
    for mutation in [
        "UPDATE public_effect_journal SET public_bytes_sha256 = zeroblob(32)",
        "UPDATE public_effect_journal SET state = 'started', attempt_count = 0, revision = 0",
    ] {
        let directory = tempdir().unwrap();
        let path = directory.path().join("effects.sqlite3");
        let candidate = prepared();
        let mut journal = SqlitePublicEffectJournal::open(&path).unwrap();
        let _ = journal.record_prepared(&candidate).unwrap();
        drop(journal);
        let control = Connection::open(&path).unwrap();
        control
            .pragma_update(None, "ignore_check_constraints", true)
            .unwrap();
        control.execute(mutation, []).unwrap();
        drop(control);

        let journal = SqlitePublicEffectJournal::open(&path).unwrap();
        assert!(matches!(
            journal.current(candidate.key()),
            Err(StoreError::CorruptPublicEffectState)
        ));
    }
}

#[test]
fn malformed_or_triggered_public_effect_schema_is_rejected_on_open() {
    for mutation in [
        "DROP TABLE public_effect_journal;
         CREATE TABLE public_effect_journal (swap_id TEXT)",
        "CREATE TRIGGER unexpected_public_effect_trigger
         AFTER INSERT ON public_effect_journal BEGIN SELECT 1; END",
    ] {
        let directory = tempdir().unwrap();
        let path = directory.path().join("effects.sqlite3");
        drop(SqlitePublicEffectJournal::open(&path).unwrap());
        let control = Connection::open(&path).unwrap();
        control.execute_batch(mutation).unwrap();
        drop(control);

        assert!(matches!(
            SqlitePublicEffectJournal::open(&path),
            Err(StoreError::CorruptPublicEffectState)
        ));
    }
}

#[test]
fn invalid_public_material_shapes_are_rejected_before_persistence() {
    assert!(matches!(
        PreparedPublicEffect::new(key(), [0; 32], "effect", vec![1]),
        Err(StoreError::InvalidPublicEffect)
    ));
    assert!(matches!(
        PreparedPublicEffect::new(key(), [0xa1; 32], "", vec![1]),
        Err(StoreError::InvalidPublicEffect)
    ));
    assert!(matches!(
        PreparedPublicEffect::new(key(), [0xa1; 32], "effect", Vec::new()),
        Err(StoreError::InvalidPublicEffect)
    ));
    assert!(matches!(
        PreparedPublicEffect::new(key(), [0xa1; 32], "contains whitespace", vec![1]),
        Err(StoreError::InvalidPublicEffect)
    ));
}
