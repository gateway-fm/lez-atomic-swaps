//! #174: an exact LEZ claim whose send failed before it reached the
//! sequencer, or ended with an ambiguous answer, keeps no dead journal row:
//! while the chain still shows the claim absent, the next drive sends the same
//! bytes again. Both owned claims take this path: the Taker's revealing claim
//! when it sells BTC and the Maker's follow-up claim when the Taker sells LEZ.

use lez_swap_core::SwapDirection;

use super::*;

const OWNED_LEZ_CLAIMS: [(SwapDirection, ActorRole, ClaimTransition); 2] = [
    (
        SwapDirection::TakerSellsForeign,
        ActorRole::Taker,
        ClaimTransition::RevealingClaim,
    ),
    (
        SwapDirection::TakerSellsLez,
        ActorRole::Maker,
        ClaimTransition::FollowupClaim,
    ),
];

type Presence = lez_bridge_client::FinalizedWitnessedClaimPresence;

async fn owned_lez_claim(
    direction: SwapDirection,
    role: ActorRole,
    transition: ClaimTransition,
) -> (ActorFixture, PreparedLezClaimEffect) {
    let fixture = ActorFixture::for_direction(direction, role);
    activate_and_project_both_locks(&fixture).await;
    if transition == ClaimTransition::FollowupClaim {
        project_revealing_claim(&fixture).await;
    }
    let effect = prepare_lez_claim_effect(
        &fixture.config,
        &fixture.agreement,
        transition,
        &durable_status(&fixture),
        &placeholder_lez_port(),
    )
    .await
    .expect("prepare the owned LEZ claim")
    .expect("this role owns the LEZ claim");
    (fixture, effect)
}

fn absent(
    fixture: &ActorFixture,
    transition: ClaimTransition,
    effect: &PreparedLezClaimEffect,
) -> Presence {
    not_found_lez_claim_presence(
        &fixture.config,
        &fixture.agreement,
        transition,
        Some(effect),
    )
}

fn journal_state(fixture: &ActorFixture, effect: &PreparedLezClaimEffect) -> PublicEffectState {
    SqlitePublicEffectJournal::open(&fixture.config.state_db)
        .expect("journal")
        .current(effect.effect.key())
        .expect("journal row")
        .expect("row exists")
        .state()
}

/// One drive of the claim against a chain showing `presence` and a sidecar
/// answering the submission with `submission`; returns the drive's result and
/// how many times it sent the claim.
async fn drive(
    fixture: &ActorFixture,
    effect: &PreparedLezClaimEffect,
    presence: Presence,
    submission: Result<SubmissionOutcome, ActorCommandError>,
) -> (Result<Value, ActorCommandError>, usize) {
    let port = FixedLezClaimPort::with_presence(effect.transaction.clone(), presence, submission);
    let result = drive_claim_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &LezClaimObserver {
            config: &fixture.config,
            chain: port.clone(),
            effect: Some(effect.clone()),
            prepared_claim: prepared_lez_claim(&fixture.config, &fixture.agreement),
            state_db: fixture.config.state_db.clone(),
        },
    )
    .await
    .map(output_json);
    for request in port.submit_requests.lock().expect("submit requests").iter() {
        assert_eq!(
            request.transaction, effect.transaction,
            "every send is the journal's exact bytes"
        );
    }
    (result, port.submit_calls())
}

/// A sidecar whose answer to the submission names another transaction: the
/// call was made and nothing can be read from what came back.
#[derive(Clone)]
struct GarbledAnswerPort(FixedLezClaimPort);

#[async_trait]
impl LezClaimChainPort for GarbledAnswerPort {
    async fn complete_witnessed_claim(
        &self,
        request: CompleteWitnessedClaimRequest,
    ) -> Result<CompleteWitnessedClaimResult, ActorCommandError> {
        self.0.complete_witnessed_claim(request).await
    }

    async fn classify_finalized_witnessed_claim(
        &self,
        request: ObserveFinalizedWitnessedClaimRequest,
    ) -> Result<Presence, ActorCommandError> {
        self.0.classify_finalized_witnessed_claim(request).await
    }

    async fn submit_transaction(
        &self,
        request: SubmitTransactionRequest,
    ) -> Result<SubmitTransactionResult, ActorCommandError> {
        let answer = self.0.submit_transaction(request).await?;
        Ok(SubmitTransactionResult::new(
            answer.context,
            TransactionId::from_bytes([0xee; 32]),
            answer.outcome,
        ))
    }
}

