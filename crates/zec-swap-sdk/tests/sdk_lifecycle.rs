use std::sync::Arc;

use lez_swap_core::{
    Chain, ChainPosition, LezUnixMilliseconds, Participant, Phase, SwapDirection, SwapId,
};
use lez_zec_swap_sdk::{
    AcceptedZecAgreementV1, ActiveZecSwap, CanonicalLezEscrowObservationV1,
    CanonicalLezEscrowRemovalV1, CanonicalZcashOutputObservation, ClaimDriveOutcome, ClaimError,
    ClaimPreimage, ClaimRecoveryStore, ClaimStepV1, CreateFirstLockOutcome,
    FirstLockConfirmedEvidenceV1, FirstLockDriveOutcome, FirstLockIntentRecordV1,
    FirstLockObservation, FirstLockPlanV1, FirstLockProjectionCommit, FirstLockRecordError,
    FirstLockStepV1, FirstLockTransitionError, FirstLockTransitionRecordV1, LezEnvironmentV1,
    LezNodeRemovalSnapshotV1, LezObservationEventV1, LezObservationReconciliationV1,
    LezObservationTrackerError, LezObservationTrackerV1, LezStableTipV1,
    MAX_FIRST_LOCK_SUBMISSION_BYTES, MAX_ZEC_AGREEMENT_RECORD_BYTES,
    MakerFundingEligibilityOutcome, MakerLockDriveOutcome, MakerLockObservationV1,
    ObserveMakerLockOutcome, ObserveTakerFirstLockOutcome, ObservedTakerFirstLockEvidenceV1,
    ObservedTakerFirstLockTransitionError, ObservedTakerFirstLockTransitionRecordV1,
    PreparedFirstLockSubmissionV1, PreparedRefundSubmissionV1, RefundDriveOutcome,
    RefundEligibilityObservationV1, RefundEvidenceV1, RefundFundingWaitReasonV1,
    RefundIntentRecordV1, RefundIntentV1, RefundRecordError, RefundStepV1, RefundSubmitOutcomeV1,
    RefundTransitionRecordV1, RefundTransitionV1, TakerFirstLockObservationV1,
    ZcashClaimContextError, ZcashFundingContextV1, ZcashFundingObservationV1,
    ZcashFundingWaitReasonV1, ZecAgreementV1Error, ZecLifecycleAction, ZecPairSdk, ZecProfileId,
    ZecSdkError,
};
use sha2::{Digest as _, Sha256};
use zcash_encoding::ReverseHex;
use zcash_transparent::bundle::OutPoint;

#[path = "support/memory.rs"]
mod memory;
use memory::*;
#[tokio::test]
async fn independent_roles_validate_the_same_wire_and_persist_before_activation() {
    let wire = agreement_wire(
        "sdk-forward",
        SwapDirection::TakerSellsForeign,
        FixtureVariant::Local,
    );
    let discovery = MemoryDiscovery::default();
    let negotiation = MemoryNegotiation { wire };
    let maker_store = MemoryStore::default();
    let taker_store = MemoryStore::default();
    let maker = ZecPairSdk::new(
        Participant::Maker,
        discovery.clone(),
        negotiation.clone(),
        NoopLez,
        NoopZcash,
        maker_store.clone(),
    );
    let taker = ZecPairSdk::new(
        Participant::Taker,
        discovery,
        negotiation,
        NoopLez,
        NoopZcash,
        taker_store.clone(),
    );

    let published = maker
        .publish_offer(Offer(7))
        .await
        .expect("maker publishes");
    assert_eq!(
        taker.discover_offers(&()).await.expect("taker discovers"),
        vec![published.clone()]
    );
    assert!(matches!(
        taker.publish_offer(Offer(9)).await,
        Err(ZecSdkError::WrongRole {
            expected: Participant::Maker,
            actual: Participant::Taker
        })
    ));

    let maker_terms = maker
        .negotiate_at(&published, Proposal, ACCEPTED_AT)
        .await
        .expect("maker validates countersigned wire");
    let taker_terms = taker
        .negotiate_at(&published, Proposal, ACCEPTED_AT)
        .await
        .expect("taker validates countersigned wire");
    assert_eq!(
        maker_terms.agreement().agreement_commitment(),
        taker_terms.agreement().agreement_commitment()
    );
    assert_eq!(maker_terms.local_participant(), Participant::Maker);
    assert_eq!(taker_terms.local_participant(), Participant::Taker);

    let maker_active: ActiveZecSwap<NoopLez, NoopZcash, MemoryStore> = maker
        .activate(maker_terms)
        .await
        .expect("maker persists before activation");
    let taker_active: ActiveZecSwap<NoopLez, NoopZcash, MemoryStore> = taker
        .activate(taker_terms)
        .await
        .expect("taker persists before activation");

    assert_eq!(maker_active.local_participant(), Participant::Maker);
    assert_eq!(taker_active.local_participant(), Participant::Taker);
    assert_eq!(maker_active.status(), Phase::Offered);
    assert_eq!(taker_active.status(), Phase::Offered);
    assert_eq!(maker_active.next_action(), ZecLifecycleAction::Wait);
    assert_eq!(taker_active.next_action(), ZecLifecycleAction::FundZcash);
    assert_eq!(maker_store.agreements.lock().expect("maker store").len(), 1);
    assert_eq!(taker_store.agreements.lock().expect("taker store").len(), 1);

    let swap_id = SwapId::new("sdk-forward").expect("id");
    let resumed = maker
        .resume(&swap_id)
        .await
        .expect("load succeeds")
        .expect("maker agreement exists after transcript expiry");
    assert_eq!(resumed.local_participant(), Participant::Maker);
    assert_eq!(resumed.status(), Phase::Offered);
    assert_eq!(resumed.revision(), 0);

    let sdk_debug = format!("{maker:?}");
    let active_debug = format!("{maker_active:?}");
    for diagnostic in [&sdk_debug, &active_debug] {
        assert!(!diagnostic.contains("sdk-forward"));
        assert!(!diagnostic.contains("MemoryNegotiation"));
        assert!(!diagnostic.contains("NoopLez"));
        assert!(!diagnostic.contains("NoopZcash"));
        assert!(!diagnostic.contains("MemoryStore"));
    }
}

#[tokio::test]
async fn first_claimant_activation_persists_agreement_and_claim_preimage_in_both_directions() {
    for (id, direction, first_claimant, secret) in [
        (
            "sdk-claim-activation-forward",
            SwapDirection::TakerSellsForeign,
            Participant::Taker,
            [0x41; 32],
        ),
        (
            "sdk-claim-activation-reverse",
            SwapDirection::TakerSellsLez,
            Participant::Maker,
            [0x42; 32],
        ),
    ] {
        let digest = Sha256::digest(secret).into();
        let store = MemoryStore::default();
        let sdk = sdk(
            first_claimant,
            agreement_wire_with_digest(id, direction, FixtureVariant::Local, digest),
            store.clone(),
        );
        let accepted = sdk
            .negotiate_at(&Offer(1), Proposal, ACCEPTED_AT)
            .await
            .expect("agreement");

        let active = sdk
            .activate_with_claim_preimage(accepted.clone(), ClaimPreimage::new(secret))
            .await
            .expect("agreement and protected preimage are atomically durable");
        sdk.activate_with_claim_preimage(accepted, ClaimPreimage::new(secret))
            .await
            .expect("exact agreement and preimage retry is idempotent");

        assert_eq!(active.local_participant(), first_claimant);
        assert_eq!(active.status(), Phase::Offered);
        assert_eq!(store.agreements.lock().expect("agreements lock").len(), 1);
        let protected = store
            .claim_materials
            .lock()
            .expect("claim materials lock")
            .get(id)
            .cloned()
            .expect("only protected material is retained");
        assert_eq!(protected.ciphertext().len(), 48);
        assert!(!format!("{protected:?}").contains(&format!("{secret:?}")));
        let loaded = store
            .load_claim_material(&SwapId::new(id).expect("swap ID"))
            .await
            .expect("protected material authenticates")
            .expect("claim material exists");
        assert_eq!(loaded.expose_secret(), &secret);
    }
}

#[tokio::test]
async fn accepted_wire_activation_and_resume_need_no_discovery_or_negotiation_capability() {
    let secret = [0x43; 32];
    let wire = agreement_wire_with_digest(
        "sdk-post-negotiation-only",
        SwapDirection::TakerSellsForeign,
        FixtureVariant::Local,
        Sha256::digest(secret).into(),
    );
    let maker_sdk = ZecPairSdk::new(
        Participant::Maker,
        NoPrelockCapability,
        NoPrelockCapability,
        NoopLez,
        NoopZcash,
        MemoryStore::default(),
    );
    let maker_accepted =
        AcceptedZecAgreementV1::accept_wire_at(&wire, ACCEPTED_AT, Participant::Maker, 0)
            .expect("maker accepts the signed wire independently");
    let maker_active = maker_sdk
        .activate(maker_accepted)
        .await
        .expect("plain activation has no transcript capability");
    assert_eq!(maker_active.status(), Phase::Offered);

    let store = MemoryStore::default();
    let sdk = ZecPairSdk::new(
        Participant::Taker,
        NoPrelockCapability,
        NoPrelockCapability,
        NoopLez,
        NoopZcash,
        store,
    );
    let accepted =
        AcceptedZecAgreementV1::accept_wire_at(&wire, ACCEPTED_AT, Participant::Taker, 0)
            .expect("signed wire is independently accepted");
    let active = sdk
        .activate_with_claim_preimage(accepted, ClaimPreimage::new(secret))
        .await
        .expect("post-negotiation activation has no transcript capability");
    let swap_id = active.agreement().coordinator().id().clone();
    assert_eq!(active.status(), Phase::Offered);

    let resumed = sdk
        .resume(&swap_id)
        .await
        .expect("post-negotiation resume")
        .expect("durable active swap");
    assert_eq!(resumed.status(), Phase::Offered);
    assert_eq!(resumed.revision(), 0);
}

#[tokio::test]
async fn claim_activation_rejects_wrong_role_or_digest_before_activation_or_effect() {
    for (id, direction, first_claimant, other_participant, secret) in [
        (
            "sdk-claim-reject-forward",
            SwapDirection::TakerSellsForeign,
            Participant::Taker,
            Participant::Maker,
            [0x51; 32],
        ),
        (
            "sdk-claim-reject-reverse",
            SwapDirection::TakerSellsLez,
            Participant::Maker,
            Participant::Taker,
            [0x52; 32],
        ),
    ] {
        let digest = Sha256::digest(secret).into();
        let wire = agreement_wire_with_digest(id, direction, FixtureVariant::Local, digest);

        let wrong_role_store = MemoryStore::default();
        let wrong_role_sdk = sdk(other_participant, wire.clone(), wrong_role_store.clone());
        let wrong_role_terms = wrong_role_sdk
            .negotiate_at(&Offer(1), Proposal, ACCEPTED_AT)
            .await
            .expect("agreement");
        assert!(matches!(
            wrong_role_sdk
                .activate_with_claim_preimage(wrong_role_terms, ClaimPreimage::new(secret))
                .await,
            Err(ZecSdkError::WrongRole {
                expected,
                actual
            }) if expected == first_claimant && actual == other_participant
        ));
        assert_store_has_no_activation_effects(&wrong_role_store);

        let wrong_digest_store = MemoryStore::default();
        let wrong_digest_sdk = sdk(first_claimant, wire, wrong_digest_store.clone());
        let wrong_digest_terms = wrong_digest_sdk
            .negotiate_at(&Offer(1), Proposal, ACCEPTED_AT)
            .await
            .expect("agreement");
        assert!(matches!(
            wrong_digest_sdk
                .activate_with_claim_preimage(wrong_digest_terms, ClaimPreimage::new([0xff; 32]),)
                .await,
            Err(ZecSdkError::InvalidClaim(ClaimError::SecretDigestMismatch))
        ));
        assert_store_has_no_activation_effects(&wrong_digest_store);
    }
}

#[tokio::test]
async fn reverse_direction_preserves_the_role_fixed_first_action() {
    let wire = agreement_wire(
        "sdk-reverse",
        SwapDirection::TakerSellsLez,
        FixtureVariant::Local,
    );
    let sdk = sdk(Participant::Taker, wire, MemoryStore::default());
    let accepted = sdk
        .negotiate_at(&Offer(1), Proposal, ACCEPTED_AT)
        .await
        .expect("agreement");
    let active = sdk.activate(accepted).await.expect("activation succeeds");
    assert_eq!(active.next_action(), ZecLifecycleAction::CreateAndFundLez);
}

#[tokio::test]
async fn first_lock_intent_is_role_fixed_and_durable_before_any_effect() {
    let forward_wire = agreement_wire(
        "sdk-first-lock-forward",
        SwapDirection::TakerSellsForeign,
        FixtureVariant::Local,
    );
    let store = MemoryStore::default();
    let taker = sdk(Participant::Taker, forward_wire.clone(), store.clone());
    let accepted = taker
        .negotiate_at(&Offer(1), Proposal, ACCEPTED_AT)
        .await
        .expect("agreement");
    let active = taker.activate(accepted).await.expect("activation");
    let plan = FirstLockPlanV1::zcash(
        PreparedFirstLockSubmissionV1::new(
            FirstLockStepV1::ZcashFund,
            [0x31; 32],
            vec![0x51, 0x52, 0x53],
        )
        .expect("bounded exact transaction"),
    )
    .expect("direction-independent plan shape");

    assert_eq!(
        active
            .stage_first_lock(plan.clone())
            .await
            .expect("intent is durable"),
        CreateFirstLockOutcome::Created
    );
    assert_eq!(active.status(), Phase::Offered);
    assert_eq!(
        active
            .stage_first_lock(plan)
            .await
            .expect("exact retry is idempotent"),
        CreateFirstLockOutcome::ExistingSame
    );
    let changed = FirstLockPlanV1::zcash(
        PreparedFirstLockSubmissionV1::new(
            FirstLockStepV1::ZcashFund,
            [0x31; 32],
            vec![0x51, 0x52, 0x54],
        )
        .expect("bounded changed transaction"),
    )
    .expect("plan");
    assert!(matches!(
        active.stage_first_lock(changed).await,
        Err(ZecSdkError::FirstLockConflict)
    ));

    let maker = sdk(Participant::Maker, forward_wire, MemoryStore::default());
    let maker_terms = maker
        .negotiate_at(&Offer(1), Proposal, ACCEPTED_AT)
        .await
        .expect("agreement");
    let maker_active = maker.activate(maker_terms).await.expect("activation");
    let substituted = FirstLockPlanV1::zcash(
        PreparedFirstLockSubmissionV1::new(FirstLockStepV1::ZcashFund, [0x41; 32], vec![0x61])
            .expect("bounded transaction"),
    )
    .expect("plan");
    assert!(matches!(
        maker_active.stage_first_lock(substituted).await,
        Err(ZecSdkError::WrongRole {
            expected: Participant::Taker,
            actual: Participant::Maker
        })
    ));
}

#[tokio::test]
async fn restart_observes_before_exact_zcash_rebroadcast_without_advancing_phase() {
    let wire = agreement_wire(
        "sdk-first-lock-zcash-restart",
        SwapDirection::TakerSellsForeign,
        FixtureVariant::Local,
    );
    let store = MemoryStore::default();
    let lez = MemoryLezPort::default();
    let zcash = MemoryZcashPort::default();
    let first = first_lock_sdk(
        Participant::Taker,
        wire.clone(),
        lez.clone(),
        zcash.clone(),
        store.clone(),
    );
    let accepted = first
        .negotiate_at(&Offer(1), Proposal, ACCEPTED_AT)
        .await
        .expect("agreement");
    let active = first.activate(accepted).await.expect("activation");
    let exact = vec![0x51, 0x52, 0x53];
    active
        .stage_first_lock(
            FirstLockPlanV1::zcash(
                PreparedFirstLockSubmissionV1::new(
                    FirstLockStepV1::ZcashFund,
                    [0x31; 32],
                    exact.clone(),
                )
                .expect("submission"),
            )
            .expect("plan"),
        )
        .await
        .expect("durable before node call");

    zcash
        .0
        .observe_as(FirstLockStepV1::ZcashFund, FirstLockObservation::Unstable);
    assert_eq!(
        active.drive_first_lock().await.expect("unstable query"),
        FirstLockDriveOutcome::AwaitingStableObservation(FirstLockStepV1::ZcashFund)
    );
    assert!(zcash.0.submissions().is_empty());
    zcash
        .0
        .observe_as(FirstLockStepV1::ZcashFund, FirstLockObservation::Absent);

    assert_eq!(
        active.drive_first_lock().await.expect("first submission"),
        FirstLockDriveOutcome::Submitted(FirstLockStepV1::ZcashFund)
    );
    assert_eq!(
        zcash.0.submissions(),
        vec![(FirstLockStepV1::ZcashFund, exact)]
    );
    assert_eq!(active.status(), Phase::Offered);

    zcash.0.observe_as(
        FirstLockStepV1::ZcashFund,
        FirstLockObservation::Confirmed(confirmed_zcash_first_lock(
            [0x32; 32],
            "wrong-zcash-first-lock",
        )),
    );
    assert!(matches!(
        active.drive_first_lock().await,
        Err(ZecSdkError::InvalidFirstLockTransition(
            FirstLockTransitionError::SubmissionIdentityMismatch
        ))
    ));

    let confirmed = confirmed_zcash_first_lock([0x31; 32], "restarted-zcash-first-lock");
    zcash.0.observe_as(
        FirstLockStepV1::ZcashFund,
        FirstLockObservation::Confirmed(confirmed.clone()),
    );
    let restarted = first_lock_sdk(Participant::Taker, wire, lez, zcash.clone(), store);
    let resumed = restarted
        .resume(&SwapId::new("sdk-first-lock-zcash-restart").expect("id"))
        .await
        .expect("resume")
        .expect("active");
    assert_eq!(
        resumed.drive_first_lock().await.expect("observe first"),
        FirstLockDriveOutcome::ReadyForFundingProjection(confirmed)
    );
    assert_eq!(zcash.0.submissions().len(), 1);
    assert_eq!(resumed.status(), Phase::Offered);
}

