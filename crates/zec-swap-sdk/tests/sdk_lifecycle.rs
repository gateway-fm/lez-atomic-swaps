use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use lez_swap_core::{Participant, Phase, SwapDirection, SwapId, UnixSeconds};
use lez_zec_swap_sdk::{
    AcceptedZecAgreementEnvelopeV1, ActiveZecSwap, Bip199Contract, ClaimPreimage,
    CreateAgreementOutcome, CreateFirstLockOutcome, ExpectedBip199Output, FirstLockDriveOutcome,
    FirstLockIntentV1, FirstLockObservation, FirstLockPlanV1, FirstLockStepV1, LezAssetV1,
    LezChainIdentityV1, LezEnvironmentV1, LezFirstLockPort, MAX_ZEC_AGREEMENT_RECORD_BYTES,
    NegotiationChannel, NegotiationTranscriptV1, OfferDiscovery, PreparedFirstLockSubmissionV1,
    RecoveryStore, ZEC_CONCRETE_AGREEMENT_SCHEMA_V1, ZcashFirstLockPort,
    ZcashTransparentDestinationV1, ZecAgreementBodyV1, ZecAgreementRecordV1, ZecAgreementV1Error,
    ZecLezTermsV1, ZecLifecycleAction, ZecPairSdk, ZecParticipantIdentityV1, ZecParticipantsV1,
    ZecProfileId, ZecProfileRecordV1, ZecRefundPlanV1, ZecSdkError, ZecSwapBinding,
    ZecSwapBindingRecordV1, ZecTransactionPolicyV1, derive_lez_metadata_account_v1,
    derive_lez_native_custody_account_v1, derive_lez_swap_id_v1,
};
use secp256k1::{Message, PublicKey, Secp256k1, SecretKey};
use zcash_protocol::{
    consensus::{BranchId, NetworkType},
    value::Zatoshis,
};
use zcash_transparent::address::TransparentAddress;

const ACCEPTED_AT: UnixSeconds = UnixSeconds::new(10);

#[derive(Clone, Debug, Eq, PartialEq)]
struct Offer(u64);

#[derive(Clone, Debug)]
struct Proposal;

#[derive(Clone, Debug, Default)]
struct MemoryDiscovery {
    offers: Arc<Mutex<Vec<Offer>>>,
}

#[async_trait]
impl OfferDiscovery for MemoryDiscovery {
    type Error = TestPortError;
    type Offer = Offer;
    type OfferRef = Offer;
    type Query = ();

    async fn publish(&self, offer: Self::Offer) -> Result<Self::OfferRef, Self::Error> {
        self.offers.lock().expect("offers lock").push(offer.clone());
        Ok(offer)
    }

    async fn discover(&self, _query: &Self::Query) -> Result<Vec<Self::OfferRef>, Self::Error> {
        Ok(self.offers.lock().expect("offers lock").clone())
    }
}

#[derive(Clone)]
struct MemoryNegotiation {
    wire: Vec<u8>,
}

impl std::fmt::Debug for MemoryNegotiation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MemoryNegotiation")
            .field("wire", &"[REDACTED]")
            .finish()
    }
}

#[async_trait]
impl NegotiationChannel for MemoryNegotiation {
    type Error = TestPortError;
    type LocalProposal = Proposal;
    type OfferRef = Offer;

    async fn negotiate(
        &self,
        _local_participant: Participant,
        _offer: &Self::OfferRef,
        _proposal: Self::LocalProposal,
    ) -> Result<Vec<u8>, Self::Error> {
        Ok(self.wire.clone())
    }
}

type AgreementMap = HashMap<String, AcceptedZecAgreementEnvelopeV1>;

#[derive(Clone, Debug, Default)]
struct MemoryStore {
    agreements: Arc<Mutex<AgreementMap>>,
    first_locks: Arc<Mutex<HashMap<String, FirstLockIntentV1>>>,
    fail_create: bool,
}

impl MemoryStore {
    fn with_record(key: &SwapId, envelope: AcceptedZecAgreementEnvelopeV1) -> Self {
        let mut agreements = HashMap::new();
        agreements.insert(key.as_str().to_owned(), envelope);
        Self {
            agreements: Arc::new(Mutex::new(agreements)),
            ..Self::default()
        }
    }
}

#[async_trait]
impl RecoveryStore for MemoryStore {
    type Error = TestPortError;

