//! #156: a Bitcoin send refused by Core's readiness check before anything was
//! offered to Core (`NotAttempted`) gives its single-send authority back, so
//! the next drive sends it; an outcome after an offer is still never rearmed.

use lez_swap_core::SwapDirection;

use super::*;

fn journal_state(fixture: &ActorFixture, key: &PublicEffectKey) -> PublicEffectState {
    SqlitePublicEffectJournal::open(&fixture.config.state_db)
        .expect("journal")
        .current(key)
        .expect("journal row")
        .expect("row exists")
        .state()
}

/// Failure A (swap 59f0467b6a1c): the Taker's revealing Bitcoin claim.
#[tokio::test(flavor = "current_thread")]
async fn a_taker_btc_claim_refused_before_the_offer_is_sent_by_the_next_drive() {
    let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Taker);
    activate_and_project_both_locks(&fixture).await;
    let effect = prepare_bitcoin_claim_effect(
        &fixture.config,
        &fixture.agreement,
        ClaimTransition::RevealingClaim,
        &durable_status(&fixture),
    )
    .expect("prepare revealing effect")
    .expect("taker owns revealing effect");

    let refused = FixedBitcoinClaimPort::new(
        BitcoinClaimScan::Unspent,
        Ok(AuthorizedClaimSubmission::NotAttempted {
            reason: "required_index_not_ready",
        }),
    );
    let error = drive_claim_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &BitcoinClaimObserver {
            chain: refused.clone(),
            effect: Some(effect.clone()),
            state_db: fixture.config.state_db.clone(),
        },
    )
    .await
    .expect_err("the refused attempt fails so that it is retried");
    assert_eq!(error, ActorCommandError::ObservationUnavailable);
    assert_eq!(refused.submit_calls(), 1);
    assert_eq!(
        journal_state(&fixture, effect.effect.key()),
        PublicEffectState::Prepared
    );

    let healthy = FixedBitcoinClaimPort::new(
        BitcoinClaimScan::Unspent,
        Ok(AuthorizedClaimSubmission::Accepted {
            transaction_id: effect.expected_transaction_id,
        }),
    );
    drive_claim_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &BitcoinClaimObserver {
            chain: healthy.clone(),
            effect: Some(effect.clone()),
            state_db: fixture.config.state_db.clone(),
        },
    )
    .await
    .expect("the retry sends the claim");
    assert_eq!(healthy.submit_calls(), 1, "the claim is sent on the retry");

    // Once sent it is never sent again.
    let replay = FixedBitcoinClaimPort::new(
        BitcoinClaimScan::Unspent,
        Ok(AuthorizedClaimSubmission::Accepted {
            transaction_id: effect.expected_transaction_id,
        }),
    );
    drive_claim_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &BitcoinClaimObserver {
            chain: replay.clone(),
            effect: Some(effect),
            state_db: fixture.config.state_db.clone(),
        },
    )
    .await
    .expect("observe-only after the send");
    assert_eq!(replay.submit_calls(), 0);
}

/// Failure B (swap ce17cf7aef25): the Maker's Bitcoin lock.
#[tokio::test(flavor = "current_thread")]
async fn a_maker_btc_lock_refused_before_the_offer_is_sent_by_the_next_attempt() {
    freeze_wall_clock_before_maker_cutoff();
    let mut fixture = ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Maker);
    configure_schema4_maker_material(&mut fixture);
    activate_and_project_taker_lock(&fixture).await;
    let plan = load_prepared_maker_lock_material(&fixture.config, &fixture.agreement)
        .expect("maker material")
        .plan()
        .clone();
    let eligibility = fresh_maker_eligibility(&fixture);
    let complete = exact_maker_lock_complete_observation(&fixture.agreement, &plan, 88);
    let first_step = plan.steps().first().expect("one step").clone();
    let accepted =
        BtcMakerLockSubmissionResult::Accepted(first_step.expected_public_id().as_str().into());
    let step_state = || {
        SqliteBtcMakerLockJournal::open(&fixture.config.state_db)
            .expect("journal")
            .load_intent(fixture.agreement.coordinator().id())
            .expect("load intent")
            .expect("intent")
            .steps()[0]
            .state()
    };

    let refused = FixedMakerLockPort::new(
        MakerLockStepChainObservationV1::Absent,
        eligibility.clone(),
        complete.clone(),
    )
    .with_not_attempted("required_index_not_ready");
    let error = drive_maker_lock_with_port(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &refused,
    )
    .await
    .expect_err("the refused attempt fails so that it is retried");
    assert_eq!(error, ActorCommandError::ObservationUnavailable);
    assert_eq!(refused.submissions(), 1);
    assert_eq!(step_state(), BtcMakerLockStepState::Prepared);

    let healthy = FixedMakerLockPort::new(
        MakerLockStepChainObservationV1::Absent,
        eligibility.clone(),
        complete.clone(),
    )
    .with_submission_result(accepted.clone());
    let sent = drive_maker_lock_with_port(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &healthy,
    )
    .await
    .expect("the retry sends the lock");
    assert_eq!(sent.revision, 1);
    assert_eq!(healthy.submissions(), 1, "the lock is sent on the retry");
    assert_eq!(step_state(), BtcMakerLockStepState::Unknown);

    // Once sent it is never sent again.
    let replay = FixedMakerLockPort::new(
        MakerLockStepChainObservationV1::Absent,
        eligibility,
        complete,
    )
    .with_submission_result(accepted);
    drive_maker_lock_with_port(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &replay,
    )
    .await
    .expect("observe-only after the send");
    assert_eq!(replay.submissions(), 0);
}