#[tokio::test]
async fn lez_initialize_is_observed_before_fund_and_each_retry_is_exact() {
    let wire = agreement_wire(
        "sdk-first-lock-lez-steps",
        SwapDirection::TakerSellsLez,
        FixtureVariant::Local,
    );
    let store = MemoryStore::default();
    let lez = MemoryLezPort::default();
    let sdk = first_lock_sdk(
        Participant::Taker,
        wire,
        lez.clone(),
        MemoryZcashPort::default(),
        store,
    );
    let accepted = sdk
        .negotiate_at(&Offer(1), Proposal, ACCEPTED_AT)
        .await
        .expect("agreement");
    let active = sdk.activate(accepted).await.expect("activation");
    active
        .stage_first_lock(
            FirstLockPlanV1::lez(
                PreparedFirstLockSubmissionV1::new(
                    FirstLockStepV1::LezInitialize,
                    [0x11; 32],
                    vec![0x71],
                )
                .expect("initialize"),
                PreparedFirstLockSubmissionV1::new(
                    FirstLockStepV1::LezFund,
                    [0x12; 32],
                    vec![0x72],
                )
                .expect("fund"),
            )
            .expect("ordered plan"),
        )
        .await
        .expect("both steps durable");

    assert_eq!(
        active.drive_first_lock().await.expect("initialize"),
        FirstLockDriveOutcome::Submitted(FirstLockStepV1::LezInitialize)
    );
    assert_eq!(
        lez.0.submissions(),
        vec![(FirstLockStepV1::LezInitialize, vec![0x71])]
    );

    lez.0.observe_as(
        FirstLockStepV1::LezInitialize,
        confirmed_first_lock_observation(
            FirstLockStepV1::LezInitialize,
            [0x11; 32],
            "lez-initialize",
        ),
    );
    assert_eq!(
        active.drive_first_lock().await.expect("fund"),
        FirstLockDriveOutcome::Submitted(FirstLockStepV1::LezFund)
    );
    assert_eq!(
        lez.0.submissions(),
        vec![
            (FirstLockStepV1::LezInitialize, vec![0x71]),
            (FirstLockStepV1::LezFund, vec![0x72]),
        ]
    );

    let fund_evidence = confirmed_first_lock(FirstLockStepV1::LezFund, [0x12; 32], "lez-fund");
    lez.0.observe_as(
        FirstLockStepV1::LezFund,
        FirstLockObservation::Confirmed(fund_evidence.clone()),
    );
    assert_eq!(
        active.drive_first_lock().await.expect("both observed"),
        FirstLockDriveOutcome::ReadyForFundingProjection(fund_evidence)
    );
    assert_eq!(lez.0.submissions().len(), 2);
    assert_eq!(active.status(), Phase::Offered);
}

#[tokio::test]
async fn confirmed_first_lock_commits_before_apply_and_replays_after_restart() {
    let id = "sdk-first-lock-project";
    let wire = agreement_wire(id, SwapDirection::TakerSellsForeign, FixtureVariant::Local);
    let store = MemoryStore::default();
    let sdk = first_lock_sdk(
        Participant::Taker,
        wire.clone(),
        MemoryLezPort::default(),
        MemoryZcashPort::default(),
        store.clone(),
    );
    let accepted = sdk
        .negotiate_at(&Offer(1), Proposal, ACCEPTED_AT)
        .await
        .expect("agreement");
    let mut active = sdk.activate(accepted).await.expect("activation");
    active
        .stage_first_lock(zcash_first_lock_plan([0x31; 32], vec![0x51]))
        .await
        .expect("intent");

    let commit = active
        .project_first_lock(confirmed_zcash_first_lock([0x31; 32], "zec-first-lock"))
        .await
        .expect("atomic projection");
    assert_eq!(commit, FirstLockProjectionCommit::new(1, false));
    assert_eq!(active.revision(), 1);
    assert_eq!(active.status(), Phase::TakerLockConfirmed);
    assert!(store.first_locks.lock().expect("intent lock").is_empty());
    assert!(matches!(
        active
            .stage_first_lock(zcash_first_lock_plan([0x31; 32], vec![0x51]))
            .await,
        Err(ZecSdkError::FirstLockNotOffered(Phase::TakerLockConfirmed))
    ));

    let restarted = first_lock_sdk(
        Participant::Taker,
        wire,
        MemoryLezPort::default(),
        MemoryZcashPort::default(),
        store,
    );
    let resumed = restarted
        .resume(&SwapId::new(id).expect("id"))
        .await
        .expect("resume")
        .expect("active");
    assert_eq!(resumed.revision(), 1);
    assert_eq!(resumed.status(), Phase::TakerLockConfirmed);
}

#[tokio::test]
async fn projection_failure_never_mutates_core_and_unknown_success_is_probed() {
    let wire = agreement_wire(
        "sdk-first-lock-atomic-faults",
        SwapDirection::TakerSellsForeign,
        FixtureVariant::Local,
    );
    let store = MemoryStore::default();
    let sdk = first_lock_sdk(
        Participant::Taker,
        wire,
        MemoryLezPort::default(),
        MemoryZcashPort::default(),
        store.clone(),
    );
    let accepted = sdk
        .negotiate_at(&Offer(1), Proposal, ACCEPTED_AT)
        .await
        .expect("agreement");
    let mut active = sdk.activate(accepted).await.expect("activation");
    active
        .stage_first_lock(zcash_first_lock_plan([0x32; 32], vec![0x52]))
        .await
        .expect("intent");

    store.set_transition_mode(TransitionCommitMode::FailBeforeCommit);
    assert!(matches!(
        active
            .project_first_lock(
                FirstLockConfirmedEvidenceV1::new(
                    FirstLockStepV1::ZcashFund,
                    [0x33; 32],
                    "wrong-identity",
                    100,
                )
                .expect("well-formed but mismatched evidence")
            )
            .await,
        Err(ZecSdkError::InvalidFirstLockTransition(_))
    ));
    assert_eq!(active.revision(), 0);
    assert_eq!(active.status(), Phase::Offered);
    assert!(matches!(
        active
            .project_first_lock(confirmed_zcash_first_lock([0x32; 32], "fault-first-lock"))
            .await,
        Err(ZecSdkError::Persistence(_))
    ));
    assert_eq!(active.revision(), 0);
    assert_eq!(active.status(), Phase::Offered);

    store.set_transition_mode(TransitionCommitMode::CommitThenReportFailure);
    let commit = active
        .project_first_lock(confirmed_zcash_first_lock([0x32; 32], "fault-first-lock"))
        .await
        .expect("exact probe proves unknown success");
    assert_eq!(commit, FirstLockProjectionCommit::new(1, true));
    assert_eq!(active.revision(), 1);
    assert_eq!(active.status(), Phase::TakerLockConfirmed);
}

#[tokio::test]
async fn maker_independently_observes_and_durably_projects_taker_first_locks_in_both_directions() {
    let forward_id = "sdk-maker-observes-zcash";
    let forward_wire = agreement_wire(
        forward_id,
        SwapDirection::TakerSellsForeign,
        FixtureVariant::Local,
    );
    let forward_store = MemoryStore::default();
    let forward_lez = MemoryLezTakerLockObservation::default();
    let forward_zcash = MemoryZcashTakerLockObservation::default();
    let forward_sdk = ZecPairSdk::new(
        Participant::Maker,
        MemoryDiscovery::default(),
        MemoryNegotiation {
            wire: forward_wire.clone(),
        },
        forward_lez.clone(),
        forward_zcash.clone(),
        forward_store.clone(),
    );
    let accepted = forward_sdk
        .negotiate_at(&Offer(1), Proposal, ACCEPTED_AT)
        .await
        .expect("maker validates the same signed agreement");
    let mut maker = forward_sdk
        .activate(accepted)
        .await
        .expect("maker activation");
    assert_eq!(maker.status(), Phase::Offered);
    assert_eq!(maker.next_action(), ZecLifecycleAction::Wait);

    assert_forward_observation_does_not_advance(
        &mut maker,
        &forward_lez,
        &forward_zcash,
        &forward_store,
    )
    .await;

    forward_zcash.0.respond(Ok(confirmed_observed_taker_lock(
        FirstLockStepV1::ZcashFund,
        "primitive-zcash-assertion",
        100,
    )));
    assert!(matches!(
        maker.observe_taker_first_lock().await,
        Err(ZecSdkError::InvalidObservedTakerFirstLockTransition(_))
    ));
    assert_eq!(maker.status(), Phase::Offered);
    let canonical = canonical_zcash_taker_lock(maker.agreement());
    forward_zcash
        .0
        .respond(Ok(TakerFirstLockObservationV1::CanonicalZcash(Box::new(
            canonical.clone(),
        ))));
    forward_store.set_transition_mode(TransitionCommitMode::FailBeforeCommit);
    assert!(maker.observe_taker_first_lock().await.is_err());
    assert_eq!(maker.status(), Phase::Offered);
    assert_eq!(maker.revision(), 0);

    forward_store.set_transition_mode(TransitionCommitMode::CommitThenReportFailure);
    assert_eq!(
        maker
            .observe_taker_first_lock()
            .await
            .expect("exact predecessor-slot probe proves unknown commit"),
        ObserveTakerFirstLockOutcome::Projected(FirstLockProjectionCommit::new(1, true))
    );
    assert_eq!(maker.status(), Phase::TakerLockConfirmed);
    assert_eq!(maker.revision(), 1);
    assert_eq!(
        maker.next_action(),
        ZecLifecycleAction::Wait,
        "adapter-asserted maker observation must not authorize the second lock"
    );
    assert_eq!(
        maker
            .observe_taker_first_lock()
            .await
            .expect("unchanged canonical poll is not persisted"),
        ObserveTakerFirstLockOutcome::Unchanged(FirstLockStepV1::ZcashFund)
    );
    assert_eq!(maker.revision(), 1);
    assert_fresh_forward_eligibility(&mut maker, 1).await;
    assert_noneligible_fresh_polls(&mut maker, &forward_zcash, &forward_store, &canonical).await;

    assert_forward_canonical_history_survives_restart(
        forward_id,
        forward_wire,
        forward_lez,
        forward_zcash,
        forward_store,
        canonical,
    )
    .await;
}

#[tokio::test]
async fn maker_happy_path_locks_the_opposite_chain_and_restarts_in_both_legs_locked() {
    assert_maker_happy_path_zcash_to_lez().await;
    assert_maker_happy_path_lez_to_zcash().await;
}

#[tokio::test]
async fn stale_maker_replays_committed_second_lock_without_resubmission() {
    let id = "sdk-stale-maker-second-lock";
    let wire = agreement_wire(id, SwapDirection::TakerSellsForeign, FixtureVariant::Local);
    let store = MemoryStore::default();
    let lez = MemoryLezTakerLockObservation::default();
    let zcash = MemoryZcashTakerLockObservation::default();
    let first_sdk = maker_lock_sdk(wire.clone(), lez.clone(), zcash.clone(), store.clone());
    let accepted = first_sdk
        .negotiate_at(&Offer(1), Proposal, ACCEPTED_AT)
        .await
        .expect("maker agreement");
    let mut first = first_sdk.activate(accepted).await.expect("first maker");
    zcash
        .0
        .respond(Ok(TakerFirstLockObservationV1::CanonicalZcash(Box::new(
            canonical_zcash_taker_lock(first.agreement()),
        ))));
    first
        .observe_taker_first_lock()
        .await
        .expect("canonical taker lock");

    let stale_sdk = maker_lock_sdk(wire, lez.clone(), zcash, store.clone());
    let mut stale = stale_sdk
        .resume(&SwapId::new(id).expect("swap ID"))
        .await
        .expect("stale resume")
        .expect("stale active maker");
    assert_eq!(
        (stale.status(), stale.revision()),
        (Phase::TakerLockConfirmed, 1)
    );

    let plan = FirstLockPlanV1::lez(
        PreparedFirstLockSubmissionV1::new(FirstLockStepV1::LezInitialize, [0x91; 32], vec![0xc1])
            .expect("initialize"),
        PreparedFirstLockSubmissionV1::new(FirstLockStepV1::LezFund, [0x92; 32], vec![0xc2])
            .expect("fund"),
    )
    .expect("maker plan");
    first
        .drive_maker_lock(plan.clone())
        .await
        .expect("submit initialize");
    lez.2.observe_as(
        FirstLockStepV1::LezInitialize,
        confirmed_first_lock_observation(
            FirstLockStepV1::LezInitialize,
            [0x91; 32],
            "stale-maker-lez-initialize",
        ),
    );
    first
        .drive_maker_lock(plan.clone())
        .await
        .expect("submit fund");
    lez.2.observe_as(
        FirstLockStepV1::LezFund,
        confirmed_first_lock_observation(
            FirstLockStepV1::LezFund,
            [0x92; 32],
            "stale-maker-lez-fund",
        ),
    );
    first
        .project_maker_lock(
            FirstLockConfirmedEvidenceV1::new(
                FirstLockStepV1::LezFund,
                [0x92; 32],
                "stale-maker-lez-fund",
                100,
            )
            .expect("maker evidence"),
        )
        .await
        .expect("first maker commits");
    let submissions = lez.2.submissions();

    assert_eq!(
        stale
            .drive_maker_lock(plan)
            .await
            .expect("stale maker catches up"),
        MakerLockDriveOutcome::AlreadyLocked { revision: 2 }
    );
    assert_eq!(
        (stale.status(), stale.revision()),
        (Phase::BothLegsLocked, 2)
    );
    assert_eq!(
        lez.2.submissions(),
        submissions,
        "stale retry must not submit"
    );
    assert_eq!(
        store
            .maker_lock_transitions
            .lock()
            .expect("maker transitions")
            .len(),
        1
    );
}

#[tokio::test]
async fn maker_projection_failure_preserves_intent_and_unknown_commit_is_probed() {
    let wire = agreement_wire(
        "sdk-maker-projection-faults",
        SwapDirection::TakerSellsForeign,
        FixtureVariant::Local,
    );
    let store = MemoryStore::default();
    let lez = MemoryLezTakerLockObservation::default();
    let zcash = MemoryZcashTakerLockObservation::default();
    let sdk = maker_lock_sdk(wire, lez, zcash.clone(), store.clone());
    let accepted = sdk
        .negotiate_at(&Offer(1), Proposal, ACCEPTED_AT)
        .await
        .expect("maker agreement");
    let mut active = sdk.activate(accepted).await.expect("maker activation");
    zcash
        .0
        .respond(Ok(TakerFirstLockObservationV1::CanonicalZcash(Box::new(
            canonical_zcash_taker_lock(active.agreement()),
        ))));
    active
        .observe_taker_first_lock()
        .await
        .expect("canonical taker lock");
    active
        .drive_maker_lock(
            FirstLockPlanV1::lez(
                PreparedFirstLockSubmissionV1::new(
                    FirstLockStepV1::LezInitialize,
                    [0xa1; 32],
                    vec![0xd1],
                )
                .expect("initialize"),
                PreparedFirstLockSubmissionV1::new(
                    FirstLockStepV1::LezFund,
                    [0xa2; 32],
                    vec![0xd2],
                )
                .expect("fund"),
            )
            .expect("maker plan"),
        )
        .await
        .expect("maker intent is durable");
    let evidence = FirstLockConfirmedEvidenceV1::new(
        FirstLockStepV1::LezFund,
        [0xa2; 32],
        "maker-projection-fault",
        100,
    )
    .expect("maker evidence");

    store.set_transition_mode(TransitionCommitMode::FailBeforeCommit);
    assert!(matches!(
        active.project_maker_lock(evidence.clone()).await,
        Err(ZecSdkError::Persistence(_))
    ));
    assert_eq!(
        (active.status(), active.revision()),
        (Phase::TakerLockConfirmed, 1)
    );
    assert_eq!(store.maker_locks.lock().expect("maker intent").len(), 1);
    assert!(
        store
            .maker_lock_transitions
            .lock()
            .expect("maker transitions")
            .is_empty()
    );

    store.set_transition_mode(TransitionCommitMode::CommitThenReportFailure);
    assert_eq!(
        active
            .project_maker_lock(evidence)
            .await
            .expect("exact probe proves unknown maker commit"),
        FirstLockProjectionCommit::new(2, true)
    );
    assert_eq!(
        (active.status(), active.revision()),
        (Phase::BothLegsLocked, 2)
    );
    assert!(store.maker_locks.lock().expect("maker intent").is_empty());
    assert_eq!(
        store
            .maker_lock_transitions
            .lock()
            .expect("maker transitions")
            .len(),
        1
    );
}

#[tokio::test]
async fn noneligible_maker_never_stages_or_submits_in_either_direction() {
    for (id, direction) in [
        ("sdk-noneligible-forward", SwapDirection::TakerSellsForeign),
        ("sdk-noneligible-reverse", SwapDirection::TakerSellsLez),
    ] {
        let store = MemoryStore::default();
        let lez = MemoryLezTakerLockObservation::default();
        let zcash = MemoryZcashTakerLockObservation::default();
        let sdk = maker_lock_sdk(
            agreement_wire(id, direction, FixtureVariant::Local),
            lez.clone(),
            zcash.clone(),
            store.clone(),
        );
        let accepted = sdk
            .negotiate_at(&Offer(1), Proposal, ACCEPTED_AT)
            .await
            .expect("maker agreement");
        let mut active = sdk.activate(accepted).await.expect("maker activation");
        let (plan, expected_step) = match direction {
            SwapDirection::TakerSellsForeign => (
                FirstLockPlanV1::lez(
                    PreparedFirstLockSubmissionV1::new(
                        FirstLockStepV1::LezInitialize,
                        [0xc1; 32],
                        vec![0xf1],
                    )
                    .expect("initialize"),
                    PreparedFirstLockSubmissionV1::new(
                        FirstLockStepV1::LezFund,
                        [0xc2; 32],
                        vec![0xf2],
                    )
                    .expect("fund"),
                )
                .expect("forward maker plan"),
                FirstLockStepV1::ZcashFund,
            ),
            SwapDirection::TakerSellsLez => (
                zcash_first_lock_plan([0xc3; 32], vec![0xf3]),
                FirstLockStepV1::LezFund,
            ),
        };
        assert_eq!(
            active
                .drive_maker_lock(plan)
                .await
                .expect("noneligible is a typed wait"),
            MakerLockDriveOutcome::AwaitingEligibility(
                MakerFundingEligibilityOutcome::AwaitingStableObservation(expected_step)
            )
        );
        assert_eq!((active.status(), active.revision()), (Phase::Offered, 0));
        assert!(store.maker_locks.lock().expect("maker intents").is_empty());
        assert!(lez.2.submissions().is_empty());
        assert!(zcash.1.submissions().is_empty());
    }
}