    async fn create_agreement(
        &self,
        envelope: &AcceptedZecAgreementEnvelopeV1,
    ) -> Result<CreateAgreementOutcome, Self::Error> {
        if self.fail_create {
            return Err(TestPortError("forced create failure".to_owned()));
        }
        let accepted = lez_zec_swap_sdk::AcceptedZecAgreementV1::resume(envelope)
            .map_err(|error| TestPortError(error.to_string()))?;
        let key = accepted.agreement().coordinator().id().as_str().to_owned();
        let mut records = self.agreements.lock().expect("agreements lock");
        match records.get(&key) {
            None => {
                records.insert(key, envelope.clone());
                Ok(CreateAgreementOutcome::Created)
            }
            Some(existing) if existing == envelope => Ok(CreateAgreementOutcome::ExistingSame),
            Some(_) => Ok(CreateAgreementOutcome::Conflict),
        }
    }

    async fn load_agreement(
        &self,
        swap_id: &SwapId,
    ) -> Result<Option<AcceptedZecAgreementEnvelopeV1>, Self::Error> {
        Ok(self
            .agreements
            .lock()
            .expect("agreements lock")
            .get(swap_id.as_str())
            .cloned())
    }

    async fn create_first_lock_intent(
        &self,
        intent: &FirstLockIntentV1,
    ) -> Result<CreateFirstLockOutcome, Self::Error> {
        let key = intent.swap_id().as_str().to_owned();
        let mut records = self.first_locks.lock().expect("first-lock lock");
        match records.get(&key) {
            None => {
                records.insert(key, intent.clone());
                Ok(CreateFirstLockOutcome::Created)
            }
            Some(existing) if existing == intent => Ok(CreateFirstLockOutcome::ExistingSame),
            Some(_) => Ok(CreateFirstLockOutcome::Conflict),
        }
    }

    async fn load_first_lock_intent(
        &self,
        swap_id: &SwapId,
    ) -> Result<Option<FirstLockIntentV1>, Self::Error> {
        Ok(self
            .first_locks
            .lock()
            .expect("first-lock lock")
            .get(swap_id.as_str())
            .cloned())
    }
}

#[derive(Clone, Copy, Debug)]
struct NoopLez;

#[derive(Clone, Copy, Debug)]
struct NoopZcash;

type FirstLockSubmissions = Vec<(FirstLockStepV1, Vec<u8>)>;

#[derive(Clone, Debug, Default)]
struct MemoryFirstLockPort {
    observations: Arc<Mutex<HashMap<FirstLockStepV1, FirstLockObservation>>>,
    submissions: Arc<Mutex<FirstLockSubmissions>>,
}

impl MemoryFirstLockPort {
    fn observe_as(&self, step: FirstLockStepV1, observation: FirstLockObservation) {
        self.observations
            .lock()
            .expect("observations lock")
            .insert(step, observation);
    }

    fn submissions(&self) -> Vec<(FirstLockStepV1, Vec<u8>)> {
        self.submissions.lock().expect("submissions lock").clone()
    }
}

#[derive(Clone, Debug, Default)]
struct MemoryLezPort(MemoryFirstLockPort);

#[async_trait]
impl LezFirstLockPort for MemoryLezPort {
    type Error = TestPortError;

    async fn observe_first_lock(
        &self,
        _agreement: &lez_zec_swap_sdk::ZecAgreementV1,
        submission: &PreparedFirstLockSubmissionV1,
    ) -> Result<FirstLockObservation, Self::Error> {
        Ok(self
            .0
            .observations
            .lock()
            .expect("observations lock")
            .get(&submission.step())
            .copied()
            .unwrap_or(FirstLockObservation::Absent))
    }

