use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use lez_swap_core::{Participant, Phase, SwapDirection, SwapId, UnixSeconds};
use lez_zec_swap_sdk::{
    AcceptedZecAgreementEnvelopeV1, AcceptedZecAgreementV1, ActiveZecSwap, Bip199Contract,
    CanonicalZcashOutputObservation, CanonicalZcashOutputRemoval, ClaimPreimage,
    CreateAgreementOutcome, CreateFirstLockOutcome, ExpectedBip199Output,
    FirstLockConfirmedEvidenceV1, FirstLockDriveOutcome, FirstLockIntentRecordV1,
    FirstLockIntentV1, FirstLockObservation, FirstLockPlanV1, FirstLockProjectionCommit,
    FirstLockRecordError, FirstLockStepV1, FirstLockTransitionRecordV1, FirstLockTransitionV1,
    LezAssetV1, LezChainIdentityV1, LezEnvironmentV1, LezFirstLockPort,
    LezTakerFirstLockObservationPort, MAX_FIRST_LOCK_SUBMISSION_BYTES,
    MAX_ZEC_AGREEMENT_RECORD_BYTES, NegotiationChannel, NegotiationTranscriptV1,
    ObserveTakerFirstLockOutcome, ObservedTakerFirstLockEvidenceV1,
    ObservedTakerFirstLockTransitionV1, OfferDiscovery, PreparedFirstLockSubmissionV1,
    RecoveryStore, TakerFirstLockObservationV1, TransparentFundingRequest, TransparentUtxo,
    ZEC_CONCRETE_AGREEMENT_SCHEMA_V1, ZcashFirstLockPort, ZcashNodeRemovalSnapshot,
    ZcashNodeSnapshot, ZcashStableTip, ZcashTakerFirstLockObservationPort,
    ZcashTransparentDestinationV1, ZecAgreementBodyV1, ZecAgreementRecordV1, ZecAgreementV1Error,
    ZecLezTermsV1, ZecLifecycleAction, ZecPairSdk, ZecParticipantIdentityV1, ZecParticipantsV1,
    ZecProfileId, ZecProfileRecordV1, ZecRefundPlanV1, ZecSdkError, ZecSwapBinding,
    ZecSwapBindingRecordV1, ZecTransactionPolicyV1, build_funding_transaction,
    derive_lez_metadata_account_v1, derive_lez_native_custody_account_v1, derive_lez_swap_id_v1,
};
use secp256k1::{Message, PublicKey, Secp256k1, SecretKey};
use zcash_primitives::block::BlockHash;
use zcash_protocol::{
    consensus::{BlockHeight, BranchId, NetworkType},
    value::Zatoshis,
};
use zcash_transparent::{
    address::{Script, TransparentAddress},
    bundle::{OutPoint, TxOut},
};

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
    first_lock_transitions: Arc<Mutex<HashMap<(String, u64), FirstLockTransitionV1>>>,
    observed_taker_first_lock_transitions:
        Arc<Mutex<HashMap<(String, u64), ObservedTakerFirstLockTransitionV1>>>,
    transition_mode: Arc<Mutex<TransitionCommitMode>>,
    fail_create: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum TransitionCommitMode {
    #[default]
    Normal,
    FailBeforeCommit,
    CommitThenReportFailure,
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

    fn set_transition_mode(&self, mode: TransitionCommitMode) {
        *self.transition_mode.lock().expect("transition mode lock") = mode;
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

    async fn commit_first_lock_transition(
        &self,
        transition: &FirstLockTransitionV1,
    ) -> Result<FirstLockProjectionCommit, Self::Error> {
        let mode = *self.transition_mode.lock().expect("transition mode lock");
        if mode == TransitionCommitMode::FailBeforeCommit {
            return Err(TestPortError("forced transition failure".to_owned()));
        }
        let key = (
            transition.swap_id().as_str().to_owned(),
            transition.predecessor_revision(),
        );
        let mut transitions = self
            .first_lock_transitions
            .lock()
            .expect("first-lock transition lock");
        let was_replay = match transitions.get(&key) {
            None => {
                transitions.insert(key, transition.clone());
                false
            }
            Some(existing) if existing == transition => true,
            Some(_) => return Err(TestPortError("conflicting transition".to_owned())),
        };
        self.first_locks
            .lock()
            .expect("first-lock lock")
            .remove(transition.swap_id().as_str());
        if mode == TransitionCommitMode::CommitThenReportFailure {
            return Err(TestPortError("unknown successful commit".to_owned()));
        }
        Ok(FirstLockProjectionCommit::new(
            transition
                .predecessor_revision()
                .checked_add(1)
                .expect("test revision"),
            was_replay,
        ))
    }

    async fn load_first_lock_transition(
        &self,
        swap_id: &SwapId,
        predecessor_revision: u64,
    ) -> Result<Option<FirstLockTransitionV1>, Self::Error> {
        Ok(self
            .first_lock_transitions
            .lock()
            .expect("first-lock transition lock")
            .get(&(swap_id.as_str().to_owned(), predecessor_revision))
            .cloned())
    }

    async fn commit_observed_taker_first_lock_transition(
        &self,
        transition: &ObservedTakerFirstLockTransitionV1,
    ) -> Result<FirstLockProjectionCommit, Self::Error> {
        let mode = *self.transition_mode.lock().expect("transition mode lock");
        if mode == TransitionCommitMode::FailBeforeCommit {
            return Err(TestPortError("forced transition failure".to_owned()));
        }
        let key = (
            transition.swap_id().as_str().to_owned(),
            transition.predecessor_revision(),
        );
        let mut transitions = self
            .observed_taker_first_lock_transitions
            .lock()
            .expect("observed transition lock");
        let was_replay = match transitions.get(&key) {
            None => {
                transitions.insert(key, transition.clone());
                false
            }
            Some(existing) if existing == transition => true,
            Some(_) => return Err(TestPortError("conflicting transition".to_owned())),
        };
        if mode == TransitionCommitMode::CommitThenReportFailure {
            return Err(TestPortError("unknown successful commit".to_owned()));
        }
        Ok(FirstLockProjectionCommit::new(
            transition.predecessor_revision() + 1,
            was_replay,
        ))
    }

    async fn load_observed_taker_first_lock_transition(
        &self,
        swap_id: &SwapId,
        predecessor_revision: u64,
    ) -> Result<Option<ObservedTakerFirstLockTransitionV1>, Self::Error> {
        Ok(self
            .observed_taker_first_lock_transitions
            .lock()
            .expect("observed transition lock")
            .get(&(swap_id.as_str().to_owned(), predecessor_revision))
            .cloned())
    }
}

#[derive(Clone, Copy, Debug)]
struct NoopLez;

#[derive(Clone, Copy, Debug)]
struct NoopZcash;

#[derive(Clone, Debug)]
struct MemoryTakerLockObservation {
    response: Arc<Mutex<Result<TakerFirstLockObservationV1, TestPortError>>>,
    calls: Arc<Mutex<usize>>,
}

impl Default for MemoryTakerLockObservation {
    fn default() -> Self {
        Self {
            response: Arc::new(Mutex::new(Ok(TakerFirstLockObservationV1::Absent))),
            calls: Arc::new(Mutex::new(0)),
        }
    }
}

impl MemoryTakerLockObservation {
    fn respond(&self, response: Result<TakerFirstLockObservationV1, TestPortError>) {
        *self.response.lock().expect("observation response lock") = response;
    }

    fn calls(&self) -> usize {
        *self.calls.lock().expect("observation calls lock")
    }
}

#[derive(Clone, Debug, Default)]
struct MemoryLezTakerLockObservation(MemoryTakerLockObservation);

#[async_trait]
impl LezTakerFirstLockObservationPort for MemoryLezTakerLockObservation {
    type Error = TestPortError;

    async fn observe_taker_first_lock(
        &self,
        _agreement: &lez_zec_swap_sdk::ZecAgreementV1,
    ) -> Result<TakerFirstLockObservationV1, Self::Error> {
        *self.0.calls.lock().expect("observation calls lock") += 1;
        self.0
            .response
            .lock()
            .expect("observation response lock")
            .clone()
    }
}

#[derive(Clone, Debug, Default)]
struct MemoryZcashTakerLockObservation(MemoryTakerLockObservation);

#[async_trait]
impl ZcashTakerFirstLockObservationPort for MemoryZcashTakerLockObservation {
    type Error = TestPortError;

    async fn observe_taker_first_lock(
        &self,
        _agreement: &lez_zec_swap_sdk::ZecAgreementV1,
    ) -> Result<TakerFirstLockObservationV1, Self::Error> {
        *self.0.calls.lock().expect("observation calls lock") += 1;
        self.0
            .response
            .lock()
            .expect("observation response lock")
            .clone()
    }
}

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

async fn assert_forward_canonical_history_survives_restart(
    forward_id: &str,
    forward_wire: Vec<u8>,
    forward_lez: MemoryLezTakerLockObservation,
    forward_zcash: MemoryZcashTakerLockObservation,
    forward_store: MemoryStore,
    canonical: CanonicalZcashOutputObservation,
) {
    let reorg_zcash = forward_zcash.clone();
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

    let deeper = canonical_zcash_replacement_depth_update(restarted.agreement());
    reorg_zcash
        .0
        .respond(Ok(TakerFirstLockObservationV1::CanonicalZcash(Box::new(
            deeper.clone(),
        ))));
    restarted
        .observe_taker_first_lock()
        .await
        .expect("same-inclusion depth increase commits");
    assert_eq!(restarted.status(), Phase::TakerLockConfirmed);
    assert_eq!(restarted.revision(), 3);

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
async fn reverse_maker_observes_lez_while_taker_cannot_use_maker_observation() {
    let reverse_store = MemoryStore::default();
    let reverse_lez = MemoryLezTakerLockObservation::default();
    let reverse_zcash = MemoryZcashTakerLockObservation::default();
    reverse_lez.0.respond(Ok(confirmed_observed_taker_lock(
        FirstLockStepV1::LezFund,
        "confirmed-lez-lock",
        100,
    )));
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
        reverse_store,
    );
    let reverse_accepted = reverse_sdk
        .negotiate_at(&Offer(1), Proposal, ACCEPTED_AT)
        .await
        .expect("reverse agreement");
    let mut reverse_maker = reverse_sdk
        .activate(reverse_accepted)
        .await
        .expect("reverse maker activation");
    assert!(matches!(
        reverse_maker.observe_taker_first_lock().await,
        Ok(ObserveTakerFirstLockOutcome::Projected(_))
    ));
    assert_eq!(reverse_lez.0.calls(), 1);
    assert_eq!(reverse_zcash.0.calls(), 0);
    assert_eq!(reverse_maker.status(), Phase::TakerLockConfirmed);
    assert_eq!(
        reverse_maker.next_action(),
        ZecLifecycleAction::Wait,
        "provisional LEZ evidence must never authorize Zcash funding"
    );

    let taker_store = MemoryStore::default();
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
        taker_store,
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

fn confirmed_observed_taker_lock(
    step: FirstLockStepV1,
    transaction_id: &str,
    confirmations: u32,
) -> TakerFirstLockObservationV1 {
    TakerFirstLockObservationV1::Confirmed(
        ObservedTakerFirstLockEvidenceV1::new(step, transaction_id, confirmations)
            .expect("well-formed primitive evidence"),
    )
}

fn canonical_zcash_taker_lock(
    agreement: &lez_zec_swap_sdk::ZecAgreementV1,
) -> CanonicalZcashOutputObservation {
    canonical_zcash_taker_lock_at_depth(agreement, [9; 32], 3)
}

fn canonical_zcash_taker_lock_with_input(
    agreement: &lez_zec_swap_sdk::ZecAgreementV1,
    input_transaction_id: [u8; 32],
) -> CanonicalZcashOutputObservation {
    canonical_zcash_taker_lock_at_depth(agreement, input_transaction_id, 3)
}

fn canonical_zcash_taker_lock_at_depth(
    agreement: &lez_zec_swap_sdk::ZecAgreementV1,
    input_transaction_id: [u8; 32],
    confirmations: u32,
) -> CanonicalZcashOutputObservation {
    canonical_zcash_taker_lock_fixture(
        agreement,
        input_transaction_id,
        BlockHash([0x44; 32]),
        BlockHash([0xaa; 32]),
        BlockHeight::from_u32(100 + confirmations - 1),
    )
}

fn canonical_zcash_replacement(
    agreement: &lez_zec_swap_sdk::ZecAgreementV1,
    removed: &CanonicalZcashOutputRemoval,
) -> CanonicalZcashOutputObservation {
    canonical_zcash_taker_lock_fixture(
        agreement,
        [8; 32],
        removed.canonical_block_hash_at_removed_height(),
        removed.tip_block_hash(),
        removed.tip_height(),
    )
}

fn canonical_zcash_replacement_depth_update(
    agreement: &lez_zec_swap_sdk::ZecAgreementV1,
) -> CanonicalZcashOutputObservation {
    canonical_zcash_taker_lock_fixture(
        agreement,
        [8; 32],
        BlockHash([0x55; 32]),
        BlockHash([0xcc; 32]),
        BlockHeight::from_u32(104),
    )
}

fn canonical_zcash_taker_lock_fixture(
    agreement: &lez_zec_swap_sdk::ZecAgreementV1,
    input_transaction_id: [u8; 32],
    transaction_block_hash: BlockHash,
    tip_block_hash: BlockHash,
    tip_height: BlockHeight,
) -> CanonicalZcashOutputObservation {
    let expected = agreement.binding().expected_output();
    let key = SecretKey::from_slice(&[7; 32]).expect("canonical observation owner key");
    let public_key = PublicKey::from_secret_key(&Secp256k1::new(), &key);
    let owner_script: Script = TransparentAddress::from_pubkey(&public_key).script().into();
    let input_value = u64::from(expected.value())
        .checked_add(20_000)
        .expect("fixture input value");
    let request = TransparentFundingRequest::new(
        vec![TransparentUtxo::new(
            OutPoint::new(input_transaction_id, 0),
            TxOut::new(
                Zatoshis::from_u64(input_value).expect("fixture input"),
                owner_script,
            ),
        )],
        public_key,
        expected.value(),
        Zatoshis::from_u64(1_000).expect("fee"),
        Zatoshis::from_u64(1_000).expect("change floor"),
        BlockHeight::from_u32(4_100_000),
        expected.consensus_branch_id(),
    )
    .expect("canonical funding request");
    let transaction = build_funding_transaction(expected.contract(), &request, &key)
        .expect("funding transaction");
    let mut raw = Vec::new();
    transaction.write(&mut raw).expect("canonical bytes");
    CanonicalZcashOutputObservation::validate(
        expected,
        &ZcashNodeSnapshot::new(
            expected.network(),
            expected.consensus_branch_id(),
            true,
            transaction_block_hash,
            transaction_block_hash,
            BlockHeight::from_u32(100),
            ZcashStableTip::new(tip_block_hash, tip_height, tip_block_hash, tip_height),
            transaction.txid(),
            raw,
            0,
            u32::from(tip_height) - 100 + 1,
        ),
    )
    .expect("agreement-bound canonical observation")
}

fn canonical_zcash_removal(
    previous: &CanonicalZcashOutputObservation,
) -> CanonicalZcashOutputRemoval {
    let (replacement_block_hash, tip_block_hash) = if previous.block_hash() == BlockHash([0x44; 32])
    {
        (BlockHash([0x55; 32]), BlockHash([0xbb; 32]))
    } else {
        (BlockHash([0x66; 32]), BlockHash([0xdd; 32]))
    };
    let tip_height = BlockHeight::from_u32(u32::from(previous.tip_height()) + 1);
    CanonicalZcashOutputRemoval::validate(
        previous,
        &ZcashNodeRemovalSnapshot::new(
            previous.network(),
            previous.consensus_branch_id(),
            replacement_block_hash,
            ZcashStableTip::new(tip_block_hash, tip_height, tip_block_hash, tip_height),
        ),
    )
    .expect("affirmative stable canonical removal")
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

fn zcash_first_lock_plan(
    expected_submission_id: [u8; 32],
    exact_submission: Vec<u8>,
) -> FirstLockPlanV1 {
    FirstLockPlanV1::zcash(
        PreparedFirstLockSubmissionV1::new(
            FirstLockStepV1::ZcashFund,
            expected_submission_id,
            exact_submission,
        )
        .expect("submission"),
    )
    .expect("plan")
}

fn confirmed_zcash_first_lock(
    expected_submission_id: [u8; 32],
    transaction_id: &str,
) -> FirstLockConfirmedEvidenceV1 {
    FirstLockConfirmedEvidenceV1::new(
        FirstLockStepV1::ZcashFund,
        expected_submission_id,
        transaction_id.to_owned(),
        100,
    )
    .expect("confirmed evidence")
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