#[tokio::test]
async fn maker_restart_observes_each_unknown_lez_submission_before_any_rebroadcast() {
    let id = "sdk-maker-unknown-lez-submissions";
    let wire = agreement_wire(id, SwapDirection::TakerSellsForeign, FixtureVariant::Local);
    let store = MemoryStore::default();
    let lez = MemoryLezTakerLockObservation::default();
    let zcash = MemoryZcashTakerLockObservation::default();
    let sdk = maker_lock_sdk(wire.clone(), lez.clone(), zcash.clone(), store.clone());
    let accepted = sdk
        .negotiate_at(&Offer(1), Proposal, ACCEPTED_AT)
        .await
        .expect("maker agreement");
    let mut active = sdk.activate(accepted).await.expect("maker activation");
    zcash
        .0
        .respond(Ok(TakerFirstLockObservationV1::CanonicalZcash(Box::new(
            canonical_zcash_taker_lock(active.agreement()),
        ))));
    active
        .observe_taker_first_lock()
        .await
        .expect("canonical taker lock");
    let plan = FirstLockPlanV1::lez(
        PreparedFirstLockSubmissionV1::new(
            FirstLockStepV1::LezInitialize,
            [0xd1; 32],
            vec![0x01, 0x02],
        )
        .expect("initialize"),
        PreparedFirstLockSubmissionV1::new(FirstLockStepV1::LezFund, [0xd2; 32], vec![0x03, 0x04])
            .expect("fund"),
    )
    .expect("maker LEZ plan");

    lez.2.fail_after_accept(FirstLockStepV1::LezInitialize);
    assert!(matches!(
        active.drive_maker_lock(plan.clone()).await,
        Err(ZecSdkError::LezFirstLock(_))
    ));
    assert_eq!(
        lez.2.submissions(),
        vec![(FirstLockStepV1::LezInitialize, vec![0x01, 0x02])]
    );
    assert_eq!(store.maker_locks.lock().expect("maker intent").len(), 1);

    lez.2.observe_as(
        FirstLockStepV1::LezInitialize,
        confirmed_first_lock_observation(
            FirstLockStepV1::LezInitialize,
            [0xd1; 32],
            "unknown-lez-initialize",
        ),
    );
    lez.2.fail_after_accept(FirstLockStepV1::LezFund);
    let restarted_sdk = maker_lock_sdk(wire.clone(), lez.clone(), zcash.clone(), store.clone());
    let mut restarted = restarted_sdk
        .resume(&SwapId::new(id).expect("swap ID"))
        .await
        .expect("restart after initialize")
        .expect("maker agreement");
    assert!(matches!(
        restarted.drive_maker_lock(plan.clone()).await,
        Err(ZecSdkError::LezFirstLock(_))
    ));
    assert_eq!(
        lez.2.submissions(),
        vec![
            (FirstLockStepV1::LezInitialize, vec![0x01, 0x02]),
            (FirstLockStepV1::LezFund, vec![0x03, 0x04]),
        ]
    );

    let fund_evidence =
        confirmed_first_lock(FirstLockStepV1::LezFund, [0xd2; 32], "unknown-lez-fund");
    lez.2.observe_as(
        FirstLockStepV1::LezFund,
        FirstLockObservation::Confirmed(fund_evidence.clone()),
    );
    let final_sdk = maker_lock_sdk(wire, lez.clone(), zcash, store);
    let mut final_attempt = final_sdk
        .resume(&SwapId::new(id).expect("swap ID"))
        .await
        .expect("restart after fund")
        .expect("maker agreement");
    assert_eq!(
        final_attempt
            .drive_maker_lock(plan)
            .await
            .expect("both accepted submissions are observed"),
        MakerLockDriveOutcome::Lock(FirstLockDriveOutcome::ReadyForFundingProjection(
            fund_evidence.clone(),
        ))
    );
    assert_eq!(lez.2.submissions().len(), 2, "neither step is rebroadcast");
    final_attempt
        .project_maker_lock(fund_evidence)
        .await
        .expect("project confirmed maker funding");
    assert_eq!(final_attempt.status(), Phase::BothLegsLocked);
}

#[tokio::test]
async fn maker_restart_observes_unknown_zcash_submission_before_any_rebroadcast() {
    let id = "sdk-maker-unknown-zcash-submission";
    let wire = agreement_wire(id, SwapDirection::TakerSellsLez, FixtureVariant::Local);
    let store = MemoryStore::default();
    let lez = MemoryLezTakerLockObservation::default();
    let zcash = MemoryZcashTakerLockObservation::default();
    let sdk = maker_lock_sdk(wire.clone(), lez.clone(), zcash.clone(), store.clone());
    let accepted = sdk
        .negotiate_at(&Offer(1), Proposal, ACCEPTED_AT)
        .await
        .expect("maker agreement");
    let mut active = sdk.activate(accepted).await.expect("maker activation");
    lez.0
        .respond(Ok(TakerFirstLockObservationV1::CanonicalLez(Box::new(
            canonical_lez_taker_lock(active.agreement()),
        ))));
    active
        .observe_taker_first_lock()
        .await
        .expect("canonical taker lock");
    let plan = zcash_first_lock_plan([0xe1; 32], vec![0x05, 0x06]);
    zcash.1.fail_after_accept(FirstLockStepV1::ZcashFund);
    assert!(matches!(
        active.drive_maker_lock(plan.clone()).await,
        Err(ZecSdkError::ZcashFirstLock(_))
    ));
    assert_eq!(
        zcash.1.submissions(),
        vec![(FirstLockStepV1::ZcashFund, vec![0x05, 0x06])]
    );

    let fund_evidence = confirmed_zcash_first_lock([0xe1; 32], "unknown-zcash-fund");
    zcash.1.observe_as(
        FirstLockStepV1::ZcashFund,
        FirstLockObservation::Confirmed(fund_evidence.clone()),
    );
    let restarted_sdk = maker_lock_sdk(wire, lez, zcash.clone(), store);
    let mut restarted = restarted_sdk
        .resume(&SwapId::new(id).expect("swap ID"))
        .await
        .expect("restart after Zcash submission")
        .expect("maker agreement");
    assert_eq!(
        restarted
            .drive_maker_lock(plan)
            .await
            .expect("accepted Zcash submission is observed"),
        MakerLockDriveOutcome::Lock(FirstLockDriveOutcome::ReadyForFundingProjection(
            fund_evidence.clone(),
        ))
    );
    assert_eq!(zcash.1.submissions().len(), 1, "Zcash is not rebroadcast");
    restarted
        .project_maker_lock(fund_evidence)
        .await
        .expect("project confirmed maker funding");
    assert_eq!(restarted.status(), Phase::BothLegsLocked);
}

#[tokio::test]
async fn taker_reorg_between_lez_maker_steps_suspends_funding_until_replacement() {
    let id = "sdk-maker-lez-suspended-by-taker-reorg";
    let store = MemoryStore::default();
    let lez = MemoryLezTakerLockObservation::default();
    let zcash = MemoryZcashTakerLockObservation::default();
    let sdk = maker_lock_sdk(
        agreement_wire(id, SwapDirection::TakerSellsForeign, FixtureVariant::Local),
        lez.clone(),
        zcash.clone(),
        store,
    );
    let accepted = sdk
        .negotiate_at(&Offer(1), Proposal, ACCEPTED_AT)
        .await
        .expect("maker agreement");
    let mut active = sdk.activate(accepted).await.expect("maker activation");
    let canonical = canonical_zcash_taker_lock(active.agreement());
    zcash
        .0
        .respond(Ok(TakerFirstLockObservationV1::CanonicalZcash(Box::new(
            canonical.clone(),
        ))));
    active
        .observe_taker_first_lock()
        .await
        .expect("canonical taker lock");
    let plan = lez_first_lock_plan([0xf1; 32], vec![0x11], [0xf2; 32], vec![0x12]);
    active
        .drive_maker_lock(plan.clone())
        .await
        .expect("submit initialize");
    lez.2.observe_as(
        FirstLockStepV1::LezInitialize,
        confirmed_lez_initialize([0xf1; 32], "removed-maker-initialize"),
    );

    let removed = canonical_zcash_removal(&canonical);
    zcash
        .0
        .respond(Ok(TakerFirstLockObservationV1::ZcashRemoved(Box::new(
            removed.clone(),
        ))));
    assert!(matches!(
        active.drive_maker_lock(plan.clone()).await,
        Ok(MakerLockDriveOutcome::AwaitingEligibility(
            MakerFundingEligibilityOutcome::CanonicalStateChanged(_)
        ))
    ));
    assert_eq!((active.status(), active.revision()), (Phase::Offered, 2));
    assert_eq!(
        lez.2.submissions(),
        vec![(FirstLockStepV1::LezInitialize, vec![0x11])]
    );

    zcash.0.respond(Ok(TakerFirstLockObservationV1::Absent));
    assert_eq!(
        active
            .drive_maker_lock(plan.clone())
            .await
            .expect("absence cannot fund"),
        MakerLockDriveOutcome::AwaitingEligibility(
            MakerFundingEligibilityOutcome::AwaitingStableObservation(FirstLockStepV1::ZcashFund)
        )
    );
    assert_eq!(lez.2.submissions().len(), 1);

    let replacement = canonical_zcash_replacement(active.agreement(), &removed);
    zcash
        .0
        .respond(Ok(TakerFirstLockObservationV1::CanonicalZcash(Box::new(
            replacement.clone(),
        ))));
    assert!(matches!(
        active.drive_maker_lock(plan.clone()).await,
        Ok(MakerLockDriveOutcome::AwaitingEligibility(
            MakerFundingEligibilityOutcome::CanonicalStateChanged(_)
        ))
    ));
    assert_eq!(
        (active.status(), active.revision()),
        (Phase::TakerLockConfirmed, 3)
    );
    assert_eq!(lez.2.submissions().len(), 1);

    zcash
        .0
        .respond(Ok(TakerFirstLockObservationV1::CanonicalZcash(Box::new(
            replacement,
        ))));
    assert_eq!(
        active
            .drive_maker_lock(plan)
            .await
            .expect("replacement permits exact recovery"),
        MakerLockDriveOutcome::Lock(FirstLockDriveOutcome::Submitted(FirstLockStepV1::LezFund))
    );
    assert_eq!(
        lez.2.submissions(),
        vec![
            (FirstLockStepV1::LezInitialize, vec![0x11]),
            (FirstLockStepV1::LezFund, vec![0x12]),
        ]
    );
}

async fn assert_maker_happy_path_zcash_to_lez() {
    let forward_id = "sdk-maker-happy-zcash-to-lez";
    let forward_wire = agreement_wire(
        forward_id,
        SwapDirection::TakerSellsForeign,
        FixtureVariant::Local,
    );
    let forward_store = MemoryStore::default();
    let forward_lez = MemoryLezTakerLockObservation::default();
    let forward_zcash = MemoryZcashTakerLockObservation::default();
    let forward_sdk = maker_lock_sdk(
        forward_wire.clone(),
        forward_lez.clone(),
        forward_zcash.clone(),
        forward_store.clone(),
    );
    let accepted = forward_sdk
        .negotiate_at(&Offer(1), Proposal, ACCEPTED_AT)
        .await
        .expect("forward maker agreement");
    let mut forward = forward_sdk.activate(accepted).await.expect("activation");
    forward_zcash
        .0
        .respond(Ok(TakerFirstLockObservationV1::CanonicalZcash(Box::new(
            canonical_zcash_taker_lock(forward.agreement()),
        ))));
    assert!(matches!(
        forward.observe_taker_first_lock().await,
        Ok(ObserveTakerFirstLockOutcome::Projected(_))
    ));

    let forward_plan = FirstLockPlanV1::lez(
        PreparedFirstLockSubmissionV1::new(FirstLockStepV1::LezInitialize, [0x71; 32], vec![0xa1])
            .expect("LEZ initialize"),
        PreparedFirstLockSubmissionV1::new(FirstLockStepV1::LezFund, [0x72; 32], vec![0xa2])
            .expect("LEZ fund"),
    )
    .expect("maker LEZ plan");
    assert_eq!(
        forward
            .drive_maker_lock(forward_plan.clone())
            .await
            .expect("fresh eligibility stages then submits initialize"),
        MakerLockDriveOutcome::Lock(FirstLockDriveOutcome::Submitted(
            FirstLockStepV1::LezInitialize
        ))
    );
    assert_eq!(
        forward_store
            .maker_locks
            .lock()
            .expect("maker intent")
            .len(),
        1
    );
    assert_eq!(
        forward_lez.2.submissions(),
        vec![(FirstLockStepV1::LezInitialize, vec![0xa1])]
    );

    forward_lez.2.observe_as(
        FirstLockStepV1::LezInitialize,
        confirmed_lez_initialize([0x71; 32], "maker-lez-initialize"),
    );
    assert_eq!(
        forward
            .drive_maker_lock(forward_plan.clone())
            .await
            .expect("fresh eligibility submits LEZ fund"),
        MakerLockDriveOutcome::Lock(FirstLockDriveOutcome::Submitted(FirstLockStepV1::LezFund))
    );
    let forward_fund_evidence =
        confirmed_first_lock(FirstLockStepV1::LezFund, [0x72; 32], "maker-lez-fund");
    forward_lez.2.observe_as(
        FirstLockStepV1::LezFund,
        FirstLockObservation::Confirmed(forward_fund_evidence.clone()),
    );
    assert_eq!(
        forward
            .drive_maker_lock(forward_plan)
            .await
            .expect("both maker steps observed"),
        MakerLockDriveOutcome::Lock(FirstLockDriveOutcome::ReadyForFundingProjection(
            forward_fund_evidence.clone(),
        ))
    );
    forward
        .project_maker_lock(forward_fund_evidence)
        .await
        .expect("maker LEZ transition commits");
    assert_eq!(forward.status(), Phase::BothLegsLocked);
    assert_eq!(forward.revision(), 2);
    let reopened_forward = maker_lock_sdk(forward_wire, forward_lez, forward_zcash, forward_store)
        .resume(&SwapId::new(forward_id).expect("forward ID"))
        .await
        .expect("forward reopen")
        .expect("forward active");
    assert_eq!(reopened_forward.status(), Phase::BothLegsLocked);
}

async fn assert_maker_happy_path_lez_to_zcash() {
    let reverse_id = "sdk-maker-happy-lez-to-zcash";
    let reverse_wire = agreement_wire(
        reverse_id,
        SwapDirection::TakerSellsLez,
        FixtureVariant::Local,
    );
    let reverse_store = MemoryStore::default();
    let reverse_lez = MemoryLezTakerLockObservation::default();
    let reverse_zcash = MemoryZcashTakerLockObservation::default();
    let reverse_sdk = maker_lock_sdk(
        reverse_wire.clone(),
        reverse_lez.clone(),
        reverse_zcash.clone(),
        reverse_store.clone(),
    );
    let accepted = reverse_sdk
        .negotiate_at(&Offer(1), Proposal, ACCEPTED_AT)
        .await
        .expect("reverse maker agreement");
    let mut reverse = reverse_sdk.activate(accepted).await.expect("activation");
    reverse_lez
        .0
        .respond(Ok(TakerFirstLockObservationV1::CanonicalLez(Box::new(
            canonical_lez_taker_lock(reverse.agreement()),
        ))));
    assert!(matches!(
        reverse.observe_taker_first_lock().await,
        Ok(ObserveTakerFirstLockOutcome::Projected(_))
    ));
    let reverse_plan = zcash_first_lock_plan([0x81; 32], vec![0xb1]);
    assert_eq!(
        reverse
            .drive_maker_lock(reverse_plan)
            .await
            .expect("fresh eligibility stages then submits Zcash"),
        MakerLockDriveOutcome::Lock(FirstLockDriveOutcome::Submitted(FirstLockStepV1::ZcashFund))
    );
    assert_eq!(
        reverse_zcash.1.submissions(),
        vec![(FirstLockStepV1::ZcashFund, vec![0xb1])]
    );
    reverse_zcash.1.observe_as(
        FirstLockStepV1::ZcashFund,
        FirstLockObservation::Confirmed(confirmed_zcash_first_lock([0x81; 32], "maker-zcash-fund")),
    );
    reverse
        .project_maker_lock(confirmed_zcash_first_lock([0x81; 32], "maker-zcash-fund"))
        .await
        .expect("maker Zcash transition commits");
    assert_eq!(reverse.status(), Phase::BothLegsLocked);
    let reopened_reverse = maker_lock_sdk(reverse_wire, reverse_lez, reverse_zcash, reverse_store)
        .resume(&SwapId::new(reverse_id).expect("reverse ID"))
        .await
        .expect("reverse reopen")
        .expect("reverse active");
    assert_eq!(reopened_reverse.status(), Phase::BothLegsLocked);
}

async fn assert_forward_canonical_history_survives_restart(
    forward_id: &str,
    forward_wire: Vec<u8>,
    forward_lez: MemoryLezTakerLockObservation,
    forward_zcash: MemoryZcashTakerLockObservation,
    forward_store: MemoryStore,
    canonical: CanonicalZcashOutputObservation,
) {
    let reorg_zcash = forward_zcash.clone();
    let _ = take_zcash_previous_heads(&reorg_zcash);
    let mut restarted = ZecPairSdk::new(
        Participant::Maker,
        MemoryDiscovery::default(),
        MemoryNegotiation { wire: forward_wire },
        forward_lez,
        forward_zcash,
        forward_store,
    )
    .resume(&SwapId::new(forward_id).expect("id"))
    .await
    .expect("maker restart reads only its independent recovery store")
    .expect("durable agreement and observed transition");
    assert_eq!(restarted.status(), Phase::TakerLockConfirmed);
    assert_eq!(restarted.revision(), 1);
    assert_eq!(
        restarted.next_action(),
        ZecLifecycleAction::Wait,
        "replayed observation requires a fresh canonical eligibility check"
    );
    assert_fresh_forward_eligibility(&mut restarted, 1).await;
    assert_eq!(
        take_zcash_previous_heads(&reorg_zcash),
        vec![Some(canonical.clone())],
        "restart replay supplies the durable canonical head for an unchanged poll"
    );
    assert_forward_reorg_history(&mut restarted, &reorg_zcash, canonical).await;
}