    async fn submit_first_lock(
        &self,
        _agreement: &lez_zec_swap_sdk::ZecAgreementV1,
        submission: &PreparedFirstLockSubmissionV1,
    ) -> Result<(), Self::Error> {
        self.0
            .submissions
            .lock()
            .expect("submissions lock")
            .push((submission.step(), submission.exact_submission().to_vec()));
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
struct MemoryZcashPort(MemoryFirstLockPort);

#[async_trait]
impl ZcashFirstLockPort for MemoryZcashPort {
    type Error = TestPortError;

    async fn observe_first_lock(
        &self,
        _agreement: &lez_zec_swap_sdk::ZecAgreementV1,
        submission: &PreparedFirstLockSubmissionV1,
    ) -> Result<FirstLockObservation, Self::Error> {
        Ok(self
            .0
            .observations
            .lock()
            .expect("observations lock")
            .get(&submission.step())
            .copied()
            .unwrap_or(FirstLockObservation::Absent))
    }

    async fn submit_first_lock(
        &self,
        _agreement: &lez_zec_swap_sdk::ZecAgreementV1,
        submission: &PreparedFirstLockSubmissionV1,
    ) -> Result<(), Self::Error> {
        self.0
            .submissions
            .lock()
            .expect("submissions lock")
            .push((submission.step(), submission.exact_submission().to_vec()));
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{0}")]
struct TestPortError(String);

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

    zcash
        .0
        .observe_as(FirstLockStepV1::ZcashFund, FirstLockObservation::Confirmed);
    let restarted = first_lock_sdk(Participant::Taker, wire, lez, zcash.clone(), store);
    let resumed = restarted
        .resume(&SwapId::new("sdk-first-lock-zcash-restart").expect("id"))
        .await
        .expect("resume")
        .expect("active");
    assert_eq!(
        resumed.drive_first_lock().await.expect("observe first"),
        FirstLockDriveOutcome::ReadyForFundingProjection
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
        FirstLockObservation::Confirmed,
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

    lez.0
        .observe_as(FirstLockStepV1::LezFund, FirstLockObservation::Confirmed);
    assert_eq!(
        active.drive_first_lock().await.expect("both observed"),
        FirstLockDriveOutcome::ReadyForFundingProjection
    );
    assert_eq!(lez.0.submissions().len(), 2);
    assert_eq!(active.status(), Phase::Offered);
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
async fn untrusted_negotiation_wire_is_bounded_and_public_profile_fails_closed() {
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
    assert!(matches!(
        public.negotiate_at(&Offer(1), Proposal, ACCEPTED_AT).await,
        Err(ZecSdkError::InvalidAgreement(
            ZecAgreementV1Error::PublicTestnetDeploymentUnavailable
        ))
    ));
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

#[test]
fn claim_preimage_is_redacted_and_not_a_wire_record() {
    let preimage = ClaimPreimage::new([0x42; 32]);
    assert_eq!(preimage.expose_secret(), &[0x42; 32]);
    assert_eq!(format!("{preimage:?}"), "ClaimPreimage([REDACTED])");
}

fn sdk(
    participant: Participant,
    wire: Vec<u8>,
    store: MemoryStore,
) -> ZecPairSdk<MemoryDiscovery, MemoryNegotiation, NoopLez, NoopZcash, MemoryStore> {
    ZecPairSdk::new(
        participant,
        MemoryDiscovery::default(),
        MemoryNegotiation { wire },
        NoopLez,
        NoopZcash,
        store,
    )
}

fn first_lock_sdk(
    participant: Participant,
    wire: Vec<u8>,
    lez: MemoryLezPort,
    zcash: MemoryZcashPort,
    store: MemoryStore,
) -> ZecPairSdk<MemoryDiscovery, MemoryNegotiation, MemoryLezPort, MemoryZcashPort, MemoryStore> {
    ZecPairSdk::new(
        participant,
        MemoryDiscovery::default(),
        MemoryNegotiation { wire },
        lez,
        zcash,
        store,
    )
}

fn envelope(
    wire: Vec<u8>,
    participant: Participant,
    revision: u64,
) -> AcceptedZecAgreementEnvelopeV1 {
    AcceptedZecAgreementEnvelopeV1::from_durable_parts(wire, ACCEPTED_AT, participant, revision)
}

#[derive(Clone, Copy)]
enum FixtureVariant {
    Local,
    ChangedTranscript,
    Public,
    CorruptCommitment,
}

fn agreement_wire(id: &str, direction: SwapDirection, variant: FixtureVariant) -> Vec<u8> {
    let maker_secret = SecretKey::from_slice(&[1; 32]).expect("maker key");
    let taker_secret = SecretKey::from_slice(&[2; 32]).expect("taker key");
    let secp = Secp256k1::new();
    let maker_key = PublicKey::from_secret_key(&secp, &maker_secret).serialize();
    let taker_key = PublicKey::from_secret_key(&secp, &taker_secret).serialize();
    let (refund_key, claimant_key) = match direction {
        SwapDirection::TakerSellsForeign => (taker_key, maker_key),
        SwapDirection::TakerSellsLez => (maker_key, taker_key),
    };
    let refund_hash = pubkey_hash(&refund_key);
    let claimant_hash = pubkey_hash(&claimant_key);
    let FixtureDeployment {
        profile,
        environment,
        network,
        zcash_anchor,
        zcash_refund_lock,
        earlier_latest_ms,
        later_earliest,
    } = fixture_deployment(matches!(variant, FixtureVariant::Public));
    let escrow_program = [1; 8];
    let onchain_swap_id = derive_lez_swap_id_v1(id.as_bytes());
    let metadata_account = derive_lez_metadata_account_v1(&escrow_program, &onchain_swap_id);
    let custody_account = derive_lez_native_custody_account_v1(&escrow_program, &onchain_swap_id);
    let digest = [9; 32];
    let binding = fixture_binding(
        profile,
        network,
        zcash_refund_lock,
        refund_hash,
        claimant_hash,
        digest,
    );
    let body = ZecAgreementBodyV1::new(
        id.to_owned(),
        direction,
        ZecProfileRecordV1::from(profile),
        ZecParticipantsV1::new(
            ZecParticipantIdentityV1::new([3; 32], maker_key),
            ZecParticipantIdentityV1::new([4; 32], taker_key),
        ),
        digest,
        ZecLezTermsV1::new(
            LezChainIdentityV1::new(environment, [7; 32]),
            escrow_program,
            LezAssetV1::Native {
                authenticated_transfer_program_id: [2; 8],
            },
            42,
            metadata_account,
            custody_account,
        ),
        ZecSwapBindingRecordV1::from_binding(&binding),
        ZecTransactionPolicyV1::new(
            [12; 32],
            ZcashTransparentDestinationV1::p2pkh(refund_hash),
            10_000,
            1_000,
            ZcashTransparentDestinationV1::p2pkh(claimant_hash),
            10_000,
            ZcashTransparentDestinationV1::p2pkh(refund_hash),
            10_000,
            40,
        ),
        ZecRefundPlanV1::new(100, zcash_anchor, earlier_latest_ms, later_earliest),
        NegotiationTranscriptV1::new(
            [5; 32],
            if matches!(variant, FixtureVariant::ChangedTranscript) {
                [0x66; 32]
            } else {
                [6; 32]
            },
            1_000,
        ),
    );
    let commitment = body.commitment();
    let record = ZecAgreementRecordV1::from_parts(
        ZEC_CONCRETE_AGREEMENT_SCHEMA_V1,
        body,
        if matches!(variant, FixtureVariant::CorruptCommitment) {
            [0x44; 32]
        } else {
            commitment
        },
        secp.sign_ecdsa(&Message::from_digest(commitment), &maker_secret)
            .serialize_compact(),
        secp.sign_ecdsa(&Message::from_digest(commitment), &taker_secret)
            .serialize_compact(),
    );
    record.encode_wire().expect("bounded fixture wire")
}

struct FixtureDeployment {
    profile: ZecProfileId,
    environment: LezEnvironmentV1,
    network: NetworkType,
    zcash_anchor: u32,
    zcash_refund_lock: u32,
    earlier_latest_ms: u64,
    later_earliest: u64,
}

fn fixture_deployment(is_public: bool) -> FixtureDeployment {
    if is_public {
        FixtureDeployment {
            profile: ZecProfileId::PublicTestnetV1,
            environment: LezEnvironmentV1::PublicTestnetV0_2,
            network: NetworkType::Test,
            zcash_anchor: 100,
            zcash_refund_lock: 292,
            earlier_latest_ms: 7_300_000,
            later_earliest: 14_500,
        }
    } else {
        FixtureDeployment {
            profile: ZecProfileId::DeterministicLocalV1,
            environment: LezEnvironmentV1::DeterministicLocalV0_2,
            network: NetworkType::Regtest,
            zcash_anchor: 116,
            zcash_refund_lock: 120,
            earlier_latest_ms: 160_000,
            later_earliest: 200,
        }
    }
}

fn fixture_binding(
    profile: ZecProfileId,
    network: NetworkType,
    refund_lock: u32,
    refund_hash: [u8; 20],
    claimant_hash: [u8; 20],
    digest: [u8; 32],
) -> ZecSwapBinding {
    ZecSwapBinding::new(
        profile,
        ExpectedBip199Output::new(
            network,
            BranchId::Nu6_2,
            Zatoshis::from_u64(100_000_000).expect("value"),
            Bip199Contract::new(refund_lock, refund_hash, digest, claimant_hash),
        ),
    )
    .expect("binding")
}

fn pubkey_hash(bytes: &[u8; 33]) -> [u8; 20] {
    match TransparentAddress::from_pubkey(&PublicKey::from_slice(bytes).expect("fixture pubkey")) {
        TransparentAddress::PublicKeyHash(hash) => hash,
        TransparentAddress::ScriptHash(_) => unreachable!("public keys produce P2PKH"),
    }
}