/// The Bitcoin refund goes through the same pre-send readiness check.
#[tokio::test(flavor = "current_thread")]
async fn a_btc_refund_refused_before_the_offer_is_sent_by_the_next_drive() {
    let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Taker);
    activate_and_project_taker_lock(&fixture).await;
    let effect = prepared_first_lock_bitcoin_refund(&fixture);
    let accepted = AuthorizedRefundSubmission::Accepted {
        transaction_id: effect.expected_transaction_id,
        witness_transaction_id: effect.expected_witness_transaction_id,
    };

    let refused = FixedBitcoinRefundPort::new(
        BitcoinRefundScan::Eligible,
        Ok(AuthorizedRefundSubmission::NotAttempted {
            reason: "chain_not_ready",
        }),
    );
    let error = drive_admitted_first_lock_refund(
        &fixture,
        &BitcoinRefundObserver {
            chain: refused.clone(),
            effect: Some(effect.clone()),
            state_db: fixture.config.state_db.clone(),
        },
    )
    .await
    .expect_err("the refused attempt fails so that it is retried");
    assert_eq!(error, ActorCommandError::ObservationUnavailable);
    assert_eq!(refused.submit_calls(), 1);
    assert_eq!(
        journal_state(&fixture, effect.effect.key()),
        PublicEffectState::Prepared
    );

    let healthy = FixedBitcoinRefundPort::new(BitcoinRefundScan::Eligible, Ok(accepted));
    drive_admitted_first_lock_refund(
        &fixture,
        &BitcoinRefundObserver {
            chain: healthy.clone(),
            effect: Some(effect),
            state_db: fixture.config.state_db.clone(),
        },
    )
    .await
    .expect("the retry sends the refund");
    assert_eq!(healthy.submit_calls(), 1, "the refund is sent on the retry");
}

/// An error after the offer (the call may have reached Core) is unchanged:
/// journaled `Unknown`, never sent again.
#[tokio::test(flavor = "current_thread")]
async fn a_taker_btc_claim_failing_after_the_offer_is_still_never_rearmed() {
    let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Taker);
    activate_and_project_both_locks(&fixture).await;
    let effect = prepare_bitcoin_claim_effect(
        &fixture.config,
        &fixture.agreement,
        ClaimTransition::RevealingClaim,
        &durable_status(&fixture),
    )
    .expect("prepare revealing effect")
    .expect("taker owns revealing effect");
    let failing = FixedBitcoinClaimPort::new(
        BitcoinClaimScan::Unspent,
        Err(ActorCommandError::ObservationUnavailable),
    );
    drive_claim_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &BitcoinClaimObserver {
            chain: failing,
            effect: Some(effect.clone()),
            state_db: fixture.config.state_db.clone(),
        },
    )
    .await
    .expect_err("post-offer failure");
    assert_eq!(
        journal_state(&fixture, effect.effect.key()),
        PublicEffectState::Unknown
    );
    for _ in 0..3 {
        let healthy = FixedBitcoinClaimPort::new(
            BitcoinClaimScan::Unspent,
            Ok(AuthorizedClaimSubmission::Accepted {
                transaction_id: effect.expected_transaction_id,
            }),
        );
        drive_claim_with_observer(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &BitcoinClaimObserver {
                chain: healthy.clone(),
                effect: Some(effect.clone()),
                state_db: fixture.config.state_db.clone(),
            },
        )
        .await
        .expect("observe-only");
        assert_eq!(healthy.submit_calls(), 0);
    }
}