async fn assert_forward_reorg_history(
    restarted: &mut ActiveZecSwap<
        MemoryLezTakerLockObservation,
        MemoryZcashTakerLockObservation,
        MemoryStore,
    >,
    reorg_zcash: &MemoryZcashTakerLockObservation,
    canonical: CanonicalZcashOutputObservation,
) {
    let removed = canonical_zcash_removal(&canonical);
    let mismatched_replacement =
        canonical_zcash_taker_lock_with_input(restarted.agreement(), [8; 32]);
    reorg_zcash
        .0
        .respond(Ok(TakerFirstLockObservationV1::ZcashReplaced {
            removed: Box::new(removed.clone()),
            canonical: Box::new(mismatched_replacement),
        }));
    assert!(
        restarted.observe_taker_first_lock().await.is_err(),
        "replacement halves from different stable tips must fail"
    );
    assert_eq!(
        take_zcash_previous_heads(reorg_zcash),
        vec![Some(canonical.clone())],
        "replacement validation receives the durable canonical predecessor"
    );
    assert_eq!(restarted.revision(), 1);
    let replacement = canonical_zcash_replacement(restarted.agreement(), &removed);
    reorg_zcash
        .0
        .respond(Ok(TakerFirstLockObservationV1::ZcashReplaced {
            removed: Box::new(removed),
            canonical: Box::new(replacement.clone()),
        }));
    let replaced = restarted
        .observe_taker_first_lock()
        .await
        .expect("atomic canonical replacement commits");
    assert!(matches!(
        replaced,
        ObserveTakerFirstLockOutcome::Projected(_)
    ));
    assert_eq!(restarted.status(), Phase::TakerLockConfirmed);
    assert_eq!(restarted.revision(), 2);
    assert_eq!(
        take_zcash_previous_heads(reorg_zcash),
        vec![Some(canonical)],
        "accepted replacement receives the same durable predecessor"
    );

    reorg_zcash
        .0
        .respond(Ok(TakerFirstLockObservationV1::CanonicalZcash(Box::new(
            replacement,
        ))));
    assert_fresh_forward_eligibility(restarted, 2).await;

    let deeper = canonical_zcash_replacement_depth_update(restarted.agreement());
    reorg_zcash
        .0
        .respond(Ok(TakerFirstLockObservationV1::CanonicalZcash(Box::new(
            deeper.clone(),
        ))));
    // The same lock seen deeper is projected durably (revision 3) and the
    // Maker is eligible from that head at once: depth is progress of the lock
    // it knows, not a changed canonical state (a changed state is a removal or
    // a replacement, below).
    assert_eq!(
        restarted
            .refresh_maker_funding_eligibility()
            .await
            .expect("changed depth commits before eligibility"),
        MakerFundingEligibilityOutcome::Eligible { revision: 3 }
    );
    assert_eq!(restarted.status(), Phase::TakerLockConfirmed);
    assert_eq!(restarted.revision(), 3);

    let _ = take_zcash_previous_heads(reorg_zcash);
    reorg_zcash
        .0
        .respond(Ok(TakerFirstLockObservationV1::ZcashRemoved(Box::new(
            canonical_zcash_removal(&deeper),
        ))));
    let removal = restarted
        .observe_taker_first_lock()
        .await
        .expect("canonical removal commits");
    assert!(matches!(
        removal,
        ObserveTakerFirstLockOutcome::Projected(_)
    ));
    assert_eq!(restarted.status(), Phase::Offered);
    assert_eq!(restarted.revision(), 4);
    assert_eq!(restarted.next_action(), ZecLifecycleAction::Wait);
    assert_eq!(
        take_zcash_previous_heads(reorg_zcash),
        vec![Some(deeper)],
        "removal validation receives the latest durable canonical predecessor"
    );
}

async fn assert_fresh_forward_eligibility(
    maker: &mut ActiveZecSwap<
        MemoryLezTakerLockObservation,
        MemoryZcashTakerLockObservation,
        MemoryStore,
    >,
    revision: u64,
) {
    assert_eq!(
        maker
            .refresh_maker_funding_eligibility()
            .await
            .expect("fresh exact canonical head is eligible"),
        MakerFundingEligibilityOutcome::Eligible { revision }
    );
    assert_eq!(
        maker.next_action(),
        ZecLifecycleAction::Wait,
        "eligibility is ephemeral and cannot authorize a missing maker effect"
    );
}

async fn assert_noneligible_fresh_polls(
    maker: &mut ActiveZecSwap<
        MemoryLezTakerLockObservation,
        MemoryZcashTakerLockObservation,
        MemoryStore,
    >,
    zcash: &MemoryZcashTakerLockObservation,
    store: &MemoryStore,
    canonical: &CanonicalZcashOutputObservation,
) {
    let transition_count = store
        .observed_taker_first_lock_transitions
        .lock()
        .expect("transition lock")
        .len();
    for observation in [
        TakerFirstLockObservationV1::Absent,
        TakerFirstLockObservationV1::Unstable,
    ] {
        zcash.0.respond(Ok(observation));
        assert_eq!(
            maker
                .refresh_maker_funding_eligibility()
                .await
                .expect("noneligible poll is not an error"),
            MakerFundingEligibilityOutcome::AwaitingStableObservation(FirstLockStepV1::ZcashFund)
        );
        assert_eq!(maker.status(), Phase::TakerLockConfirmed);
        assert_eq!(maker.revision(), 1);
        assert_eq!(maker.next_action(), ZecLifecycleAction::Wait);
        assert_eq!(
            store
                .observed_taker_first_lock_transitions
                .lock()
                .expect("transition lock")
                .len(),
            transition_count
        );
    }
    zcash
        .0
        .respond(Ok(TakerFirstLockObservationV1::CanonicalZcash(Box::new(
            canonical.clone(),
        ))));
}

async fn assert_forward_observation_does_not_advance(
    maker: &mut ActiveZecSwap<
        MemoryLezTakerLockObservation,
        MemoryZcashTakerLockObservation,
        MemoryStore,
    >,
    lez: &MemoryLezTakerLockObservation,
    zcash: &MemoryZcashTakerLockObservation,
    store: &MemoryStore,
) {
    assert_eq!(
        maker
            .observe_taker_first_lock()
            .await
            .expect("stable absence is not a state transition"),
        ObserveTakerFirstLockOutcome::AwaitingStableObservation(FirstLockStepV1::ZcashFund)
    );
    assert_eq!(zcash.0.calls(), 1);
    assert_eq!(
        take_zcash_previous_heads(zcash),
        vec![None],
        "the first observation has no durable canonical predecessor"
    );
    assert_eq!(lez.0.calls(), 0);
    assert_eq!(maker.status(), Phase::Offered);
    assert!(
        store
            .observed_taker_first_lock_transitions
            .lock()
            .expect("transition lock")
            .is_empty()
    );

    zcash.0.respond(Ok(TakerFirstLockObservationV1::Unstable));
    assert!(matches!(
        maker.observe_taker_first_lock().await,
        Ok(ObserveTakerFirstLockOutcome::AwaitingStableObservation(
            FirstLockStepV1::ZcashFund
        ))
    ));
    zcash
        .0
        .respond(Err(TestPortError("fresh RPC query failed".to_owned())));
    assert!(maker.observe_taker_first_lock().await.is_err());
    assert_eq!(maker.status(), Phase::Offered);

    zcash.0.respond(Ok(confirmed_observed_taker_lock(
        FirstLockStepV1::LezFund,
        "wrong-chain-lock",
        100,
    )));
    assert!(matches!(
        maker.observe_taker_first_lock().await,
        Err(ZecSdkError::InvalidObservedTakerFirstLockTransition(_))
    ));
    assert!(
        ObservedTakerFirstLockEvidenceV1::new(
            FirstLockStepV1::ZcashFund,
            "zero-confirmation-lock",
            0,
        )
        .is_err()
    );
    assert_eq!(maker.revision(), 0);
}

#[tokio::test]
async fn reverse_lez_opaque_evidence_is_rebound_to_the_active_agreement() {
    let store = MemoryStore::default();
    let lez = MemoryLezTakerLockObservation::default();
    let active_sdk = ZecPairSdk::new(
        Participant::Maker,
        MemoryDiscovery::default(),
        MemoryNegotiation {
            wire: agreement_wire(
                "sdk-active-lez-binding",
                SwapDirection::TakerSellsLez,
                FixtureVariant::Local,
            ),
        },
        lez.clone(),
        MemoryZcashTakerLockObservation::default(),
        store.clone(),
    );
    let accepted = active_sdk
        .negotiate_at(&Offer(1), Proposal, ACCEPTED_AT)
        .await
        .expect("active reverse agreement");
    let mut active = active_sdk.activate(accepted).await.expect("activation");

    let foreign_sdk = sdk(
        Participant::Maker,
        agreement_wire(
            "sdk-foreign-lez-binding",
            SwapDirection::TakerSellsLez,
            FixtureVariant::Local,
        ),
        MemoryStore::default(),
    );
    let foreign = foreign_sdk
        .negotiate_at(&Offer(1), Proposal, ACCEPTED_AT)
        .await
        .expect("foreign reverse agreement");
    let canonical = canonical_lez_taker_lock(foreign.agreement());
    let removed = canonical_lez_removal_for(foreign.agreement(), &canonical, [0xbb; 32], 104);
    let replacement = CanonicalLezEscrowObservationV1::validate(
        foreign.agreement(),
        &canonical_lez_snapshot(foreign.agreement(), LezSnapshotMutation::ReplacementSafe),
    )
    .expect("foreign replacement");
    for observation in [
        TakerFirstLockObservationV1::CanonicalLez(Box::new(canonical)),
        TakerFirstLockObservationV1::LezRemoved(Box::new(removed.clone())),
        TakerFirstLockObservationV1::LezReplaced {
            removed: Box::new(removed),
            canonical: Box::new(replacement),
        },
    ] {
        lez.0.respond(Ok(observation));
        assert!(matches!(
            active.observe_taker_first_lock().await,
            Err(ZecSdkError::InvalidObservedTakerFirstLockTransition(
                ObservedTakerFirstLockTransitionError::CanonicalLezBindingMismatch
            ))
        ));
        assert_eq!(active.revision(), 0);
        assert!(
            store
                .observed_taker_first_lock_transitions
                .lock()
                .expect("maker journal")
                .is_empty()
        );
    }
}

#[tokio::test]
async fn reverse_maker_observes_lez_while_taker_cannot_use_maker_observation() {
    let reverse_store = MemoryStore::default();
    let reverse_lez = MemoryLezTakerLockObservation::default();
    let reverse_zcash = MemoryZcashTakerLockObservation::default();
    let reverse_sdk = ZecPairSdk::new(
        Participant::Maker,
        MemoryDiscovery::default(),
        MemoryNegotiation {
            wire: agreement_wire(
                "sdk-maker-observes-lez",
                SwapDirection::TakerSellsLez,
                FixtureVariant::Local,
            ),
        },
        reverse_lez.clone(),
        reverse_zcash.clone(),
        reverse_store.clone(),
    );
    let reverse_accepted = reverse_sdk
        .negotiate_at(&Offer(1), Proposal, ACCEPTED_AT)
        .await
        .expect("reverse agreement");
    let mut reverse_maker = reverse_sdk
        .activate(reverse_accepted.clone())
        .await
        .expect("reverse maker activation");
    reverse_lez.0.respond(Ok(confirmed_observed_taker_lock(
        FirstLockStepV1::LezFund,
        "primitive-lez-lock",
        100,
    )));
    assert!(matches!(
        reverse_maker.observe_taker_first_lock().await,
        Err(ZecSdkError::InvalidObservedTakerFirstLockTransition(_))
    ));
    assert_eq!(reverse_maker.revision(), 0);
    let initial_lez = canonical_lez_taker_lock(reverse_maker.agreement());
    let initial_lez_txid = *initial_lez.transaction_id();
    reverse_lez
        .0
        .respond(Ok(TakerFirstLockObservationV1::CanonicalLez(Box::new(
            initial_lez.clone(),
        ))));
    assert!(matches!(
        reverse_maker.observe_taker_first_lock().await,
        Ok(ObserveTakerFirstLockOutcome::Projected(_))
    ));
    assert_eq!(reverse_lez.0.calls(), 2);
    assert_eq!(reverse_zcash.0.calls(), 0);
    assert_eq!(reverse_maker.status(), Phase::TakerLockConfirmed);
    let durable_transition = reverse_store
        .observed_taker_first_lock_transitions
        .lock()
        .expect("reverse observation journal")
        .get(&("sdk-maker-observes-lez".to_owned(), 0))
        .expect("canonical LEZ transition committed")
        .clone();
    let record = ObservedTakerFirstLockTransitionRecordV1::from(&durable_transition);
    record
        .revalidate(&reverse_accepted, 0)
        .expect("canonical LEZ primitive record survives restart revalidation");
    reverse_lez
        .0
        .respond(Ok(TakerFirstLockObservationV1::CanonicalLez(Box::new(
            initial_lez,
        ))));
    assert_fresh_reverse_eligibility(&mut reverse_maker, &reverse_lez, 1, 3).await;
    let deeper_finalized = deeper_finalized_lez(reverse_maker.agreement());
    reverse_lez
        .0
        .respond(Ok(TakerFirstLockObservationV1::CanonicalLez(Box::new(
            deeper_finalized,
        ))));
    assert!(matches!(
        reverse_maker.observe_taker_first_lock().await,
        Ok(ObserveTakerFirstLockOutcome::Projected(_))
    ));
    assert_eq!(reverse_maker.revision(), 2);
    assert_eq!(reverse_lez.0.calls(), 4);
    assert_eq!(
        *reverse_lez.1.lock().expect("LEZ previous-head history"),
        vec![None, None, Some(initial_lez_txid), Some(initial_lez_txid)]
    );
    assert_eq!(
        reverse_maker.next_action(),
        ZecLifecycleAction::Wait,
        "canonical LEZ evidence must not bypass the distinct fresh-funding check"
    );
    assert_fresh_reverse_eligibility(&mut reverse_maker, &reverse_lez, 2, 5).await;
    assert_unstable_reverse_is_not_eligible(&mut reverse_maker, &reverse_lez, 2, 6).await;

    assert_taker_cannot_observe_for_maker().await;
}

async fn assert_fresh_reverse_eligibility(
    maker: &mut ActiveZecSwap<
        MemoryLezTakerLockObservation,
        MemoryZcashTakerLockObservation,
        MemoryStore,
    >,
    lez: &MemoryLezTakerLockObservation,
    revision: u64,
    expected_calls: usize,
) {
    assert_eq!(
        maker
            .refresh_maker_funding_eligibility()
            .await
            .expect("fresh exact LEZ head is eligible"),
        MakerFundingEligibilityOutcome::Eligible { revision }
    );
    assert_eq!(lez.0.calls(), expected_calls);
    assert_eq!(maker.next_action(), ZecLifecycleAction::Wait);
}

async fn assert_unstable_reverse_is_not_eligible(
    maker: &mut ActiveZecSwap<
        MemoryLezTakerLockObservation,
        MemoryZcashTakerLockObservation,
        MemoryStore,
    >,
    lez: &MemoryLezTakerLockObservation,
    revision: u64,
    expected_calls: usize,
) {
    lez.0.respond(Ok(TakerFirstLockObservationV1::Unstable));
    assert_eq!(
        maker
            .refresh_maker_funding_eligibility()
            .await
            .expect("unstable poll cannot reuse earlier eligibility"),
        MakerFundingEligibilityOutcome::AwaitingStableObservation(FirstLockStepV1::LezFund)
    );
    assert_eq!(maker.revision(), revision);
    assert_eq!(lez.0.calls(), expected_calls);
    assert_eq!(maker.next_action(), ZecLifecycleAction::Wait);
}

async fn assert_taker_cannot_observe_for_maker() {
    let taker_sdk = ZecPairSdk::new(
        Participant::Taker,
        MemoryDiscovery::default(),
        MemoryNegotiation {
            wire: agreement_wire(
                "sdk-taker-cannot-observe-for-maker",
                SwapDirection::TakerSellsForeign,
                FixtureVariant::Local,
            ),
        },
        MemoryLezTakerLockObservation::default(),
        MemoryZcashTakerLockObservation::default(),
        MemoryStore::default(),
    );
    let taker_accepted = taker_sdk
        .negotiate_at(&Offer(1), Proposal, ACCEPTED_AT)
        .await
        .expect("taker agreement");
    let mut taker = taker_sdk
        .activate(taker_accepted)
        .await
        .expect("activation");
    assert!(matches!(
        taker.observe_taker_first_lock().await,
        Err(ZecSdkError::WrongRole {
            expected: Participant::Maker,
            actual: Participant::Taker
        })
    ));
}

#[test]
fn canonical_lez_observation_uses_the_direction_derived_depositor() {
    for (direction, swap_id) in [
        (
            SwapDirection::TakerSellsLez,
            "canonical-lez-taker-depositor",
        ),
        (
            SwapDirection::TakerSellsForeign,
            "canonical-lez-maker-depositor",
        ),
    ] {
        let agreement = lez_zec_swap_sdk::ZecAgreementV1::from_wire_at(
            &agreement_wire(swap_id, direction, FixtureVariant::Local),
            ACCEPTED_AT,
        )
        .expect("valid direction-specific agreement");
        CanonicalLezEscrowObservationV1::validate(
            &agreement,
            &canonical_lez_snapshot(&agreement, LezSnapshotMutation::Pending),
        )
        .expect("agreement-derived depositor signs the canonical LEZ fund");
        assert!(matches!(
            CanonicalLezEscrowObservationV1::validate(
                &agreement,
                &canonical_lez_snapshot(&agreement, LezSnapshotMutation::Signer),
            ),
            Err(lez_zec_swap_sdk::LezObservationError::TransactionBindingMismatch)
        ));
    }
}

#[test]
fn canonical_lez_observation_rejects_each_changed_chain_transaction_and_account_binding() {
    let agreement = lez_zec_swap_sdk::ZecAgreementV1::from_wire_at(
        &agreement_wire(
            "canonical-lez-negative-matrix",
            SwapDirection::TakerSellsLez,
            FixtureVariant::Local,
        ),
        ACCEPTED_AT,
    )
    .expect("valid reverse agreement");
    for mutation in [
        LezSnapshotMutation::Environment,
        LezSnapshotMutation::Channel,
        LezSnapshotMutation::Genesis,
        LezSnapshotMutation::TipDrift,
        LezSnapshotMutation::InclusionBlock,
        LezSnapshotMutation::InclusionAboveTip,
        LezSnapshotMutation::Program,
        LezSnapshotMutation::Instruction,
        LezSnapshotMutation::Signer,
        LezSnapshotMutation::Accounts,
        LezSnapshotMutation::SwapId,
        LezSnapshotMutation::NonPublic,
        LezSnapshotMutation::InvalidSignature,
        LezSnapshotMutation::MetadataOwner,
        LezSnapshotMutation::MetadataAccount,
        LezSnapshotMutation::MetadataAmount,
        LezSnapshotMutation::CustodyAccount,
        LezSnapshotMutation::CustodyAmount,
    ] {
        assert!(
            CanonicalLezEscrowObservationV1::validate(
                &agreement,
                &canonical_lez_snapshot(&agreement, mutation)
            )
            .is_err()
        );
    }
}