/// Case (a), failure before the sequencer: the sidecar could not be reached,
/// or its inclusion lookup failed before the send. The actor sees one failed
/// call either way. The next drive that finds the claim absent sends it.
#[tokio::test(flavor = "current_thread")]
async fn a_lez_claim_that_never_reached_the_sequencer_is_sent_by_the_next_drive() {
    for (direction, role, transition) in OWNED_LEZ_CLAIMS {
        let (fixture, effect) = owned_lez_claim(direction, role, transition).await;

        let (failed, sends) = drive(
            &fixture,
            &effect,
            absent(&fixture, transition, &effect),
            Err(ActorCommandError::ObservationUnavailable),
        )
        .await;
        assert_eq!(failed, Err(ActorCommandError::ObservationUnavailable));
        assert_eq!(sends, 1);
        assert_eq!(journal_state(&fixture, &effect), PublicEffectState::Unknown);

        let (retried, sends) = drive(
            &fixture,
            &effect,
            absent(&fixture, transition, &effect),
            Ok(SubmissionOutcome::Accepted),
        )
        .await;
        let retried = retried.expect("the retry sends the claim and waits for it");
        assert_eq!(retried["outcome"], "awaiting_observation");
        assert_eq!(
            sends, 1,
            "{transition:?}: the claim is sent on the next drive"
        );
        assert_eq!(
            journal_state(&fixture, &effect),
            PublicEffectState::Accepted
        );

        // Accepted by the node, it is not sent again while it waits for finality.
        let (_, sends) = drive(
            &fixture,
            &effect,
            absent(&fixture, transition, &effect),
            Ok(SubmissionOutcome::Accepted),
        )
        .await;
        assert_eq!(sends, 0);

        let (projected, sends) = drive(
            &fixture,
            &effect,
            exact_lez_claim_presence(
                &fixture.config,
                &fixture.agreement,
                transition,
                Some(&effect),
                effect.transaction.clone(),
                effect.aggregate_signature,
            ),
            Err(ActorCommandError::ObservationUnavailable),
        )
        .await;
        let projected = projected.expect("the final claim projects");
        assert_eq!(projected["outcome"], "observed_then_projected");
        assert_eq!(projected["revision"], transition.revision());
        assert_eq!(sends, 0);
    }
}

/// Case (b), an ambiguous answer after the submission: the call returned
/// something that proves neither acceptance nor refusal. Uncertain chain
/// evidence authorizes nothing; absence, or the exact-idempotent admission of
/// a prefix, sends the same bytes again, as often as the answer stays
/// ambiguous, and never once the node accepted them or the chain shows them.
#[tokio::test(flavor = "current_thread")]
async fn a_lez_claim_with_an_ambiguous_answer_is_sent_again_until_it_is_accepted() {
    for (direction, role, transition) in OWNED_LEZ_CLAIMS {
        let (fixture, effect) = owned_lez_claim(direction, role, transition).await;

        let garbled = GarbledAnswerPort(FixedLezClaimPort::with_presence(
            effect.transaction.clone(),
            absent(&fixture, transition, &effect),
            Ok(SubmissionOutcome::Accepted),
        ));
        let error = drive_claim_with_observer(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &LezClaimObserver {
                config: &fixture.config,
                chain: garbled.clone(),
                effect: Some(effect.clone()),
                prepared_claim: prepared_lez_claim(&fixture.config, &fixture.agreement),
                state_db: fixture.config.state_db.clone(),
            },
        )
        .await
        .expect_err("an answer for another transaction is not an acceptance");
        assert_eq!(error, ActorCommandError::AgreementBindingInvalid);
        assert_eq!(garbled.0.submit_calls(), 1);
        assert_eq!(journal_state(&fixture, &effect), PublicEffectState::Unknown);

        for uncertain in [
            Presence::Uncertain(lez_bridge_client::FinalizedWitnessedClaimUncertain::Timeout),
            Presence::Uncertain(lez_bridge_client::FinalizedWitnessedClaimUncertain::Transport),
        ] {
            let (waiting, sends) = drive(
                &fixture,
                &effect,
                uncertain,
                Ok(SubmissionOutcome::Accepted),
            )
            .await;
            let _ = waiting.expect("uncertain evidence stays pending");
            assert_eq!(sends, 0, "uncertainty is not evidence of absence");
        }

        let prefix = || {
            prefix_uncertain_lez_claim_presence(
                &fixture.config,
                &fixture.agreement,
                transition,
                Some(&effect),
            )
        };
        let (failed, sends) = drive(
            &fixture,
            &effect,
            prefix(),
            Err(ActorCommandError::ObservationUnavailable),
        )
        .await;
        assert_eq!(failed, Err(ActorCommandError::ObservationUnavailable));
        assert_eq!(sends, 1, "{transition:?}: sent again under a prefix");
        assert_eq!(journal_state(&fixture, &effect), PublicEffectState::Unknown);

        let (known, sends) = drive(
            &fixture,
            &effect,
            prefix(),
            Ok(SubmissionOutcome::AlreadyKnown),
        )
        .await;
        let _ = known.expect("the resend is answered");
        assert_eq!(sends, 1, "{transition:?}: sent again while still unknown");
        assert_eq!(
            journal_state(&fixture, &effect),
            PublicEffectState::Accepted
        );

        let (_, sends) = drive(&fixture, &effect, prefix(), Ok(SubmissionOutcome::Accepted)).await;
        assert_eq!(sends, 0);
    }
}