#[test]
// One linear scenario keeps proposal/commit, finality, replacement, and stale
// evidence assertions visibly tied to the same durable tracker head.
#[allow(clippy::too_many_lines)]
fn lez_tracker_requires_durable_exact_head_events_and_preserves_finality() {
    let agreement = lez_zec_swap_sdk::ZecAgreementV1::from_wire_at(
        &agreement_wire(
            "canonical-lez-tracker",
            SwapDirection::TakerSellsLez,
            FixtureVariant::Local,
        ),
        ACCEPTED_AT,
    )
    .expect("valid reverse agreement");
    let canonical = |mutation| {
        CanonicalLezEscrowObservationV1::validate(
            &agreement,
            &canonical_lez_snapshot(&agreement, mutation),
        )
        .expect("valid tracker observation")
    };
    let first = canonical(LezSnapshotMutation::Pending);
    let deeper_safe = canonical(LezSnapshotMutation::DeeperSafe);
    let deeper_finalized = canonical(LezSnapshotMutation::DeeperFinalized);
    let replacement = canonical(LezSnapshotMutation::ReplacementSafe);
    let regressed_safe = canonical(LezSnapshotMutation::TipRegressionSafe);
    let mut tracker = LezObservationTrackerV1::default();

    let first_event = LezObservationEventV1::Canonical(first.clone());
    assert_eq!(
        tracker
            .propose(&LezObservationReconciliationV1::Canonical(first.clone()))
            .expect("first proposal"),
        Some(first_event.clone())
    );
    assert_eq!(tracker.current(), None, "proposal is not a durable commit");
    tracker
        .apply_committed(&first_event)
        .expect("first durable commit");
    assert_eq!(
        tracker
            .propose(&LezObservationReconciliationV1::Canonical(first.clone()))
            .expect("duplicate poll"),
        None
    );

    let safe_event = tracker
        .propose(&LezObservationReconciliationV1::Canonical(
            deeper_safe.clone(),
        ))
        .expect("same-inclusion safe promotion")
        .expect("safe promotion journals");
    tracker
        .apply_committed(&safe_event)
        .expect("safe promotion commit");
    assert_eq!(
        tracker.propose(&LezObservationReconciliationV1::Canonical(regressed_safe)),
        Err(LezObservationTrackerError::TipRegression)
    );
    let finalized_event = tracker
        .propose(&LezObservationReconciliationV1::Canonical(
            deeper_finalized.clone(),
        ))
        .expect("same-inclusion depth/finality promotion")
        .expect("promotion journals");
    tracker
        .apply_committed(&finalized_event)
        .expect("finality promotion commit");
    assert_eq!(
        tracker.propose(&LezObservationReconciliationV1::Canonical(first.clone())),
        Err(LezObservationTrackerError::FinalityRegression)
    );
    assert_eq!(
        CanonicalLezEscrowRemovalV1::validate(
            &deeper_finalized,
            &LezNodeRemovalSnapshotV1::new(
                agreement.lez_terms().chain().environment(),
                *agreement.lez_terms().chain().channel_id(),
                *agreement.lez_terms().chain().genesis_block_hash(),
                [0x61; 32],
                LezStableTipV1::new([0xbb; 32], 104, [0xbb; 32], 104),
            ),
        ),
        Err(lez_zec_swap_sdk::LezObservationError::FinalizedHistoryViolation)
    );

    let mut replaceable = LezObservationTrackerV1::from_current(Some(deeper_safe.clone()));
    assert_eq!(
        replaceable.propose(&LezObservationReconciliationV1::Canonical(
            replacement.clone()
        )),
        Err(LezObservationTrackerError::ReplacementProofRequired)
    );
    assert_eq!(
        CanonicalLezEscrowRemovalV1::validate(
            &deeper_safe,
            &LezNodeRemovalSnapshotV1::new(
                agreement.lez_terms().chain().environment(),
                *agreement.lez_terms().chain().channel_id(),
                *agreement.lez_terms().chain().genesis_block_hash(),
                [0x61; 32],
                LezStableTipV1::new([0xba; 32], 102, [0xba; 32], 102),
            ),
        ),
        Err(lez_zec_swap_sdk::LezObservationError::TipRegression)
    );
    let mismatched_tip_removal = CanonicalLezEscrowRemovalV1::validate(
        &deeper_safe,
        &LezNodeRemovalSnapshotV1::new(
            agreement.lez_terms().chain().environment(),
            *agreement.lez_terms().chain().channel_id(),
            *agreement.lez_terms().chain().genesis_block_hash(),
            [0x61; 32],
            LezStableTipV1::new([0xbc; 32], 105, [0xbc; 32], 105),
        ),
    )
    .expect("valid removal from a different stable tip");
    assert_eq!(
        replaceable.propose(&LezObservationReconciliationV1::Replaced {
            removed: Box::new(mismatched_tip_removal),
            canonical: Box::new(replacement.clone()),
        }),
        Err(LezObservationTrackerError::ReplacementTipMismatch)
    );
    let removed = CanonicalLezEscrowRemovalV1::validate(
        &deeper_safe,
        &LezNodeRemovalSnapshotV1::new(
            agreement.lez_terms().chain().environment(),
            *agreement.lez_terms().chain().channel_id(),
            *agreement.lez_terms().chain().genesis_block_hash(),
            [0x61; 32],
            LezStableTipV1::new([0xbb; 32], 104, [0xbb; 32], 104),
        ),
    )
    .expect("affirmative nonfinal removal");
    let replacement_event = LezObservationEventV1::Replaced {
        removed: Box::new(removed.clone()),
        canonical: Box::new(replacement.clone()),
    };
    assert_eq!(
        replaceable
            .propose(&LezObservationReconciliationV1::Replaced {
                removed: Box::new(removed.clone()),
                canonical: Box::new(replacement.clone()),
            })
            .expect("atomic replacement"),
        Some(replacement_event.clone())
    );
    replaceable
        .apply_committed(&replacement_event)
        .expect("replacement commit");
    assert_eq!(replaceable.current(), Some(&replacement));
    assert_eq!(
        replaceable.propose(&LezObservationReconciliationV1::Removed(removed)),
        Err(LezObservationTrackerError::StaleEvidence)
    );
}

#[tokio::test]
async fn durable_first_lock_records_revalidate_primitive_payloads_and_closed_intent() {
    let swap_id = "sdk-first-lock-records";
    let store = MemoryStore::default();
    let sdk = first_lock_sdk(
        Participant::Taker,
        agreement_wire(
            swap_id,
            SwapDirection::TakerSellsForeign,
            FixtureVariant::Local,
        ),
        MemoryLezPort::default(),
        MemoryZcashPort::default(),
        store.clone(),
    );
    let accepted = sdk
        .negotiate_at(&Offer(1), Proposal, ACCEPTED_AT)
        .await
        .expect("agreement");
    let mut active = sdk.activate(accepted.clone()).await.expect("activation");
    active
        .stage_first_lock(zcash_first_lock_plan([0x41; 32], vec![0x51, 0x52]))
        .await
        .expect("intent");
    let trusted_intent = store
        .first_locks
        .lock()
        .expect("intent lock")
        .get(swap_id)
        .cloned()
        .expect("retained intent");
    let intent_record = FirstLockIntentRecordV1::from(&trusted_intent);
    assert_eq!(
        FirstLockIntentRecordV1::from(
            &intent_record
                .revalidate(&accepted, 0)
                .expect("intent revalidates")
        ),
        intent_record
    );

    assert_invalid_intent_records(&accepted, &intent_record);

    active
        .project_first_lock(confirmed_zcash_first_lock([0x41; 32], "record-first-lock"))
        .await
        .expect("transition");
    let trusted_transition = store
        .first_lock_transitions
        .lock()
        .expect("transition lock")
        .get(&(swap_id.to_owned(), 0))
        .cloned()
        .expect("retained transition");
    let transition_record = FirstLockTransitionRecordV1::from(&trusted_transition);
    assert_eq!(
        FirstLockTransitionRecordV1::from(
            &transition_record
                .revalidate(&accepted, &intent_record, 0)
                .expect("transition and closed intent revalidate")
        ),
        transition_record
    );

    assert_invalid_transition_contexts(&accepted, &intent_record, &transition_record);
    assert_invalid_transition_evidence(&accepted, &intent_record, &transition_record);
}

fn assert_invalid_intent_records(
    accepted: &AcceptedZecAgreementV1,
    intent_record: &FirstLockIntentRecordV1,
) {
    let mutate = |field: &str, value: serde_json::Value| {
        let mut json = serde_json::to_value(intent_record).expect("intent JSON");
        json[field] = value;
        serde_json::from_value::<FirstLockIntentRecordV1>(json).expect("mutated intent record")
    };
    let future = mutate("schema_version", serde_json::json!(2));
    assert!(matches!(
        future.revalidate(accepted, 0),
        Err(FirstLockRecordError::UnsupportedSchema { actual: 2, .. })
    ));
    let wrong_role = mutate("local_participant", serde_json::json!("maker"));
    assert!(matches!(
        wrong_role.revalidate(accepted, 0),
        Err(FirstLockRecordError::RoleMismatch)
    ));
    let wrong_swap = mutate("swap_id", serde_json::json!("wrong-swap"));
    assert!(matches!(
        wrong_swap.revalidate(accepted, 0),
        Err(FirstLockRecordError::SwapIdMismatch)
    ));
    let wrong_revision = mutate("predecessor_revision", serde_json::json!(1));
    assert!(matches!(
        wrong_revision.revalidate(accepted, 0),
        Err(FirstLockRecordError::RevisionMismatch)
    ));

    let mut wrong_commitment = serde_json::to_value(intent_record).expect("intent JSON");
    wrong_commitment["agreement_commitment"][0] = serde_json::json!(0xff);
    let wrong_commitment = serde_json::from_value::<FirstLockIntentRecordV1>(wrong_commitment)
        .expect("wrong commitment record");
    assert!(matches!(
        wrong_commitment.revalidate(accepted, 0),
        Err(FirstLockRecordError::AgreementCommitmentMismatch)
    ));

    let mut oversized = serde_json::to_value(intent_record).expect("intent JSON");
    oversized["plan"]["funding"]["exact_submission"] =
        serde_json::to_value(vec![0_u8; MAX_FIRST_LOCK_SUBMISSION_BYTES + 1])
            .expect("oversized primitive bytes");
    let oversized = serde_json::from_value::<FirstLockIntentRecordV1>(oversized)
        .expect("oversized primitive record");
    assert!(matches!(
        oversized.revalidate(accepted, 0),
        Err(FirstLockRecordError::Intent(_))
    ));

    let mut wrong_plan = serde_json::to_value(intent_record).expect("intent JSON");
    let initialize = wrong_plan["plan"]["funding"].take();
    let mut fund = initialize.clone();
    fund["step"] = serde_json::json!("lez_fund");
    fund["expected_submission_id"][0] = serde_json::json!(0xfc);
    let mut initialize = initialize;
    initialize["step"] = serde_json::json!("lez_initialize");
    wrong_plan["plan"] = serde_json::json!({
        "plan": "lez",
        "initialize": initialize,
        "fund": fund
    });
    let wrong_plan =
        serde_json::from_value::<FirstLockIntentRecordV1>(wrong_plan).expect("wrong plan record");
    assert!(matches!(
        wrong_plan.revalidate(accepted, 0),
        Err(FirstLockRecordError::Intent(
            lez_zec_swap_sdk::FirstLockIntentError::WrongPlanForDirection(_)
        ))
    ));
}

fn assert_invalid_transition_contexts(
    accepted: &AcceptedZecAgreementV1,
    intent_record: &FirstLockIntentRecordV1,
    transition_record: &FirstLockTransitionRecordV1,
) {
    let mutate = |field: &str, value: serde_json::Value| {
        let mut json = serde_json::to_value(transition_record).expect("transition JSON");
        json[field] = value;
        serde_json::from_value::<FirstLockTransitionRecordV1>(json)
            .expect("mutated transition record")
    };
    let future = mutate("schema_version", serde_json::json!(2));
    assert!(matches!(
        future.revalidate(accepted, intent_record, 0),
        Err(FirstLockRecordError::UnsupportedSchema { actual: 2, .. })
    ));
    let wrong_role = mutate("local_participant", serde_json::json!("maker"));
    assert!(matches!(
        wrong_role.revalidate(accepted, intent_record, 0),
        Err(FirstLockRecordError::RoleMismatch)
    ));
    let wrong_swap = mutate("swap_id", serde_json::json!("wrong-swap"));
    assert!(matches!(
        wrong_swap.revalidate(accepted, intent_record, 0),
        Err(FirstLockRecordError::SwapIdMismatch)
    ));
    let wrong_revision = mutate("predecessor_revision", serde_json::json!(1));
    assert!(matches!(
        wrong_revision.revalidate(accepted, intent_record, 0),
        Err(FirstLockRecordError::RevisionMismatch)
    ));
    let mut wrong_commitment = serde_json::to_value(transition_record).expect("transition JSON");
    wrong_commitment["agreement_commitment"][0] = serde_json::json!(0xfb);
    let wrong_commitment = serde_json::from_value::<FirstLockTransitionRecordV1>(wrong_commitment)
        .expect("wrong-commitment transition record");
    assert!(matches!(
        wrong_commitment.revalidate(accepted, intent_record, 0),
        Err(FirstLockRecordError::AgreementCommitmentMismatch)
    ));
}

fn assert_invalid_transition_evidence(
    accepted: &AcceptedZecAgreementV1,
    intent_record: &FirstLockIntentRecordV1,
    transition_record: &FirstLockTransitionRecordV1,
) {
    let mut wrong_step = serde_json::to_value(transition_record).expect("transition JSON");
    wrong_step["evidence"]["step"] = serde_json::json!("lez_fund");
    let wrong_step = serde_json::from_value::<FirstLockTransitionRecordV1>(wrong_step)
        .expect("wrong-step transition record");
    assert!(matches!(
        wrong_step.revalidate(accepted, intent_record, 0),
        Err(FirstLockRecordError::Transition(_))
    ));

    let mut wrong_identity = serde_json::to_value(transition_record).expect("transition JSON");
    wrong_identity["evidence"]["expected_submission_id"][0] = serde_json::json!(0xfe);
    let wrong_identity = serde_json::from_value::<FirstLockTransitionRecordV1>(wrong_identity)
        .expect("wrong-identity transition record");
    assert!(matches!(
        wrong_identity.revalidate(accepted, intent_record, 0),
        Err(FirstLockRecordError::Transition(_))
    ));

    let mut insufficient = serde_json::to_value(transition_record).expect("transition JSON");
    insufficient["evidence"]["confirmations"] = serde_json::json!(0);
    let insufficient = serde_json::from_value::<FirstLockTransitionRecordV1>(insufficient)
        .expect("zero-confirmation transition record");
    assert!(matches!(
        insufficient.revalidate(accepted, intent_record, 0),
        Err(FirstLockRecordError::Transition(
            lez_zec_swap_sdk::FirstLockTransitionError::ZeroConfirmations
        ))
    ));

    let mut corrupt_closed = serde_json::to_value(intent_record).expect("intent JSON");
    corrupt_closed["agreement_commitment"][0] = serde_json::json!(0xfd);
    let corrupt_closed = serde_json::from_value::<FirstLockIntentRecordV1>(corrupt_closed)
        .expect("corrupt closed intent record");
    assert!(matches!(
        transition_record.revalidate(accepted, &corrupt_closed, 0),
        Err(FirstLockRecordError::ClosedIntent(_))
    ));
}

#[tokio::test]
async fn activation_is_idempotent_for_exact_replay_and_conflicts_on_changed_same_key() {
    let store = MemoryStore::default();
    let first = sdk(
        Participant::Maker,
        agreement_wire(
            "sdk-idempotent",
            SwapDirection::TakerSellsForeign,
            FixtureVariant::Local,
        ),
        store.clone(),
    );
    let accepted = first
        .negotiate_at(&Offer(1), Proposal, ACCEPTED_AT)
        .await
        .expect("agreement");
    first
        .activate(accepted.clone())
        .await
        .expect("first create");
    first.activate(accepted).await.expect("exact replay");

    let changed = sdk(
        Participant::Maker,
        agreement_wire(
            "sdk-idempotent",
            SwapDirection::TakerSellsForeign,
            FixtureVariant::ChangedTranscript,
        ),
        store,
    );
    let changed_terms = changed
        .negotiate_at(&Offer(1), Proposal, ACCEPTED_AT)
        .await
        .expect("changed record remains internally valid");
    assert!(matches!(
        changed.activate(changed_terms).await,
        Err(ZecSdkError::AgreementConflict)
    ));
}

#[tokio::test]
async fn persistence_failure_prevents_activation() {
    let store = MemoryStore {
        fail_create: true,
        ..MemoryStore::default()
    };
    let sdk = sdk(
        Participant::Maker,
        agreement_wire(
            "sdk-persist-first",
            SwapDirection::TakerSellsForeign,
            FixtureVariant::Local,
        ),
        store,
    );
    let accepted = sdk
        .negotiate_at(&Offer(1), Proposal, ACCEPTED_AT)
        .await
        .expect("agreement");
    assert!(matches!(
        sdk.activate(accepted).await,
        Err(ZecSdkError::Persistence(_))
    ));
}

#[tokio::test]
async fn activation_rejects_substituted_role_and_revision_before_store() {
    let wire = agreement_wire(
        "sdk-activation-context",
        SwapDirection::TakerSellsForeign,
        FixtureVariant::Local,
    );
    let store = MemoryStore::default();
    let sdk = sdk(Participant::Maker, wire.clone(), store.clone());
    let wrong_role = lez_zec_swap_sdk::AcceptedZecAgreementV1::accept_wire_at(
        &wire,
        ACCEPTED_AT,
        Participant::Taker,
        0,
    )
    .expect("valid agreement with substituted local context");
    assert!(matches!(
        sdk.activate(wrong_role).await,
        Err(ZecSdkError::LocalRoleMismatch {
            expected: Participant::Maker,
            actual: Participant::Taker
        })
    ));

    let wrong_revision = lez_zec_swap_sdk::AcceptedZecAgreementV1::accept_wire_at(
        &wire,
        ACCEPTED_AT,
        Participant::Maker,
        1,
    )
    .expect("valid agreement with substituted initial revision");
    assert!(matches!(
        sdk.activate(wrong_revision).await,
        Err(ZecSdkError::InvalidActivationRevision(1))
    ));
    assert!(store.agreements.lock().expect("store").is_empty());
}

#[tokio::test]
async fn untrusted_negotiation_wire_is_bounded_and_accepts_signed_public_deployment() {
    let oversized = sdk(
        Participant::Maker,
        vec![0; MAX_ZEC_AGREEMENT_RECORD_BYTES + 1],
        MemoryStore::default(),
    );
    assert!(matches!(
        oversized
            .negotiate_at(&Offer(1), Proposal, ACCEPTED_AT)
            .await,
        Err(ZecSdkError::InvalidAgreement(
            ZecAgreementV1Error::OversizedWireRecord { .. }
        ))
    ));

    let public = sdk(
        Participant::Maker,
        agreement_wire(
            "sdk-public",
            SwapDirection::TakerSellsForeign,
            FixtureVariant::Public,
        ),
        MemoryStore::default(),
    );
    let accepted = public
        .negotiate_at(&Offer(1), Proposal, ACCEPTED_AT)
        .await
        .expect("SDK accepts a countersigned exact public deployment");
    assert_eq!(
        accepted.agreement().binding().profile_id(),
        ZecProfileId::PublicTestnetV1
    );
    assert_eq!(
        accepted.agreement().lez_terms().chain().environment(),
        LezEnvironmentV1::PublicTestnetV0_2
    );
}

#[tokio::test]
async fn resume_revalidates_requested_id_role_commitment_and_revision() {
    let requested = SwapId::new("sdk-requested").expect("id");
    let valid_wire = agreement_wire(
        requested.as_str(),
        SwapDirection::TakerSellsForeign,
        FixtureVariant::Local,
    );

    let wrong_id = envelope(
        agreement_wire(
            "sdk-other",
            SwapDirection::TakerSellsForeign,
            FixtureVariant::Local,
        ),
        Participant::Maker,
        0,
    );
    assert!(matches!(
        sdk(
            Participant::Maker,
            valid_wire.clone(),
            MemoryStore::with_record(&requested, wrong_id)
        )
        .resume(&requested)
        .await,
        Err(ZecSdkError::AgreementIdentityMismatch { .. })
    ));

    let wrong_role = envelope(valid_wire.clone(), Participant::Taker, 0);
    assert!(matches!(
        sdk(
            Participant::Maker,
            valid_wire.clone(),
            MemoryStore::with_record(&requested, wrong_role)
        )
        .resume(&requested)
        .await,
        Err(ZecSdkError::LocalRoleMismatch {
            expected: Participant::Maker,
            actual: Participant::Taker
        })
    ));

    let corrupt_commitment = envelope(
        agreement_wire(
            requested.as_str(),
            SwapDirection::TakerSellsForeign,
            FixtureVariant::CorruptCommitment,
        ),
        Participant::Maker,
        0,
    );
    assert!(matches!(
        sdk(
            Participant::Maker,
            valid_wire.clone(),
            MemoryStore::with_record(&requested, corrupt_commitment)
        )
        .resume(&requested)
        .await,
        Err(ZecSdkError::InvalidAgreement(
            ZecAgreementV1Error::CommitmentMismatch
        ))
    ));

    let invalid_revision = envelope(valid_wire.clone(), Participant::Maker, u64::MAX);
    assert!(matches!(
        sdk(
            Participant::Maker,
            valid_wire,
            MemoryStore::with_record(&requested, invalid_revision)
        )
        .resume(&requested)
        .await,
        Err(ZecSdkError::InvalidAgreement(
            ZecAgreementV1Error::InvalidDurableRevision(value)
        )) if value == u64::MAX
    ));
}

#[tokio::test]
async fn independent_actors_reach_both_legs_locked_from_chain_evidence_in_both_directions() {
    Box::pin(assert_independent_actors_reach_both_legs_locked(
        "sdk-actors-forward-both-locked",
        SwapDirection::TakerSellsForeign,
    ))
    .await;
    Box::pin(assert_independent_actors_reach_both_legs_locked(
        "sdk-actors-reverse-both-locked",
        SwapDirection::TakerSellsLez,
    ))
    .await;
}

#[tokio::test]
async fn independent_actors_complete_lez_then_zcash_claims_in_both_directions() {
    for (id, direction, first_claimant, secret) in [
        (
            "sdk-actors-forward-completed",
            SwapDirection::TakerSellsForeign,
            Participant::Taker,
            [0x91; 32],
        ),
        (
            "sdk-actors-reverse-completed",
            SwapDirection::TakerSellsLez,
            Participant::Maker,
            [0x92; 32],
        ),
    ] {
        Box::pin(assert_independent_claim_happy_path(
            id,
            direction,
            first_claimant,
            secret,
        ))
        .await;
    }
}

async fn assert_early_one_leg_deadline(fixture: &mut ClaimActorFixture, step: RefundStepV1) {
    let early_position = match step {
        RefundStepV1::Lez => {
            ChainPosition::lez_timestamp_from_milliseconds_floor(LezUnixMilliseconds::new(
                fixture
                    .taker
                    .agreement()
                    .lez_refund_at_ms()
                    .saturating_sub(1_000),
            ))
        }
        RefundStepV1::Zcash => ChainPosition::block_height(
            Chain::Zcash,
            u64::from(fixture.taker.agreement().zcash_refund_at_height()).saturating_sub(1),
        ),
    };
    fixture.refunds.set_eligibility(
        step,
        RefundEligibilityObservationV1::canonical(early_position),
    );
    assert_eq!(
        fixture
            .taker
            .drive_refund()
            .await
            .expect("early one-leg deadline is a wait"),
        RefundDriveOutcome::AwaitingDeadline(step)
    );
    assert!(fixture.refunds.submissions().is_empty());
    assert!(
        !fixture.refunds.events().iter().any(
            |event| matches!(event, RefundPortEvent::Prepare(candidate) if *candidate == step)
        )
    );
}

async fn project_one_leg_refund(fixture: &mut ClaimActorFixture, step: RefundStepV1) {
    let position = match step {
        RefundStepV1::Lez => ChainPosition::lez_timestamp_from_milliseconds_floor(
            LezUnixMilliseconds::new(fixture.taker.agreement().lez_refund_at_ms()),
        ),
        RefundStepV1::Zcash => ChainPosition::block_height(
            Chain::Zcash,
            u64::from(fixture.taker.agreement().zcash_refund_at_height()),
        ),
    };
    fixture
        .refunds
        .set_eligibility(step, RefundEligibilityObservationV1::canonical(position));
    assert_eq!(
        fixture
            .taker
            .drive_refund()
            .await
            .expect("Taker submits its one-leg refund"),
        RefundDriveOutcome::Submitted(step)
    );
    let submissions = fixture.refunds.submissions();
    assert_eq!(submissions.len(), 1);
    assert_eq!(submissions[0].0, step);
    fixture.refunds.confirm(step);
    assert_eq!(
        fixture
            .taker
            .drive_refund()
            .await
            .expect("Taker projects its one-leg refund"),
        RefundDriveOutcome::Refunded { revision: 2 }
    );
    assert_eq!(
        fixture
            .maker
            .drive_refund()
            .await
            .expect("Maker only observes the one-leg refund"),
        RefundDriveOutcome::Refunded { revision: 2 }
    );
    assert_eq!(fixture.maker.status(), Phase::Refunded);
    assert_eq!(fixture.taker.status(), Phase::Refunded);
    let submissions = fixture.refunds.submissions();
    assert_eq!(submissions.len(), 1);
    assert_eq!(submissions[0].0, step);
}

async fn assert_one_leg_terminal_replay(id: &str, fixture: &mut ClaimActorFixture) {
    let events_before_replay = fixture.refunds.events();
    let submissions_before_replay = fixture.refunds.submissions();
    restart_refund_actor(id, fixture, Participant::Maker).await;
    restart_refund_actor(id, fixture, Participant::Taker).await;
    assert_eq!(
        fixture
            .maker
            .drive_refund()
            .await
            .expect("maker terminal replay"),
        RefundDriveOutcome::Refunded { revision: 2 }
    );
    assert_eq!(
        fixture
            .taker
            .drive_refund()
            .await
            .expect("taker terminal replay"),
        RefundDriveOutcome::Refunded { revision: 2 }
    );
    assert_eq!(
        (fixture.maker.status(), fixture.maker.revision()),
        (Phase::Refunded, 2)
    );
    assert_eq!(
        (fixture.taker.status(), fixture.taker.revision()),
        (Phase::Refunded, 2)
    );
    assert_eq!(fixture.refunds.events(), events_before_replay);
    assert_eq!(fixture.refunds.submissions(), submissions_before_replay);
}

async fn assert_taker_one_leg_recovery(
    id: &str,
    direction: SwapDirection,
    first_claimant: Participant,
    secret: [u8; 32],
) {
    let mut fixture = activate_claim_actor_fixture(id, direction, first_claimant, secret).await;
    project_actor_taker_first_lock(direction, &mut fixture.taker, &fixture.lez, &fixture.zcash)
        .await;
    expose_taker_lock_to_maker(direction, &fixture.maker, &fixture.lez, &fixture.zcash);
    fixture
        .maker
        .observe_taker_first_lock()
        .await
        .expect("maker observes the only funded leg");
    assert_eq!(fixture.maker.status(), Phase::TakerLockConfirmed);
    assert_eq!(fixture.taker.status(), Phase::TakerLockConfirmed);

    let step = match direction {
        SwapDirection::TakerSellsForeign => RefundStepV1::Zcash,
        SwapDirection::TakerSellsLez => RefundStepV1::Lez,
    };
    assert_eq!(step.owner(fixture.taker.agreement()), Participant::Taker);
    assert_early_one_leg_deadline(&mut fixture, step).await;
    project_one_leg_refund(&mut fixture, step).await;
    assert_one_leg_terminal_replay(id, &mut fixture).await;
}