/// A process that stopped during the submission call left the row `Started`:
/// the same ambiguity, made durable. The next drive sends the claim again.
#[tokio::test(flavor = "current_thread")]
async fn a_lez_claim_left_in_flight_by_a_stopped_process_is_sent_again() {
    for (direction, role, transition) in OWNED_LEZ_CLAIMS {
        let (fixture, effect) = owned_lez_claim(direction, role, transition).await;
        let mut journal =
            SqlitePublicEffectJournal::open(&fixture.config.state_db).expect("journal");
        let _ = journal
            .record_prepared(&effect.effect)
            .expect("persist exact public bytes");
        assert!(matches!(
            journal
                .reconcile(effect.effect.key(), PublicEffectObservation::Absent)
                .expect("consume send authority before the simulated stop"),
            PublicEffectDecision::SubmitOnce(_)
        ));
        drop(journal);
        assert_eq!(journal_state(&fixture, &effect), PublicEffectState::Started);

        let (restarted, sends) = drive(
            &fixture,
            &effect,
            absent(&fixture, transition, &effect),
            Ok(SubmissionOutcome::Accepted),
        )
        .await;
        let _ = restarted.expect("the restarted drive sends the claim");
        assert_eq!(sends, 1, "{transition:?}");
        assert_eq!(
            journal_state(&fixture, &effect),
            PublicEffectState::Accepted
        );
    }
}

/// Chain evidence that contradicts the claim still ends every send: after it,
/// no later absence sends the claim again (ADR 0036).
#[tokio::test(flavor = "current_thread")]
async fn a_lez_claim_contradicted_on_the_chain_is_never_sent_again() {
    for (direction, role, transition) in OWNED_LEZ_CLAIMS {
        let (fixture, effect) = owned_lez_claim(direction, role, transition).await;
        let (_, sends) = drive(
            &fixture,
            &effect,
            absent(&fixture, transition, &effect),
            Err(ActorCommandError::ObservationUnavailable),
        )
        .await;
        assert_eq!(sends, 1);

        let (conflict, sends) = drive(
            &fixture,
            &effect,
            exact_lez_claim_presence(
                &fixture.config,
                &fixture.agreement,
                transition,
                Some(&effect),
                PreparedTransaction::new(
                    effect.transaction.transaction_id,
                    ExactTransactionBytes::new(vec![103; 128]).expect("conflicting exact bytes"),
                ),
                effect.aggregate_signature,
            ),
            Ok(SubmissionOutcome::Accepted),
        )
        .await;
        assert_eq!(conflict, Err(ActorCommandError::AgreementBindingInvalid));
        assert_eq!(sends, 0);

        for _ in 0..2 {
            let (_, sends) = drive(
                &fixture,
                &effect,
                absent(&fixture, transition, &effect),
                Ok(SubmissionOutcome::Accepted),
            )
            .await;
            assert_eq!(sends, 0, "{transition:?}: no send after a contradiction");
        }
        assert_eq!(journal_state(&fixture, &effect), PublicEffectState::Unknown);
    }
}

/// The custom-token claim is the same exact effect on the same journal key
/// and takes the same path, in both owned directions.
#[tokio::test(flavor = "current_thread")]
async fn a_token_claim_with_a_failed_send_is_sent_by_the_next_drive() {
    for (direction, role, transition) in OWNED_LEZ_CLAIMS {
        let mut fixture = ActorFixture::for_direction(direction, role);
        let _ = configure_schema5_asset_claim(&mut fixture);
        activate_and_project_schema5_both_locks(&fixture).await;
        if transition == ClaimTransition::FollowupClaim {
            project_revealing_claim(&fixture).await;
        }
        let transaction = asset_claim_transaction();
        let effect = prepare_lez_asset_claim_effect(
            &fixture.config,
            &fixture.agreement,
            transition,
            &durable_status(&fixture),
            &FixedLezAssetClaimPort::new(
                &fixture.config,
                transaction.clone(),
                unavailable_asset_claim_outcome(),
                Ok(SubmissionOutcome::Accepted),
            ),
        )
        .await
        .expect("prepare exact asset claim")
        .expect("claimant owns exact asset claim");

        for (submission, expected_sends, expected_state) in [
            (
                Err(ActorCommandError::ObservationUnavailable),
                1,
                PublicEffectState::Unknown,
            ),
            (
                Ok(SubmissionOutcome::Accepted),
                1,
                PublicEffectState::Accepted,
            ),
            (
                Ok(SubmissionOutcome::Accepted),
                0,
                PublicEffectState::Accepted,
            ),
        ] {
            let port = FixedLezAssetClaimPort::new(
                &fixture.config,
                transaction.clone(),
                absent_asset_claim_outcome(&fixture.config),
                submission,
            );
            let result = drive_claim_with_observer(
                &fixture.config,
                fixture.agreement.clone(),
                fixture.agreement_wire.clone(),
                &LezAssetClaimObserver {
                    config: &fixture.config,
                    chain: port.clone(),
                    effect: Some(effect.clone()),
                    prepared_claim: prepared_lez_asset_claim(&fixture.config, &fixture.agreement),
                    state_db: fixture.config.state_db.clone(),
                },
            )
            .await;
            assert_eq!(result.is_ok(), submission.is_ok());
            assert_eq!(port.submit_calls(), expected_sends, "{transition:?}");
            assert_eq!(journal_state(&fixture, &effect), expected_state);
        }
    }
}