#[tokio::test]
async fn taker_recovers_its_only_funded_leg_in_both_directions() {
    for (id, direction, first_claimant, secret) in [
        (
            "sdk-one-leg-refund-zcash",
            SwapDirection::TakerSellsForeign,
            Participant::Taker,
            [0xa1; 32],
        ),
        (
            "sdk-one-leg-refund-lez",
            SwapDirection::TakerSellsLez,
            Participant::Maker,
            [0xa2; 32],
        ),
    ] {
        Box::pin(assert_taker_one_leg_recovery(
            id,
            direction,
            first_claimant,
            secret,
        ))
        .await;
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn refund_contract_keeps_lez_before_zcash_in_both_directions() {
    for (id, direction, first_claimant, secret) in [
        (
            "sdk-refund-contract-forward",
            SwapDirection::TakerSellsForeign,
            Participant::Taker,
            [0xb1; 32],
        ),
        (
            "sdk-refund-contract-reverse",
            SwapDirection::TakerSellsLez,
            Participant::Maker,
            [0xb2; 32],
        ),
    ] {
        let mut fixture = activate_claim_actor_fixture(id, direction, first_claimant, secret).await;
        lock_claim_actor_fixture(direction, &mut fixture).await;
        let swap_id = SwapId::new(id).expect("swap ID");
        let lez_owner = fixture.maker.agreement().lez_depositor();
        let zcash_owner = fixture.maker.agreement().lez_claimant();
        let lez_refund_at_ms = fixture.maker.agreement().lez_refund_at_ms();
        let zcash_refund_at_height = fixture.maker.agreement().zcash_refund_at_height();
        let lez_position = ChainPosition::lez_timestamp_from_milliseconds_floor(
            LezUnixMilliseconds::new(lez_refund_at_ms),
        );
        let zcash_position =
            ChainPosition::block_height(Chain::Zcash, u64::from(zcash_refund_at_height));

        assert_eq!(RefundStepV1::Lez.next(), Some(RefundStepV1::Zcash));

        fixture
            .refunds
            .set_eligibility(RefundStepV1::Lez, RefundEligibilityObservationV1::Unstable);
        assert_eq!(
            drive_refund_actor(&mut fixture, lez_owner)
                .await
                .expect("unstable LEZ view is a wait"),
            RefundDriveOutcome::AwaitingStableObservation(RefundStepV1::Lez)
        );
        fixture.refunds.set_eligibility(
            RefundStepV1::Lez,
            RefundEligibilityObservationV1::FundingUnavailable(RefundFundingWaitReasonV1::Reorged),
        );
        assert_eq!(
            drive_refund_actor(&mut fixture, lez_owner)
                .await
                .expect("reorged LEZ funding is a wait"),
            RefundDriveOutcome::AwaitingFunding {
                step: RefundStepV1::Lez,
                reason: RefundFundingWaitReasonV1::Reorged,
            }
        );
        fixture.refunds.set_eligibility(
            RefundStepV1::Lez,
            RefundEligibilityObservationV1::canonical(
                ChainPosition::lez_timestamp_from_milliseconds_floor(LezUnixMilliseconds::new(
                    lez_refund_at_ms.saturating_sub(1_000),
                )),
            ),
        );
        assert_eq!(
            drive_refund_actor(&mut fixture, lez_owner)
                .await
                .expect("early LEZ deadline is a wait"),
            RefundDriveOutcome::AwaitingDeadline(RefundStepV1::Lez)
        );
        assert!(fixture.refunds.submissions().is_empty());
        assert!(
            !fixture
                .refunds
                .events()
                .iter()
                .any(|event| matches!(event, RefundPortEvent::Prepare(_)))
        );

        fixture.refunds.set_eligibility(
            RefundStepV1::Lez,
            RefundEligibilityObservationV1::canonical(lez_position),
        );
        fixture
            .refunds
            .set_submit_outcome(RefundStepV1::Lez, RefundSubmitOutcomeV1::Unknown);
        assert_eq!(
            drive_refund_actor(&mut fixture, lez_owner)
                .await
                .expect("unknown LEZ outcome is explicit"),
            RefundDriveOutcome::SubmissionOutcomeUnknown(RefundStepV1::Lez)
        );
        restart_refund_actor(id, &mut fixture, lez_owner).await;
        fixture.refunds.set_submit_outcome(
            RefundStepV1::Lez,
            RefundSubmitOutcomeV1::DefinitivelyRejected,
        );
        assert_eq!(
            drive_refund_actor(&mut fixture, lez_owner)
                .await
                .expect("definitive LEZ rejection is explicit"),
            RefundDriveOutcome::SubmissionRejected(RefundStepV1::Lez)
        );
        let lez_submissions = fixture.refunds.submissions();
        assert_eq!(lez_submissions.len(), 2);
        assert_eq!(lez_submissions[0], lez_submissions[1]);
        let events = fixture.refunds.events();
        let second_submit = events
            .iter()
            .rposition(|event| *event == RefundPortEvent::Submit(RefundStepV1::Lez))
            .expect("second LEZ submit event");
        assert!(matches!(
            events.get(second_submit.saturating_sub(2)),
            Some(RefundPortEvent::ObservePrepared(RefundStepV1::Lez))
        ));
        assert!(matches!(
            events.get(second_submit.saturating_sub(1)),
            Some(RefundPortEvent::Eligibility(RefundStepV1::Lez))
        ));

        assert_eq!(
            drive_refund_actor(&mut fixture, lez_owner.other())
                .await
                .expect("non-owner only observes LEZ"),
            RefundDriveOutcome::AwaitingStableObservation(RefundStepV1::Lez)
        );
        assert!(
            !fixture
                .refunds
                .events()
                .iter()
                .any(|event| { matches!(event, RefundPortEvent::Prepare(RefundStepV1::Zcash)) })
        );
        fixture
            .refunds
            .set_submit_outcome(RefundStepV1::Lez, RefundSubmitOutcomeV1::Accepted);
        fixture.refunds.confirm(RefundStepV1::Lez);
        refund_store(&fixture, lez_owner)
            .set_transition_mode(TransitionCommitMode::FailBeforeCommit);
        assert!(matches!(
            drive_refund_actor(&mut fixture, lez_owner).await,
            Err(ZecSdkError::Persistence(_))
        ));
        assert_eq!(
            refund_actor(&fixture, lez_owner).status(),
            Phase::BothLegsLocked,
            "failed atomic projection leaves in-memory state unchanged"
        );
        refund_store(&fixture, lez_owner)
            .set_transition_mode(TransitionCommitMode::CommitThenReportFailure);
        assert!(matches!(
            drive_refund_actor(&mut fixture, lez_owner).await,
            Ok(RefundDriveOutcome::Projected {
                step: RefundStepV1::Lez,
                revision: 3
            })
        ));
        refund_store(&fixture, lez_owner).set_transition_mode(TransitionCommitMode::Normal);
        assert!(matches!(
            drive_refund_actor(&mut fixture, lez_owner.other()).await,
            Ok(RefundDriveOutcome::Projected {
                step: RefundStepV1::Lez,
                revision: 3
            })
        ));
        assert!(matches!(
            fixture.maker.drive_claim().await,
            Err(ZecSdkError::ClaimNotReady(
                Phase::MakerLegRefunded | Phase::TakerLegRefunded
            ))
        ));

        fixture.refunds.set_eligibility(
            RefundStepV1::Zcash,
            RefundEligibilityObservationV1::canonical(zcash_position),
        );
        assert_eq!(
            drive_refund_actor(&mut fixture, zcash_owner)
                .await
                .expect("later Zcash refund submits"),
            RefundDriveOutcome::Submitted(RefundStepV1::Zcash)
        );
        fixture.refunds.confirm(RefundStepV1::Zcash);
        assert_eq!(
            drive_refund_actor(&mut fixture, zcash_owner)
                .await
                .expect("Zcash owner projects refund"),
            RefundDriveOutcome::Refunded { revision: 4 }
        );
        assert_eq!(
            drive_refund_actor(&mut fixture, zcash_owner.other())
                .await
                .expect("Zcash observer projects refund"),
            RefundDriveOutcome::Refunded { revision: 4 }
        );
        assert_eq!(fixture.maker.status(), Phase::Refunded);
        assert_eq!(fixture.taker.status(), Phase::Refunded);
        assert_eq!(
            fixture
                .refunds
                .events()
                .iter()
                .filter(|event| matches!(event, RefundPortEvent::Prepare(RefundStepV1::Lez)))
                .count(),
            1,
            "restart and observer paths never re-sign the LEZ refund"
        );
        assert_eq!(
            fixture
                .refunds
                .events()
                .iter()
                .filter(|event| matches!(event, RefundPortEvent::Prepare(RefundStepV1::Zcash)))
                .count(),
            1,
            "only the Zcash funding owner signs the later refund"
        );
        assert!(matches!(
            fixture.maker.drive_claim().await,
            Err(ZecSdkError::ClaimNotReady(Phase::Refunded))
        ));

        fixture.maker = actor_sdk(
            Participant::Maker,
            fixture.wire.clone(),
            fixture.lez.clone(),
            fixture.zcash.clone(),
            fixture.maker_store.clone(),
        )
        .resume_all_capable(&swap_id)
        .await
        .expect("restart maker")
        .expect("durable maker");
        fixture.taker = actor_sdk(
            Participant::Taker,
            fixture.wire.clone(),
            fixture.lez.clone(),
            fixture.zcash.clone(),
            fixture.taker_store.clone(),
        )
        .resume_all_capable(&swap_id)
        .await
        .expect("restart taker")
        .expect("durable taker");
        assert_eq!(fixture.maker.status(), Phase::Refunded);
        assert_eq!(fixture.maker.revision(), 4);
        assert_eq!(fixture.taker.status(), Phase::Refunded);
        assert_eq!(fixture.taker.revision(), 4);
        assert_eq!(
            fixture.maker.drive_refund().await.expect("maker replay"),
            RefundDriveOutcome::Refunded { revision: 4 }
        );
        assert_eq!(
            fixture.taker.drive_refund().await.expect("taker replay"),
            RefundDriveOutcome::Refunded { revision: 4 }
        );

        for (role, store) in [
            (Participant::Maker, fixture.maker_store.clone()),
            (Participant::Taker, fixture.taker_store.clone()),
        ] {
            let status_sdk: ZecPairSdk<(), (), (), (), MemoryStore> =
                ZecPairSdk::new(role, (), (), (), (), store);
            let replay = status_sdk
                .resume_all_capable(&swap_id)
                .await
                .expect("offline terminal refund restart")
                .expect("durable refunded swap");
            assert_eq!(replay.status(), Phase::Refunded);
            assert_eq!(replay.revision(), 4);
        }
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn refund_records_round_trip_only_after_full_context_revalidation() {
    for (id, direction, first_claimant, secret) in [
        (
            "sdk-refund-record-forward",
            SwapDirection::TakerSellsForeign,
            Participant::Taker,
            [0xb3; 32],
        ),
        (
            "sdk-refund-record-reverse",
            SwapDirection::TakerSellsLez,
            Participant::Maker,
            [0xb4; 32],
        ),
    ] {
        let mut fixture = activate_claim_actor_fixture(id, direction, first_claimant, secret).await;
        lock_claim_actor_fixture(direction, &mut fixture).await;
        let swap_id = SwapId::new(id).expect("swap ID");
        let owner = fixture.maker.agreement().lez_depositor();
        let owner_store = refund_store(&fixture, owner);
        let accepted = owner_store
            .accepted(&swap_id)
            .expect("accepted owner terms");
        let coordinator = owner_store
            .coordinator_at(&accepted, 2)
            .expect("locked owner coordinator");
        let prepared = PreparedRefundSubmissionV1::new(
            RefundStepV1::Lez,
            refund_submission_id(RefundStepV1::Lez),
            refund_submission_bytes(RefundStepV1::Lez).to_vec(),
        )
        .expect("prepared LEZ refund");
        let intent =
            RefundIntentV1::from_active(accepted.agreement(), &coordinator, owner, 2, prepared)
                .expect("owner intent");
        let intent_record = RefundIntentRecordV1::from(&intent);
        let encoded_intent = serde_json::to_vec(&intent_record).expect("serialize intent record");
        let decoded_intent: RefundIntentRecordV1 =
            serde_json::from_slice(&encoded_intent).expect("decode intent record");
        assert_eq!(
            decoded_intent
                .revalidate(&accepted, &coordinator, 2)
                .expect("revalidate intent"),
            intent
        );

        let position = ChainPosition::lez_timestamp_from_milliseconds_floor(
            LezUnixMilliseconds::new(accepted.agreement().lez_refund_at_ms()),
        );
        let evidence = RefundEvidenceV1::new(
            accepted.agreement(),
            RefundStepV1::Lez,
            refund_submission_id(RefundStepV1::Lez),
            "durable-record-lez-refund",
            position,
            3,
        )
        .expect("canonical refund evidence");
        let transition = RefundTransitionV1::from_owned(
            accepted.agreement(),
            &coordinator,
            &intent,
            2,
            evidence,
        )
        .expect("owner transition");
        let transition_record = RefundTransitionRecordV1::from(&transition);
        let encoded_transition =
            serde_json::to_vec(&transition_record).expect("serialize transition record");
        let decoded_transition: RefundTransitionRecordV1 =
            serde_json::from_slice(&encoded_transition).expect("decode transition record");
        assert_eq!(
            decoded_transition
                .revalidate(&accepted, &coordinator, 2, Some(&decoded_intent))
                .expect("revalidate owner transition"),
            transition
        );
        assert!(matches!(
            decoded_transition.revalidate(&accepted, &coordinator, 2, None),
            Err(RefundRecordError::MissingIntent)
        ));

        let observer_store = refund_store(&fixture, owner.other());
        let observer_accepted = observer_store
            .accepted(&swap_id)
            .expect("accepted observer terms");
        let observer_coordinator = observer_store
            .coordinator_at(&observer_accepted, 2)
            .expect("locked observer coordinator");
        let observed_evidence = RefundEvidenceV1::new(
            observer_accepted.agreement(),
            RefundStepV1::Lez,
            refund_submission_id(RefundStepV1::Lez),
            "durable-record-lez-refund",
            position,
            3,
        )
        .expect("observed canonical refund evidence");
        let observed = RefundTransitionV1::from_observed(
            observer_accepted.agreement(),
            &observer_coordinator,
            owner.other(),
            2,
            observed_evidence,
        )
        .expect("observer transition");
        let observed_record = RefundTransitionRecordV1::from(&observed);
        assert_eq!(
            observed_record
                .revalidate(&observer_accepted, &observer_coordinator, 2, None)
                .expect("revalidate observer transition"),
            observed
        );
        assert!(matches!(
            observed_record.revalidate(
                &observer_accepted,
                &observer_coordinator,
                2,
                Some(&decoded_intent),
            ),
            Err(RefundRecordError::UnexpectedIntent)
        ));

        let mut unknown_field: serde_json::Value =
            serde_json::from_slice(&encoded_transition).expect("transition JSON");
        unknown_field
            .as_object_mut()
            .expect("transition object")
            .insert("future_field".to_owned(), serde_json::json!(true));
        assert!(serde_json::from_value::<RefundTransitionRecordV1>(unknown_field).is_err());
    }
}

async fn drive_refund_actor(
    fixture: &mut ClaimActorFixture,
    participant: Participant,
) -> Result<RefundDriveOutcome, ZecSdkError> {
    match participant {
        Participant::Maker => fixture.maker.drive_refund().await,
        Participant::Taker => fixture.taker.drive_refund().await,
    }
}

fn refund_actor(fixture: &ClaimActorFixture, participant: Participant) -> &ActorActive {
    match participant {
        Participant::Maker => &fixture.maker,
        Participant::Taker => &fixture.taker,
    }
}

fn refund_store(fixture: &ClaimActorFixture, participant: Participant) -> &MemoryStore {
    match participant {
        Participant::Maker => &fixture.maker_store,
        Participant::Taker => &fixture.taker_store,
    }
}

async fn restart_refund_actor(id: &str, fixture: &mut ClaimActorFixture, participant: Participant) {
    let swap_id = SwapId::new(id).expect("swap ID");
    let resumed = actor_sdk(
        participant,
        fixture.wire.clone(),
        fixture.lez.clone(),
        fixture.zcash.clone(),
        refund_store(fixture, participant).clone(),
    )
    .resume(&swap_id)
    .await
    .expect("restart refund owner")
    .expect("durable refund owner");
    match participant {
        Participant::Maker => fixture.maker = resumed,
        Participant::Taker => fixture.taker = resumed,
    }
}

#[tokio::test]
async fn reveal_waits_for_exact_fresh_unspent_zcash_funding_after_restart_in_both_directions() {
    for (id, direction, first_claimant, secret) in [
        (
            "sdk-pre-reveal-gate-forward",
            SwapDirection::TakerSellsForeign,
            Participant::Taker,
            [0xa1; 32],
        ),
        (
            "sdk-pre-reveal-gate-reverse",
            SwapDirection::TakerSellsLez,
            Participant::Maker,
            [0xa2; 32],
        ),
    ] {
        Box::pin(assert_pre_reveal_funding_gate(
            id,
            direction,
            first_claimant,
            secret,
        ))
        .await;
    }
}

async fn assert_pre_reveal_funding_gate(
    id: &str,
    direction: SwapDirection,
    first_claimant: Participant,
    secret: [u8; 32],
) {
    let mut fixture = activate_claim_actor_fixture(id, direction, first_claimant, secret).await;
    lock_claim_actor_fixture(direction, &mut fixture).await;
    let expected_context = match first_claimant {
        Participant::Maker => fixture.maker.zcash_funding_context(),
        Participant::Taker => fixture.taker.zcash_funding_context(),
    }
    .expect("both locked derives exact funding context");

    assert_funding_wait(
        &mut fixture,
        ZcashFundingObservationV1::Absent,
        ZcashFundingWaitReasonV1::Absent,
    )
    .await;
    assert_eq!(
        fixture
            .corridor
            .port_events()
            .iter()
            .filter(|event| **event == ClaimPortEvent::PrepareReveal)
            .count(),
        1,
        "unsafe funding still leaves one durable reveal intent"
    );

    restart_revealing_actor(id, &mut fixture).await;
    assert_funding_wait(
        &mut fixture,
        ZcashFundingObservationV1::Spent,
        ZcashFundingWaitReasonV1::Spent,
    )
    .await;
    assert_funding_wait(
        &mut fixture,
        ZcashFundingObservationV1::Unstable,
        ZcashFundingWaitReasonV1::Unstable,
    )
    .await;

    let agreement = fixture.maker.agreement().clone();
    assert_unsafe_funding_mutations(&mut fixture, &agreement).await;
    assert_exact_funding_submission(&mut fixture, &agreement, &expected_context, secret).await;
}

async fn assert_funding_wait(
    fixture: &mut ClaimActorFixture,
    observation: ZcashFundingObservationV1,
    reason: ZcashFundingWaitReasonV1,
) {
    fixture.corridor.expose_funding_as(observation);
    assert_eq!(
        drive_revealing_actor(fixture)
            .await
            .expect("unsafe funding waits"),
        ClaimDriveOutcome::AwaitingSafeZcashFunding(reason)
    );
    assert!(fixture.corridor.submissions().is_empty());
}

async fn assert_unsafe_funding_mutations(
    fixture: &mut ClaimActorFixture,
    agreement: &lez_zec_swap_sdk::ZecAgreementV1,
) {
    for mutation in [
        FundingMutation::CanonicalTransactionId,
        FundingMutation::CanonicalValue,
        FundingMutation::CanonicalScript,
        FundingMutation::Network,
        FundingMutation::ConsensusBranch,
        FundingMutation::InsufficientConfirmations,
        FundingMutation::UnstableUnspentTip,
        FundingMutation::UnspentTip,
        FundingMutation::UnspentTransactionId,
        FundingMutation::UnspentOutputIndex,
        FundingMutation::UnspentValue,
        FundingMutation::UnspentScript,
        FundingMutation::RefundHeightReached,
    ] {
        fixture
            .corridor
            .expose_funding_as(mutated_zcash_funding_observation(agreement, mutation));
        assert_invalid_funding_observation(fixture, mutation).await;
    }
}

async fn assert_invalid_funding_observation(
    fixture: &mut ClaimActorFixture,
    mutation: FundingMutation,
) {
    assert!(
        matches!(
            drive_revealing_actor(fixture).await,
            Err(ZecSdkError::InvalidZcashFundingObservation(_))
        ),
        "{mutation:?} must fail closed"
    );
    assert!(
        fixture.corridor.submissions().is_empty(),
        "{mutation:?} must not reveal the preimage"
    );
}

async fn assert_exact_funding_submission(
    fixture: &mut ClaimActorFixture,
    agreement: &lez_zec_swap_sdk::ZecAgreementV1,
    expected_context: &ZcashFundingContextV1,
    secret: [u8; 32],
) {
    fixture
        .corridor
        .expose_funding_as(exact_zcash_funding_observation(agreement));
    assert_eq!(
        drive_revealing_actor(fixture)
            .await
            .expect("exact canonical unspent funding permits reveal"),
        ClaimDriveOutcome::Submitted(ClaimStepV1::RevealingLez)
    );
    let mut expected_submission = vec![0x6c, 0x65, 0x7a];
    expected_submission.extend_from_slice(&secret);
    assert_eq!(
        fixture.corridor.revealing_submission_bytes(),
        vec![expected_submission],
        "restart submits the same single retained reveal, never a re-prepared substitute"
    );
    let events = fixture.corridor.port_events();
    assert_eq!(
        &events[events.len() - 3..],
        &[
            ClaimPortEvent::ObserveReveal,
            ClaimPortEvent::ObserveFunding,
            ClaimPortEvent::SubmitReveal,
        ],
        "fresh funding observation is literally the final awaited claim-port call before submit"
    );
    assert!(
        fixture
            .corridor
            .funding_contexts()
            .iter()
            .all(|context| context == expected_context),
        "every retry, including after restart, re-derives the same durable exact outpoint"
    );
}

async fn drive_revealing_actor(
    fixture: &mut ClaimActorFixture,
) -> Result<ClaimDriveOutcome, ZecSdkError> {
    match fixture.first_claimant {
        Participant::Maker => fixture.maker.drive_claim().await,
        Participant::Taker => fixture.taker.drive_claim().await,
    }
}

async fn restart_revealing_actor(id: &str, fixture: &mut ClaimActorFixture) {
    let swap_id = SwapId::new(id).expect("swap ID");
    match fixture.first_claimant {
        Participant::Maker => {
            fixture.maker = actor_sdk(
                Participant::Maker,
                fixture.wire.clone(),
                fixture.lez.clone(),
                fixture.zcash.clone(),
                fixture.maker_store.clone(),
            )
            .resume_claim_capable(&swap_id)
            .await
            .expect("restart revealing maker")
            .expect("durable revealing maker");
        }
        Participant::Taker => {
            fixture.taker = actor_sdk(
                Participant::Taker,
                fixture.wire.clone(),
                fixture.lez.clone(),
                fixture.zcash.clone(),
                fixture.taker_store.clone(),
            )
            .resume_claim_capable(&swap_id)
            .await
            .expect("restart revealing taker")
            .expect("durable revealing taker");
        }
    }
}

type ActorActive =
    ActiveZecSwap<MemoryLezTakerLockObservation, MemoryZcashTakerLockObservation, MemoryStore>;

struct ClaimActorFixture {
    wire: Vec<u8>,
    corridor: MemoryClaimCorridor,
    refunds: MemoryRefundCorridor,
    lez: MemoryLezTakerLockObservation,
    zcash: MemoryZcashTakerLockObservation,
    maker_store: MemoryStore,
    taker_store: MemoryStore,
    maker: ActorActive,
    taker: ActorActive,
    first_claimant: Participant,
    secret: [u8; 32],
}

async fn assert_independent_claim_happy_path(
    id: &str,
    direction: SwapDirection,
    first_claimant: Participant,
    secret: [u8; 32],
) {
    let mut fixture = activate_claim_actor_fixture(id, direction, first_claimant, secret).await;
    assert!(matches!(
        fixture.maker.zcash_claim_context(),
        Err(ZecSdkError::InvalidZcashClaimContext(
            ZcashClaimContextError::ClaimNotReady {
                actual: Phase::Offered
            }
        ))
    ));
    lock_claim_actor_fixture(direction, &mut fixture).await;
    assert!(matches!(
        fixture.taker.zcash_claim_context(),
        Err(ZecSdkError::InvalidZcashClaimContext(
            ZcashClaimContextError::ClaimNotReady {
                actual: Phase::BothLegsLocked
            }
        ))
    ));
    drive_claim_actor_fixture(id, &mut fixture).await;
    Box::pin(assert_claim_actor_restarts(id, fixture)).await;
}

async fn activate_claim_actor_fixture(
    id: &str,
    direction: SwapDirection,
    first_claimant: Participant,
    secret: [u8; 32],
) -> ClaimActorFixture {
    let wire = agreement_wire_with_digest(
        id,
        direction,
        FixtureVariant::Local,
        Sha256::digest(secret).into(),
    );
    let corridor = MemoryClaimCorridor::default();
    let refunds = MemoryRefundCorridor::default();
    let lez = MemoryLezTakerLockObservation(
        MemoryTakerLockObservation::default(),
        Arc::default(),
        MemoryFirstLockPort::default(),
        MemoryMakerLockObservation::default(),
        corridor.clone(),
        refunds.clone(),
    );
    let zcash = MemoryZcashTakerLockObservation(
        MemoryTakerLockObservation::default(),
        MemoryFirstLockPort::default(),
        MemoryMakerLockObservation::default(),
        corridor.clone(),
        Arc::default(),
        refunds.clone(),
    );
    let maker_store = MemoryStore::default();
    let taker_store = MemoryStore::default();
    let maker_sdk = actor_sdk(
        Participant::Maker,
        wire.clone(),
        lez.clone(),
        zcash.clone(),
        maker_store.clone(),
    );
    let taker_sdk = actor_sdk(
        Participant::Taker,
        wire.clone(),
        lez.clone(),
        zcash.clone(),
        taker_store.clone(),
    );
    let maker_terms = maker_sdk
        .negotiate_at(&Offer(1), Proposal, ACCEPTED_AT)
        .await
        .expect("maker agreement");
    let taker_terms = taker_sdk
        .negotiate_at(&Offer(1), Proposal, ACCEPTED_AT)
        .await
        .expect("taker agreement");
    let maker = if first_claimant == Participant::Maker {
        maker_sdk
            .activate_with_claim_preimage(maker_terms, ClaimPreimage::new(secret))
            .await
            .expect("first claimant protects its preimage at activation")
    } else {
        maker_sdk
            .activate(maker_terms)
            .await
            .expect("maker activation")
    };
    let taker = if first_claimant == Participant::Taker {
        taker_sdk
            .activate_with_claim_preimage(taker_terms, ClaimPreimage::new(secret))
            .await
            .expect("first claimant protects its preimage at activation")
    } else {
        taker_sdk
            .activate(taker_terms)
            .await
            .expect("taker activation")
    };
    ClaimActorFixture {
        wire,
        corridor,
        refunds,
        lez,
        zcash,
        maker_store,
        taker_store,
        maker,
        taker,
        first_claimant,
        secret,
    }
}

async fn lock_claim_actor_fixture(direction: SwapDirection, fixture: &mut ClaimActorFixture) {
    project_actor_taker_first_lock(direction, &mut fixture.taker, &fixture.lez, &fixture.zcash)
        .await;
    expose_taker_lock_to_maker(direction, &fixture.maker, &fixture.lez, &fixture.zcash);
    fixture
        .maker
        .observe_taker_first_lock()
        .await
        .expect("maker observes the taker lock");
    let evidence = project_actor_maker_second_lock(
        direction,
        &mut fixture.maker,
        &fixture.lez,
        &fixture.zcash,
    )
    .await;
    expose_maker_lock_to_taker(direction, evidence, &fixture.lez, &fixture.zcash);
    fixture
        .taker
        .observe_maker_lock()
        .await
        .expect("taker observes the maker lock");
    assert_eq!(fixture.maker.status(), Phase::BothLegsLocked);
    assert_eq!(fixture.taker.status(), Phase::BothLegsLocked);
}

async fn drive_claim_actor_fixture(id: &str, fixture: &mut ClaimActorFixture) {
    match fixture.first_claimant {
        Participant::Maker => fixture
            .maker
            .drive_claim()
            .await
            .expect("maker durably prepares and submits the LEZ reveal"),
        Participant::Taker => fixture
            .taker
            .drive_claim()
            .await
            .expect("taker durably prepares and submits the LEZ reveal"),
    };
    assert_eq!(
        fixture.corridor.submissions(),
        vec![ClaimStepV1::RevealingLez]
    );
    fixture.corridor.confirm_revealing_claim(fixture.secret);
    fixture
        .maker
        .drive_claim()
        .await
        .expect("maker canonically observes the LEZ reveal");
    fixture
        .taker
        .drive_claim()
        .await
        .expect("taker canonically observes the LEZ reveal");
    assert_eq!(fixture.maker.status(), Phase::ClaimEvidenceAvailable);
    assert_eq!(fixture.taker.status(), Phase::ClaimEvidenceAvailable);

    let follower_store = match fixture.first_claimant {
        Participant::Maker => &fixture.taker_store,
        Participant::Taker => &fixture.maker_store,
    };
    let recovered = follower_store
        .load_claim_material(&SwapId::new(id).expect("swap ID"))
        .await
        .expect("follower authenticates extracted material")
        .expect("canonical LEZ evidence persisted the preimage internally");
    assert_eq!(recovered.expose_secret(), &fixture.secret);
    match fixture.first_claimant {
        Participant::Maker => fixture
            .taker
            .drive_claim()
            .await
            .expect("taker consumes only protected material for Zcash"),
        Participant::Taker => fixture
            .maker
            .drive_claim()
            .await
            .expect("maker consumes only protected material for Zcash"),
    };
    assert_eq!(
        fixture.corridor.submissions(),
        vec![ClaimStepV1::RevealingLez, ClaimStepV1::FollowupZcash],
        "the corridor must reveal on LEZ before spending Zcash"
    );
    fixture.corridor.confirm_followup_claim();
    fixture
        .maker
        .drive_claim()
        .await
        .expect("maker observes canonical Zcash completion");
    fixture
        .taker
        .drive_claim()
        .await
        .expect("taker observes canonical Zcash completion");
    let funding_transaction_id = canonical_zcash_taker_lock(fixture.maker.agreement())
        .transaction_id()
        .to_string();
    let internal_transaction_id =
        ReverseHex::decode(&funding_transaction_id).expect("canonical fixture transaction ID");
    let contexts = fixture.corridor.zcash_contexts();
    assert_eq!(
        contexts.len(),
        5,
        "prepare, exact observation, submit, owner retry, and counterparty observation all bind context"
    );
    assert!(contexts.iter().all(|context| {
        context.agreement_commitment() == fixture.maker.agreement().agreement_commitment()
            && context.swap_id().as_str() == id
            && context.zcash_funder() == fixture.first_claimant
            && context.funding_transaction_id() == funding_transaction_id
            && context.funding_transaction_id_bytes() == &internal_transaction_id
            && context.funding_output_index() == 0
            && context.funding_outpoint() == &OutPoint::new(internal_transaction_id, 0)
    }));
    assert_eq!(
        (fixture.maker.status(), fixture.maker.revision()),
        (Phase::Completed, 4)
    );
    assert_eq!(
        (fixture.taker.status(), fixture.taker.revision()),
        (Phase::Completed, 4)
    );
}

async fn assert_claim_actor_restarts(id: &str, fixture: ClaimActorFixture) {
    let swap_id = SwapId::new(id).expect("swap ID");
    let maker_sdk: ZecPairSdk<(), (), (), (), MemoryStore> =
        ZecPairSdk::new(Participant::Maker, (), (), (), (), fixture.maker_store);
    let maker_replay = maker_sdk
        .resume_all_capable(&swap_id)
        .await
        .expect("offline maker restart")
        .expect("durable maker swap");
    let taker_sdk: ZecPairSdk<(), (), (), (), MemoryStore> =
        ZecPairSdk::new(Participant::Taker, (), (), (), (), fixture.taker_store);
    let taker_recovery = taker_sdk
        .resume_all_capable(&swap_id)
        .await
        .expect("offline taker restart")
        .expect("durable taker swap");
    assert_eq!(maker_replay.status(), Phase::Completed);
    assert_eq!(maker_replay.revision(), 4);
    assert_eq!(taker_recovery.status(), Phase::Completed);
    assert_eq!(taker_recovery.revision(), 4);
}

async fn assert_independent_actors_reach_both_legs_locked(id: &str, direction: SwapDirection) {
    let wire = agreement_wire(id, direction, FixtureVariant::Local);
    let lez = MemoryLezTakerLockObservation::default();
    let zcash = MemoryZcashTakerLockObservation::default();
    let maker_store = MemoryStore::default();
    let taker_store = MemoryStore::default();
    let maker_sdk = actor_sdk(
        Participant::Maker,
        wire.clone(),
        lez.clone(),
        zcash.clone(),
        maker_store.clone(),
    );
    let taker_sdk = actor_sdk(
        Participant::Taker,
        wire.clone(),
        lez.clone(),
        zcash.clone(),
        taker_store.clone(),
    );
    let maker_terms = maker_sdk
        .negotiate_at(&Offer(1), Proposal, ACCEPTED_AT)
        .await
        .expect("maker agreement");
    let taker_terms = taker_sdk
        .negotiate_at(&Offer(1), Proposal, ACCEPTED_AT)
        .await
        .expect("taker agreement");
    let mut maker = maker_sdk
        .activate(maker_terms)
        .await
        .expect("maker activation");
    let mut taker = taker_sdk
        .activate(taker_terms)
        .await
        .expect("taker activation");

    project_actor_taker_first_lock(direction, &mut taker, &lez, &zcash).await;
    expose_taker_lock_to_maker(direction, &maker, &lez, &zcash);
    maker
        .observe_taker_first_lock()
        .await
        .expect("maker independently observes taker lock");
    let maker_evidence = project_actor_maker_second_lock(direction, &mut maker, &lez, &zcash).await;
    expose_maker_lock_to_taker(direction, maker_evidence, &lez, &zcash);
    taker_store.set_transition_mode(TransitionCommitMode::CommitThenReportFailure);
    let ObserveMakerLockOutcome::Projected(commit) = taker
        .observe_maker_lock()
        .await
        .expect("exact-slot probe accepts the unknown successful commit")
    else {
        panic!("confirmed maker evidence must project");
    };
    assert!(commit.was_replay());
    taker_store.set_transition_mode(TransitionCommitMode::Normal);

    assert_eq!(
        (maker.status(), maker.revision()),
        (Phase::BothLegsLocked, 2)
    );
    assert_eq!(
        (taker.status(), taker.revision()),
        (Phase::BothLegsLocked, 2)
    );
    assert_actor_stores_are_isolated(&maker_store, &taker_store);
    assert_actor_restarts_at_both_locked(id, wire, lez, zcash, maker_store, taker_store).await;
}

fn assert_actor_stores_are_isolated(maker_store: &MemoryStore, taker_store: &MemoryStore) {
    assert_eq!(
        maker_store
            .observed_maker_lock_transitions
            .lock()
            .expect("maker store")
            .len(),
        0,
        "maker store must not receive the taker-local observation"
    );
    assert_eq!(
        taker_store
            .observed_maker_lock_transitions
            .lock()
            .expect("taker store")
            .len(),
        1
    );
}

async fn assert_actor_restarts_at_both_locked(
    id: &str,
    wire: Vec<u8>,
    lez: MemoryLezTakerLockObservation,
    zcash: MemoryZcashTakerLockObservation,
    maker_store: MemoryStore,
    taker_store: MemoryStore,
) {
    let swap_id = SwapId::new(id).expect("swap ID");
    let maker_replay = actor_sdk(
        Participant::Maker,
        wire.clone(),
        lez.clone(),
        zcash.clone(),
        maker_store,
    )
    .resume(&swap_id)
    .await
    .expect("maker restart")
    .expect("durable maker swap");
    let taker_recovery = actor_sdk(Participant::Taker, wire, lez, zcash, taker_store)
        .resume(&swap_id)
        .await
        .expect("taker restart")
        .expect("durable taker swap");
    assert_eq!(
        (maker_replay.status(), maker_replay.revision()),
        (Phase::BothLegsLocked, 2)
    );
    assert_eq!(
        (taker_recovery.status(), taker_recovery.revision()),
        (Phase::BothLegsLocked, 2)
    );
}

async fn project_actor_taker_first_lock(
    direction: SwapDirection,
    taker: &mut ActorActive,
    lez: &MemoryLezTakerLockObservation,
    zcash: &MemoryZcashTakerLockObservation,
) {
    let expected_evidence = match direction {
        SwapDirection::TakerSellsForeign => {
            let plan = zcash_first_lock_plan([0x31; 32], vec![0x51]);
            taker
                .stage_first_lock(plan.clone())
                .await
                .expect("stage Zcash");
            taker.drive_first_lock().await.expect("submit Zcash");
            let funding_transaction_id = canonical_zcash_taker_lock(taker.agreement())
                .transaction_id()
                .to_string();
            let evidence = confirmed_zcash_first_lock([0x31; 32], &funding_transaction_id);
            zcash.1.observe_as(
                FirstLockStepV1::ZcashFund,
                FirstLockObservation::Confirmed(evidence.clone()),
            );
            evidence
        }
        SwapDirection::TakerSellsLez => {
            let plan = lez_first_lock_plan([0x41; 32], vec![0x61], [0x42; 32], vec![0x62]);
            taker
                .stage_first_lock(plan.clone())
                .await
                .expect("stage LEZ");
            taker
                .drive_first_lock()
                .await
                .expect("submit LEZ initialize");
            lez.2.observe_as(
                FirstLockStepV1::LezInitialize,
                confirmed_first_lock_observation(
                    FirstLockStepV1::LezInitialize,
                    [0x41; 32],
                    "actor-taker-lez-initialize",
                ),
            );
            taker.drive_first_lock().await.expect("submit LEZ fund");
            let evidence =
                confirmed_first_lock(FirstLockStepV1::LezFund, [0x42; 32], "actor-taker-lez");
            lez.2.observe_as(
                FirstLockStepV1::LezFund,
                FirstLockObservation::Confirmed(evidence.clone()),
            );
            evidence
        }
    };
    let FirstLockDriveOutcome::ReadyForFundingProjection(observed_evidence) =
        taker.drive_first_lock().await.expect("observe exact plan")
    else {
        panic!("final adapter observation must be ready for projection");
    };
    assert_eq!(observed_evidence, expected_evidence);
    taker
        .project_first_lock(observed_evidence)
        .await
        .expect("project taker lock");
}

fn expose_taker_lock_to_maker(
    direction: SwapDirection,
    maker: &ActorActive,
    lez: &MemoryLezTakerLockObservation,
    zcash: &MemoryZcashTakerLockObservation,
) {
    match direction {
        SwapDirection::TakerSellsForeign => {
            zcash
                .0
                .respond(Ok(TakerFirstLockObservationV1::CanonicalZcash(Box::new(
                    canonical_zcash_taker_lock(maker.agreement()),
                ))));
        }
        SwapDirection::TakerSellsLez => {
            lez.0
                .respond(Ok(TakerFirstLockObservationV1::CanonicalLez(Box::new(
                    canonical_lez_taker_lock(maker.agreement()),
                ))));
        }
    }
}

async fn project_actor_maker_second_lock(
    direction: SwapDirection,
    maker: &mut ActorActive,
    lez: &MemoryLezTakerLockObservation,
    zcash: &MemoryZcashTakerLockObservation,
) -> FirstLockConfirmedEvidenceV1 {
    let evidence = match direction {
        SwapDirection::TakerSellsForeign => {
            let plan = lez_first_lock_plan([0x71; 32], vec![0xa1], [0x72; 32], vec![0xa2]);
            maker
                .drive_maker_lock(plan.clone())
                .await
                .expect("submit maker initialize");
            lez.2.observe_as(
                FirstLockStepV1::LezInitialize,
                confirmed_first_lock_observation(
                    FirstLockStepV1::LezInitialize,
                    [0x71; 32],
                    "actor-maker-lez-initialize",
                ),
            );
            maker
                .drive_maker_lock(plan.clone())
                .await
                .expect("submit maker LEZ fund");
            let expected =
                confirmed_first_lock(FirstLockStepV1::LezFund, [0x72; 32], "actor-maker-lez");
            lez.2.observe_as(
                FirstLockStepV1::LezFund,
                FirstLockObservation::Confirmed(expected.clone()),
            );
            let MakerLockDriveOutcome::Lock(FirstLockDriveOutcome::ReadyForFundingProjection(
                observed,
            )) = maker
                .drive_maker_lock(plan)
                .await
                .expect("observe maker LEZ fund")
            else {
                panic!("maker LEZ evidence must be ready for projection");
            };
            assert_eq!(observed, expected);
            observed
        }
        SwapDirection::TakerSellsLez => {
            let plan = zcash_first_lock_plan([0x81; 32], vec![0xb1]);
            maker
                .drive_maker_lock(plan.clone())
                .await
                .expect("submit maker Zcash");
            let funding_transaction_id = canonical_zcash_taker_lock(maker.agreement())
                .transaction_id()
                .to_string();
            let expected = confirmed_zcash_first_lock([0x81; 32], &funding_transaction_id);
            zcash.1.observe_as(
                FirstLockStepV1::ZcashFund,
                FirstLockObservation::Confirmed(expected.clone()),
            );
            let MakerLockDriveOutcome::Lock(FirstLockDriveOutcome::ReadyForFundingProjection(
                observed,
            )) = maker
                .drive_maker_lock(plan)
                .await
                .expect("observe maker Zcash fund")
            else {
                panic!("maker Zcash evidence must be ready for projection");
            };
            assert_eq!(observed, expected);
            observed
        }
    };
    maker
        .project_maker_lock(evidence.clone())
        .await
        .expect("project maker lock");
    evidence
}

fn expose_maker_lock_to_taker(
    direction: SwapDirection,
    evidence: FirstLockConfirmedEvidenceV1,
    lez: &MemoryLezTakerLockObservation,
    zcash: &MemoryZcashTakerLockObservation,
) {
    match direction {
        SwapDirection::TakerSellsForeign => lez
            .3
            .respond(Ok(MakerLockObservationV1::Confirmed(evidence))),
        SwapDirection::TakerSellsLez => zcash
            .2
            .respond(Ok(MakerLockObservationV1::Confirmed(evidence))),
    }
}

#[test]
fn claim_preimage_is_redacted_and_not_a_wire_record() {
    let preimage = ClaimPreimage::new([0x42; 32]);
    assert_eq!(preimage.expose_secret(), &[0x42; 32]);
    assert_eq!(format!("{preimage:?}"), "ClaimPreimage([REDACTED])");
}

/// The Taker's LEZ lock seen again at a deeper tip is the same lock, deeper:
/// the Maker's funding eligibility must proceed from it, not send the Maker
/// back to "poll again" (on a chain anything else moves, that never ended).
#[tokio::test]
async fn maker_funds_when_the_same_lez_lock_is_observed_deeper() {
    let id = "sdk-maker-funds-deeper-lez-lock";
    let wire = agreement_wire(id, SwapDirection::TakerSellsLez, FixtureVariant::Local);
    let store = MemoryStore::default();
    let lez = MemoryLezTakerLockObservation::default();
    let zcash = MemoryZcashTakerLockObservation::default();
    let sdk = maker_lock_sdk(wire, lez.clone(), zcash.clone(), store);
    let accepted = sdk
        .negotiate_at(&Offer(1), Proposal, ACCEPTED_AT)
        .await
        .expect("maker agreement");
    let mut active = sdk.activate(accepted).await.expect("maker activation");
    lez.0
        .respond(Ok(TakerFirstLockObservationV1::CanonicalLez(Box::new(
            canonical_lez_taker_lock(active.agreement()),
        ))));
    assert!(matches!(
        active.observe_taker_first_lock().await,
        Ok(ObserveTakerFirstLockOutcome::Projected(_))
    ));
    assert_eq!(
        (active.status(), active.revision()),
        (Phase::TakerLockConfirmed, 1)
    );
    // The same transaction in the same block, the tip one block further and
    // the inclusion now finalized: progress of the lock the Maker already knows.
    let deeper = CanonicalLezEscrowObservationV1::validate(
        active.agreement(),
        &canonical_lez_snapshot(active.agreement(), LezSnapshotMutation::DeeperFinalized),
    )
    .expect("deeper canonical observation");
    lez.0
        .respond(Ok(TakerFirstLockObservationV1::CanonicalLez(Box::new(
            deeper,
        ))));
    let plan = zcash_first_lock_plan([0x91; 32], vec![0xc1]);
    assert_eq!(
        active
            .drive_maker_lock(plan)
            .await
            .expect("the deeper head is projected and funding proceeds"),
        MakerLockDriveOutcome::Lock(FirstLockDriveOutcome::Submitted(FirstLockStepV1::ZcashFund))
    );
    assert_eq!(
        active.revision(),
        2,
        "the deeper head was projected durably"
    );
    assert_eq!(
        zcash.1.submissions(),
        vec![(FirstLockStepV1::ZcashFund, vec![0xc1])]
    );
}

/// A replaced LEZ lock (another transaction in another block) is a changed
/// canonical state: the Maker projects it and polls again before funding.
#[tokio::test]
async fn maker_polls_again_when_the_lez_lock_is_replaced() {
    let id = "sdk-maker-replaced-lez-lock";
    let wire = agreement_wire(id, SwapDirection::TakerSellsLez, FixtureVariant::Local);
    let store = MemoryStore::default();
    let lez = MemoryLezTakerLockObservation::default();
    let zcash = MemoryZcashTakerLockObservation::default();
    let sdk = maker_lock_sdk(wire, lez.clone(), zcash.clone(), store);
    let accepted = sdk
        .negotiate_at(&Offer(1), Proposal, ACCEPTED_AT)
        .await
        .expect("maker agreement");
    let mut active = sdk.activate(accepted).await.expect("maker activation");
    let canonical = canonical_lez_taker_lock(active.agreement());
    lez.0
        .respond(Ok(TakerFirstLockObservationV1::CanonicalLez(Box::new(
            canonical.clone(),
        ))));
    assert!(matches!(
        active.observe_taker_first_lock().await,
        Ok(ObserveTakerFirstLockOutcome::Projected(_))
    ));
    let removed = canonical_lez_removal_for(active.agreement(), &canonical, [0xbb; 32], 104);
    let replacement = CanonicalLezEscrowObservationV1::validate(
        active.agreement(),
        &canonical_lez_snapshot(active.agreement(), LezSnapshotMutation::ReplacementSafe),
    )
    .expect("replacement observation");
    lez.0.respond(Ok(TakerFirstLockObservationV1::LezReplaced {
        removed: Box::new(removed),
        canonical: Box::new(replacement),
    }));
    let plan = zcash_first_lock_plan([0x92; 32], vec![0xc2]);
    assert!(matches!(
        active.drive_maker_lock(plan).await,
        Ok(MakerLockDriveOutcome::AwaitingEligibility(
            MakerFundingEligibilityOutcome::CanonicalStateChanged(_)
        ))
    ));
    assert!(zcash.1.submissions().is_empty());
}
