//! In-memory store, simulated chain ports and fixture builders shared by the
//! lifecycle tests and `examples/full-lifecycle.rs` (both include this file
//! with `#[path]`). Deterministic, no I/O; a simulation, not a reference port.
#![allow(dead_code)]

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use lez_swap_core::{Pair, Participant, SwapCoordinator, SwapDirection, SwapId, UnixSeconds};
use lez_zec_swap_sdk::{
    AcceptedZecAgreementEnvelopeV1, AcceptedZecAgreementV1, Bip199Contract,
    CanonicalLezEscrowObservationV1, CanonicalLezEscrowRemovalV1, CanonicalZcashOutputObservation,
    CanonicalZcashOutputRemoval, ClaimIntentRecordV1, ClaimIntentV1, ClaimMaterialContext,
    ClaimMaterialPurpose, ClaimPreimage, ClaimRecoveryStore, ClaimStepV1, ClaimSubmissionContext,
    CreateAgreementOutcome, CreateFirstLockOutcome, ExpectedBip199Output,
    FirstLockConfirmedEvidenceV1, FirstLockIntentV1, FirstLockObservation, FirstLockPlanV1,
    FirstLockProjectionCommit, FirstLockStepV1, FirstLockSubmitOutcome, FirstLockSupersessionV1,
    FirstLockTransitionV1, FollowupClaimEvidenceV1, FollowupClaimObservationV1,
    FollowupClaimTransitionRecordV1, FollowupClaimTransitionV1, LezAssetV1, LezChainIdentityV1,
    LezClaimInstructionV1, LezClaimNodeSnapshotV1, LezClaimPort, LezClaimTransactionSnapshotV1,
    LezCustodySnapshotV1, LezEnvironmentV1, LezEscrowMetadataSnapshotV1, LezEscrowStatusV1,
    LezFirstLockPort, LezFundInstructionV1, LezFundTransactionSnapshotV1, LezInclusionStatusV1,
    LezMakerLockObservationPort, LezNodeRemovalSnapshotV1, LezNodeSnapshotV1, LezRefundPort,
    LezStableTipV1, LezTakerFirstLockObservationPort, MakerLockIntentV1, MakerLockObservationV1,
    MakerLockTransitionV1, NegotiationChannel, NegotiationTranscriptV1,
    ObservedFollowupClaimTransitionRecordV1, ObservedFollowupClaimTransitionV1,
    ObservedMakerLockTransitionV1, ObservedRevealingClaimTransitionRecordV1,
    ObservedRevealingClaimTransitionV1, ObservedTakerFirstLockEvidenceV1,
    ObservedTakerFirstLockTransitionV1, OfferDiscovery, PROTECTED_CLAIM_SCHEMA_V1,
    PreparedClaimSubmissionV1, PreparedFirstLockSubmissionV1, PreparedRefundSubmissionV1,
    ProtectedClaimEnvelope, ProtectedClaimKey, ProtectedClaimPayloadEnvelope, RecoveryStore,
    RefundEligibilityObservationV1, RefundEvidenceV1, RefundIntentV1, RefundObservationV1,
    RefundRecoveryStore, RefundStepV1, RefundSubmitOutcomeV1, RefundTransitionV1,
    RevealingClaimEvidenceV1, RevealingClaimObservationV1, RevealingClaimTransitionRecordV1,
    RevealingClaimTransitionV1, TakerFirstLockObservationV1, TransparentFundingRequest,
    TransparentUtxo, ZEC_CONCRETE_AGREEMENT_SCHEMA_V2, ZcashClaimContextV1, ZcashClaimPort,
    ZcashFirstLockPort, ZcashFundingContextV1, ZcashFundingObservationV1,
    ZcashMakerLockObservationPort, ZcashNodeRemovalSnapshot, ZcashNodeSnapshot, ZcashRefundPort,
    ZcashStableTip, ZcashTakerFirstLockObservationPort, ZcashTransparentDestinationV1,
    ZcashUnspentOutputSnapshotV1, ZecAgreementBodyV1, ZecAgreementRecordV1, ZecLezTermsV1,
    ZecPairSdk, ZecParticipantIdentityV1, ZecParticipantsV1, ZecProfileId, ZecProfileRecordV1,
    ZecRefundPlanV1, ZecSwapBinding, ZecSwapBindingRecordV1, ZecTransactionPolicyV1,
    build_funding_transaction, derive_lez_metadata_account_v1,
    derive_lez_native_custody_account_v1, derive_lez_swap_id_v1,
};
use secp256k1::{Message, PublicKey, Secp256k1, SecretKey};
use sha2::{Digest as _, Sha256};
use zcash_primitives::block::BlockHash;
use zcash_protocol::{
    consensus::{BlockHeight, BranchId, NetworkType},
    value::Zatoshis,
};
use zcash_transparent::{
    address::{Script, TransparentAddress},
    bundle::{OutPoint, TxOut},
};

pub(crate) const ACCEPTED_AT: UnixSeconds = UnixSeconds::new(10);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Offer(pub(crate) u64);

#[derive(Clone, Debug)]
pub(crate) struct Proposal;

#[derive(Clone, Debug, Default)]
pub(crate) struct MemoryDiscovery {
    pub(crate) offers: Arc<Mutex<Vec<Offer>>>,
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
pub(crate) struct MemoryNegotiation {
    pub(crate) wire: Vec<u8>,
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

pub(crate) type AgreementMap = HashMap<String, AcceptedZecAgreementEnvelopeV1>;
pub(crate) type ClaimMaterialMap = HashMap<String, ProtectedClaimEnvelope>;
pub(crate) type ClaimIntentMap = HashMap<String, (ClaimIntentV1, ProtectedClaimPayloadEnvelope)>;
pub(crate) type RevealingClaimTransitionMap = HashMap<
    (String, u64),
    (
        RevealingClaimTransitionRecordV1,
        ClaimIntentRecordV1,
        ProtectedClaimEnvelope,
        ClaimMaterialPurpose,
    ),
>;
pub(crate) type FollowupClaimTransitionMap =
    HashMap<(String, u64), (FollowupClaimTransitionRecordV1, ClaimIntentRecordV1)>;
pub(crate) type ObservedRevealingClaimTransitionMap = HashMap<
    (String, u64),
    (
        ObservedRevealingClaimTransitionRecordV1,
        ProtectedClaimEnvelope,
        ClaimMaterialPurpose,
    ),
>;
pub(crate) type ObservedFollowupClaimTransitionMap =
    HashMap<(String, u64), ObservedFollowupClaimTransitionRecordV1>;
pub(crate) type RefundIntentMap = HashMap<String, RefundIntentV1>;
pub(crate) type RefundTransitionMap = HashMap<(String, u64), RefundTransitionV1>;

#[derive(Clone, Debug, Default)]
pub(crate) struct MemoryStore {
    pub(crate) agreements: Arc<Mutex<AgreementMap>>,
    pub(crate) claim_materials: Arc<Mutex<ClaimMaterialMap>>,
    pub(crate) claim_material_purposes: Arc<Mutex<HashMap<String, ClaimMaterialPurpose>>>,
    pub(crate) claim_intents: Arc<Mutex<ClaimIntentMap>>,
    pub(crate) revealing_claim_transitions: Arc<Mutex<RevealingClaimTransitionMap>>,
    pub(crate) followup_claim_transitions: Arc<Mutex<FollowupClaimTransitionMap>>,
    pub(crate) observed_revealing_claim_transitions:
        Arc<Mutex<ObservedRevealingClaimTransitionMap>>,
    pub(crate) observed_followup_claim_transitions: Arc<Mutex<ObservedFollowupClaimTransitionMap>>,
    pub(crate) refund_intents: Arc<Mutex<RefundIntentMap>>,
    pub(crate) refund_transitions: Arc<Mutex<RefundTransitionMap>>,
    pub(crate) claim_nonce_sequence: Arc<Mutex<u64>>,
    pub(crate) first_locks: Arc<Mutex<HashMap<String, FirstLockIntentV1>>>,
    pub(crate) supersessions: Arc<Mutex<HashMap<String, Vec<FirstLockSupersessionV1>>>>,
    pub(crate) first_lock_transitions: Arc<Mutex<HashMap<(String, u64), FirstLockTransitionV1>>>,
    pub(crate) observed_taker_first_lock_transitions:
        Arc<Mutex<HashMap<(String, u64), ObservedTakerFirstLockTransitionV1>>>,
    pub(crate) observed_maker_lock_transitions:
        Arc<Mutex<HashMap<(String, u64), ObservedMakerLockTransitionV1>>>,
    pub(crate) maker_locks: Arc<Mutex<HashMap<String, MakerLockIntentV1>>>,
    pub(crate) maker_lock_transitions: Arc<Mutex<HashMap<(String, u64), MakerLockTransitionV1>>>,
    pub(crate) transition_mode: Arc<Mutex<TransitionCommitMode>>,
    pub(crate) fail_create: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum TransitionCommitMode {
    #[default]
    Normal,
    FailBeforeCommit,
    CommitThenReportFailure,
}

impl MemoryStore {
    pub(crate) fn with_record(key: &SwapId, envelope: AcceptedZecAgreementEnvelopeV1) -> Self {
        let mut agreements = HashMap::new();
        agreements.insert(key.as_str().to_owned(), envelope);
        Self {
            agreements: Arc::new(Mutex::new(agreements)),
            ..Self::default()
        }
    }

    pub(crate) fn set_transition_mode(&self, mode: TransitionCommitMode) {
        *self.transition_mode.lock().expect("transition mode lock") = mode;
    }

    pub(crate) fn next_claim_nonce(&self, domain: &[u8]) -> [u8; 24] {
        let mut sequence = self
            .claim_nonce_sequence
            .lock()
            .expect("claim nonce sequence lock");
        *sequence = sequence.checked_add(1).expect("test nonce sequence");
        let mut digest = Sha256::new();
        digest.update(b"lez-zec-sdk-test/claim-payload-nonce/v1");
        digest.update(domain);
        digest.update(sequence.to_le_bytes());
        let digest = digest.finalize();
        let mut nonce = [0_u8; 24];
        nonce.copy_from_slice(&digest[..24]);
        nonce
    }

    pub(crate) fn accepted(
        &self,
        swap_id: &SwapId,
    ) -> Result<AcceptedZecAgreementV1, TestPortError> {
        let envelope = self
            .agreements
            .lock()
            .expect("agreements lock")
            .get(swap_id.as_str())
            .cloned()
            .ok_or_else(|| TestPortError("missing accepted agreement".to_owned()))?;
        AcceptedZecAgreementV1::resume(&envelope).map_err(|error| TestPortError(error.to_string()))
    }

    pub(crate) fn coordinator_at(
        &self,
        accepted: &AcceptedZecAgreementV1,
        target_revision: u64,
    ) -> Result<SwapCoordinator, TestPortError> {
        let agreement = accepted.agreement();
        let swap_key = agreement.coordinator().id().as_str().to_owned();
        let mut coordinator = agreement.coordinator().clone();
        for revision in 0..target_revision {
            let key = (swap_key.clone(), revision);
            let next = self.apply_lock_transition(agreement, &coordinator, revision, &key)?;
            coordinator = match next {
                Some(next) => next,
                None => {
                    match self.apply_claim_transition(accepted, &coordinator, revision, &key)? {
                        Some(next) => next,
                        None => self
                            .apply_refund_transition(agreement, &coordinator, revision, &key)?
                            .ok_or_else(|| {
                                TestPortError(format!("missing transition at revision {revision}"))
                            })?,
                    }
                }
            };
        }
        Ok(coordinator)
    }

    pub(crate) fn apply_lock_transition(
        &self,
        agreement: &lez_zec_swap_sdk::ZecAgreementV1,
        coordinator: &SwapCoordinator,
        revision: u64,
        key: &(String, u64),
    ) -> Result<Option<SwapCoordinator>, TestPortError> {
        if let Some(transition) = self
            .first_lock_transitions
            .lock()
            .expect("first-lock transitions lock")
            .get(key)
            .cloned()
        {
            return transition
                .apply_to(agreement, coordinator, revision)
                .map(Some)
                .map_err(|error| TestPortError(error.to_string()));
        }
        if let Some(transition) = self
            .observed_taker_first_lock_transitions
            .lock()
            .expect("observed taker transitions lock")
            .get(key)
            .cloned()
        {
            return transition
                .apply_to(agreement, coordinator, revision)
                .map(Some)
                .map_err(|error| TestPortError(error.to_string()));
        }
        if let Some(transition) = self
            .maker_lock_transitions
            .lock()
            .expect("maker transitions lock")
            .get(key)
            .cloned()
        {
            return transition
                .apply_to(agreement, coordinator, revision)
                .map(Some)
                .map_err(|error| TestPortError(error.to_string()));
        }
        self.observed_maker_lock_transitions
            .lock()
            .expect("observed maker transitions lock")
            .get(key)
            .cloned()
            .map_or(Ok(None), |transition| {
                transition
                    .apply_to(agreement, coordinator, revision)
                    .map(Some)
                    .map_err(|error| TestPortError(error.to_string()))
            })
    }

    pub(crate) fn apply_claim_transition(
        &self,
        accepted: &AcceptedZecAgreementV1,
        coordinator: &SwapCoordinator,
        revision: u64,
        key: &(String, u64),
    ) -> Result<Option<SwapCoordinator>, TestPortError> {
        let agreement = accepted.agreement();
        if let Some((record, intent, protected, purpose)) = self
            .revealing_claim_transitions
            .lock()
            .expect("revealing transitions lock")
            .get(key)
            .cloned()
        {
            let preimage = protected
                .decrypt(
                    &memory_claim_key(),
                    claim_material_context_for(accepted, purpose),
                )
                .map_err(|error| TestPortError(error.to_string()))?;
            let transition = record
                .revalidate(accepted, coordinator, &intent, revision, preimage)
                .map_err(|error| TestPortError(error.to_string()))?;
            return transition
                .apply_to(agreement, coordinator, revision)
                .map(Some)
                .map_err(|error| TestPortError(error.to_string()));
        }
        if let Some((record, protected, purpose)) = self
            .observed_revealing_claim_transitions
            .lock()
            .expect("observed revealing transitions lock")
            .get(key)
            .cloned()
        {
            let preimage = protected
                .decrypt(
                    &memory_claim_key(),
                    claim_material_context_for(accepted, purpose),
                )
                .map_err(|error| TestPortError(error.to_string()))?;
            let transition = record
                .revalidate(accepted, coordinator, revision, preimage)
                .map_err(|error| TestPortError(error.to_string()))?;
            return transition
                .apply_to(agreement, coordinator, revision)
                .map(Some)
                .map_err(|error| TestPortError(error.to_string()));
        }
        if let Some((record, intent)) = self
            .followup_claim_transitions
            .lock()
            .expect("follow-up transitions lock")
            .get(key)
            .cloned()
        {
            let transition = record
                .revalidate(accepted, coordinator, &intent, revision)
                .map_err(|error| TestPortError(error.to_string()))?;
            return transition
                .apply_to(agreement, coordinator, revision)
                .map(Some)
                .map_err(|error| TestPortError(error.to_string()));
        }
        self.observed_followup_claim_transitions
            .lock()
            .expect("observed follow-up transitions lock")
            .get(key)
            .cloned()
            .map_or(Ok(None), |record| {
                let transition = record
                    .revalidate(accepted, coordinator, revision)
                    .map_err(|error| TestPortError(error.to_string()))?;
                transition
                    .apply_to(agreement, coordinator, revision)
                    .map(Some)
                    .map_err(|error| TestPortError(error.to_string()))
            })
    }

    pub(crate) fn apply_refund_transition(
        &self,
        agreement: &lez_zec_swap_sdk::ZecAgreementV1,
        coordinator: &SwapCoordinator,
        revision: u64,
        key: &(String, u64),
    ) -> Result<Option<SwapCoordinator>, TestPortError> {
        self.refund_transitions
            .lock()
            .expect("refund transitions lock")
            .get(key)
            .cloned()
            .map_or(Ok(None), |transition| {
                transition
                    .apply_to(agreement, coordinator, revision)
                    .map(Some)
                    .map_err(|error| TestPortError(error.to_string()))
            })
    }
}

pub(crate) fn assert_store_has_no_activation_effects(store: &MemoryStore) {
    assert!(store.agreements.lock().expect("agreements lock").is_empty());
    assert!(
        store
            .claim_materials
            .lock()
            .expect("claim materials lock")
            .is_empty()
    );
    assert!(
        store
            .first_locks
            .lock()
            .expect("first-lock intents lock")
            .is_empty()
    );
    assert!(
        store
            .maker_locks
            .lock()
            .expect("maker-lock intents lock")
            .is_empty()
    );
}

pub(crate) fn memory_claim_key() -> ProtectedClaimKey {
    ProtectedClaimKey::new("sdk-lifecycle-memory-key-v1", [0x7a; 32])
        .expect("valid deterministic test key")
}

pub(crate) fn claim_material_context(
    accepted: &AcceptedZecAgreementV1,
) -> ClaimMaterialContext<'_> {
    claim_material_context_for(accepted, ClaimMaterialPurpose::LocalFirstClaim)
}

pub(crate) fn claim_material_context_for(
    accepted: &AcceptedZecAgreementV1,
    purpose: ClaimMaterialPurpose,
) -> ClaimMaterialContext<'_> {
    let agreement = accepted.agreement();
    ClaimMaterialContext::new(
        PROTECTED_CLAIM_SCHEMA_V1,
        agreement.coordinator().id(),
        Pair::Zcash,
        agreement.direction(),
        agreement.agreement_commitment(),
        accepted.local_participant(),
        purpose,
    )
}

pub(crate) fn claim_submission_context<'a>(
    agreement: &'a lez_zec_swap_sdk::ZecAgreementV1,
    local: Participant,
    step: ClaimStepV1,
    staged_revision: u64,
    expected_submission_id: &'a [u8; 32],
) -> ClaimSubmissionContext<'a> {
    ClaimSubmissionContext::new(
        ClaimMaterialContext::new(
            PROTECTED_CLAIM_SCHEMA_V1,
            agreement.coordinator().id(),
            Pair::Zcash,
            agreement.direction(),
            agreement.agreement_commitment(),
            local,
            match step {
                ClaimStepV1::RevealingLez => ClaimMaterialPurpose::LezClaimSubmission,
                ClaimStepV1::FollowupZcash => ClaimMaterialPurpose::ZcashClaimSubmission,
            },
        ),
        step,
        staged_revision,
        expected_submission_id,
    )
}

pub(crate) fn deterministic_claim_nonce(accepted: &AcceptedZecAgreementV1) -> [u8; 24] {
    let mut digest = Sha256::new();
    digest.update(b"lez-zec-sdk-test/claim-nonce/v1");
    digest.update(accepted.agreement().coordinator().id().as_str().as_bytes());
    digest.update(accepted.agreement().agreement_commitment());
    digest.update([match accepted.local_participant() {
        Participant::Maker => 0,
        Participant::Taker => 1,
    }]);
    let digest = digest.finalize();
    let mut nonce = [0_u8; 24];
    nonce.copy_from_slice(&digest[..24]);
    nonce
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

    async fn restage_first_lock_intent(
        &self,
        intent: &FirstLockIntentV1,
        supersession: &FirstLockSupersessionV1,
    ) -> Result<CreateFirstLockOutcome, Self::Error> {
        let key = intent.swap_id().as_str().to_owned();
        let mut records = self.first_locks.lock().expect("first-lock lock");
        let mut lineage = self.supersessions.lock().expect("supersession lock");
        let history = lineage.entry(key.clone()).or_default();
        let Some(open) = records.get(&key) else {
            return Ok(CreateFirstLockOutcome::Conflict);
        };
        if let Some(recorded) = history
            .iter()
            .find(|recorded| recorded.replan_index() == supersession.replan_index())
        {
            return Ok(if recorded == supersession && open == intent {
                CreateFirstLockOutcome::ExistingSame
            } else {
                CreateFirstLockOutcome::Conflict
            });
        }
        if !supersession.links(open.plan(), intent.plan())
            || supersession.replan_index() != u32::try_from(history.len()).unwrap() + 1
        {
            return Ok(CreateFirstLockOutcome::Conflict);
        }
        history.push(supersession.clone());
        records.insert(key, intent.clone());
        Ok(CreateFirstLockOutcome::Created)
    }

    async fn load_first_lock_supersessions(
        &self,
        swap_id: &SwapId,
    ) -> Result<Vec<FirstLockSupersessionV1>, Self::Error> {
        Ok(self
            .supersessions
            .lock()
            .expect("supersession lock")
            .get(swap_id.as_str())
            .cloned()
            .unwrap_or_default())
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

    async fn commit_observed_maker_lock_transition(
        &self,
        transition: &ObservedMakerLockTransitionV1,
    ) -> Result<FirstLockProjectionCommit, Self::Error> {
        let mode = *self.transition_mode.lock().expect("transition mode lock");
        if mode == TransitionCommitMode::FailBeforeCommit {
            return Err(TestPortError(
                "forced observed maker transition failure".to_owned(),
            ));
        }
        let key = (
            transition.swap_id().as_str().to_owned(),
            transition.predecessor_revision(),
        );
        let mut transitions = self
            .observed_maker_lock_transitions
            .lock()
            .expect("observed maker transition lock");
        let was_replay = match transitions.get(&key) {
            None => {
                transitions.insert(key, transition.clone());
                false
            }
            Some(existing) if existing == transition => true,
            Some(_) => {
                return Err(TestPortError(
                    "conflicting observed maker transition".to_owned(),
                ));
            }
        };
        if mode == TransitionCommitMode::CommitThenReportFailure {
            return Err(TestPortError(
                "unknown successful observed maker commit".to_owned(),
            ));
        }
        Ok(FirstLockProjectionCommit::new(
            transition.predecessor_revision() + 1,
            was_replay,
        ))
    }

    async fn load_observed_maker_lock_transition(
        &self,
        swap_id: &SwapId,
        predecessor_revision: u64,
    ) -> Result<Option<ObservedMakerLockTransitionV1>, Self::Error> {
        Ok(self
            .observed_maker_lock_transitions
            .lock()
            .expect("observed maker transition lock")
            .get(&(swap_id.as_str().to_owned(), predecessor_revision))
            .cloned())
    }

    async fn create_maker_lock_intent(
        &self,
        intent: &MakerLockIntentV1,
    ) -> Result<CreateFirstLockOutcome, Self::Error> {
        let key = intent.swap_id().as_str().to_owned();
        let mut records = self.maker_locks.lock().expect("maker-lock lock");
        match records.get(&key) {
            None => {
                records.insert(key, intent.clone());
                Ok(CreateFirstLockOutcome::Created)
            }
            Some(existing) if existing == intent => Ok(CreateFirstLockOutcome::ExistingSame),
            Some(_) => Ok(CreateFirstLockOutcome::Conflict),
        }
    }

    async fn load_maker_lock_intent(
        &self,
        swap_id: &SwapId,
    ) -> Result<Option<MakerLockIntentV1>, Self::Error> {
        Ok(self
            .maker_locks
            .lock()
            .expect("maker-lock lock")
            .get(swap_id.as_str())
            .cloned())
    }

    async fn restage_maker_lock_intent(
        &self,
        intent: &MakerLockIntentV1,
        supersession: &FirstLockSupersessionV1,
    ) -> Result<CreateFirstLockOutcome, Self::Error> {
        let key = intent.swap_id().as_str().to_owned();
        let mut records = self.maker_locks.lock().expect("maker-lock lock");
        let mut lineage = self.supersessions.lock().expect("supersession lock");
        let history = lineage.entry(key.clone()).or_default();
        let Some(open) = records.get(&key) else {
            return Ok(CreateFirstLockOutcome::Conflict);
        };
        if let Some(recorded) = history
            .iter()
            .find(|recorded| recorded.replan_index() == supersession.replan_index())
        {
            return Ok(if recorded == supersession && open == intent {
                CreateFirstLockOutcome::ExistingSame
            } else {
                CreateFirstLockOutcome::Conflict
            });
        }
        if !supersession.links(open.plan(), intent.plan())
            || supersession.replan_index() != u32::try_from(history.len()).unwrap() + 1
        {
            return Ok(CreateFirstLockOutcome::Conflict);
        }
        history.push(supersession.clone());
        records.insert(key, intent.clone());
        Ok(CreateFirstLockOutcome::Created)
    }

    async fn commit_maker_lock_transition(
        &self,
        transition: &MakerLockTransitionV1,
    ) -> Result<FirstLockProjectionCommit, Self::Error> {
        let mode = *self.transition_mode.lock().expect("transition mode lock");
        if mode == TransitionCommitMode::FailBeforeCommit {
            return Err(TestPortError("forced maker transition failure".to_owned()));
        }
        let key = (
            transition.swap_id().as_str().to_owned(),
            transition.predecessor_revision(),
        );
        let mut transitions = self
            .maker_lock_transitions
            .lock()
            .expect("maker-lock transition lock");
        let was_replay = match transitions.get(&key) {
            None => {
                transitions.insert(key, transition.clone());
                false
            }
            Some(existing) if existing == transition => true,
            Some(_) => return Err(TestPortError("conflicting maker transition".to_owned())),
        };
        self.maker_locks
            .lock()
            .expect("maker-lock lock")
            .remove(transition.swap_id().as_str());
        if mode == TransitionCommitMode::CommitThenReportFailure {
            return Err(TestPortError("unknown successful maker commit".to_owned()));
        }
        Ok(FirstLockProjectionCommit::new(
            transition.predecessor_revision() + 1,
            was_replay,
        ))
    }

    async fn load_maker_lock_transition(
        &self,
        swap_id: &SwapId,
        predecessor_revision: u64,
    ) -> Result<Option<MakerLockTransitionV1>, Self::Error> {
        Ok(self
            .maker_lock_transitions
            .lock()
            .expect("maker-lock transition lock")
            .get(&(swap_id.as_str().to_owned(), predecessor_revision))
            .cloned())
    }
}

#[async_trait]
impl ClaimRecoveryStore for MemoryStore {
    async fn create_agreement_with_local_claim_material(
        &self,
        envelope: &AcceptedZecAgreementEnvelopeV1,
        preimage: &ClaimPreimage,
    ) -> Result<CreateAgreementOutcome, Self::Error> {
        if self.fail_create {
            return Err(TestPortError("forced create failure".to_owned()));
        }
        let accepted = AcceptedZecAgreementV1::resume(envelope)
            .map_err(|error| TestPortError(error.to_string()))?;
        let swap_key = accepted.agreement().coordinator().id().as_str().to_owned();
        let key = memory_claim_key();

        let mut agreements = self.agreements.lock().expect("agreements lock");
        let mut materials = self.claim_materials.lock().expect("claim materials lock");
        let mut purposes = self
            .claim_material_purposes
            .lock()
            .expect("claim material purposes lock");
        match (agreements.get(&swap_key), materials.get(&swap_key)) {
            (Some(existing_envelope), Some(existing_material)) if existing_envelope == envelope => {
                let existing_preimage = existing_material
                    .decrypt(&key, claim_material_context(&accepted))
                    .map_err(|error| TestPortError(error.to_string()))?;
                if existing_preimage.expose_secret() == preimage.expose_secret() {
                    Ok(CreateAgreementOutcome::ExistingSame)
                } else {
                    Ok(CreateAgreementOutcome::Conflict)
                }
            }
            (Some(existing_envelope), None) if existing_envelope == envelope => {
                let protected = ProtectedClaimEnvelope::encrypt(
                    preimage,
                    &key,
                    deterministic_claim_nonce(&accepted),
                    claim_material_context(&accepted),
                )
                .map_err(|error| TestPortError(error.to_string()))?;
                materials.insert(swap_key, protected);
                purposes.insert(
                    accepted.agreement().coordinator().id().as_str().to_owned(),
                    ClaimMaterialPurpose::LocalFirstClaim,
                );
                Ok(CreateAgreementOutcome::Created)
            }
            (None, None) => {
                let protected = ProtectedClaimEnvelope::encrypt(
                    preimage,
                    &key,
                    deterministic_claim_nonce(&accepted),
                    claim_material_context(&accepted),
                )
                .map_err(|error| TestPortError(error.to_string()))?;
                agreements.insert(swap_key.clone(), envelope.clone());
                materials.insert(swap_key.clone(), protected);
                purposes.insert(swap_key, ClaimMaterialPurpose::LocalFirstClaim);
                Ok(CreateAgreementOutcome::Created)
            }
            _ => Ok(CreateAgreementOutcome::Conflict),
        }
    }

    async fn load_claim_material(
        &self,
        swap_id: &SwapId,
    ) -> Result<Option<ClaimPreimage>, Self::Error> {
        let agreements = self.agreements.lock().expect("agreements lock");
        let materials = self.claim_materials.lock().expect("claim materials lock");
        let purposes = self
            .claim_material_purposes
            .lock()
            .expect("claim material purposes lock");
        let envelope = agreements.get(swap_id.as_str());
        let protected = materials.get(swap_id.as_str());
        match (envelope, protected) {
            (None, None) => Ok(None),
            (Some(envelope), Some(protected)) => {
                let accepted = AcceptedZecAgreementV1::resume(envelope)
                    .map_err(|error| TestPortError(error.to_string()))?;
                let purpose = purposes
                    .get(swap_id.as_str())
                    .copied()
                    .unwrap_or(ClaimMaterialPurpose::LocalFirstClaim);
                protected
                    .decrypt(
                        &memory_claim_key(),
                        claim_material_context_for(&accepted, purpose),
                    )
                    .map(Some)
                    .map_err(|error| TestPortError(error.to_string()))
            }
            _ => Err(TestPortError(
                "agreement and protected claim material are not atomic".to_owned(),
            )),
        }
    }

    async fn protect_claim_submission(
        &self,
        agreement: &lez_zec_swap_sdk::ZecAgreementV1,
        local_participant: Participant,
        staged_revision: u64,
        prepared: &PreparedClaimSubmissionV1,
    ) -> Result<ProtectedClaimPayloadEnvelope, Self::Error> {
        ProtectedClaimPayloadEnvelope::encrypt(
            prepared.exact_submission(),
            &memory_claim_key(),
            self.next_claim_nonce(prepared.expected_submission_id()),
            claim_submission_context(
                agreement,
                local_participant,
                prepared.step(),
                staged_revision,
                prepared.expected_submission_id(),
            ),
        )
        .map_err(|error| TestPortError(error.to_string()))
    }

    async fn open_claim_submission(
        &self,
        agreement: &lez_zec_swap_sdk::ZecAgreementV1,
        intent: &ClaimIntentV1,
        protected: &ProtectedClaimPayloadEnvelope,
    ) -> Result<PreparedClaimSubmissionV1, Self::Error> {
        if protected.fingerprint() != intent.protected_payload_fingerprint() {
            return Err(TestPortError(
                "claim intent and protected payload disagree".to_owned(),
            ));
        }
        let exact = protected
            .decrypt(
                &memory_claim_key(),
                claim_submission_context(
                    agreement,
                    intent.local_participant(),
                    intent.step(),
                    intent.staged_revision(),
                    intent.expected_submission_id(),
                ),
            )
            .map_err(|error| TestPortError(error.to_string()))?;
        PreparedClaimSubmissionV1::new(
            intent.step(),
            *intent.expected_submission_id(),
            exact.to_vec(),
        )
        .map_err(|error| TestPortError(error.to_string()))
    }

    async fn create_claim_intent(
        &self,
        intent: &ClaimIntentV1,
        protected: &ProtectedClaimPayloadEnvelope,
    ) -> Result<CreateFirstLockOutcome, Self::Error> {
        if protected.fingerprint() != intent.protected_payload_fingerprint() {
            return Err(TestPortError(
                "claim intent and protected payload disagree".to_owned(),
            ));
        }
        let key = intent.swap_id().as_str().to_owned();
        let mut intents = self.claim_intents.lock().expect("claim intents lock");
        match intents.get(&key) {
            None => {
                intents.insert(key, (intent.clone(), protected.clone()));
                Ok(CreateFirstLockOutcome::Created)
            }
            Some((existing_intent, existing_protected))
                if existing_intent == intent && existing_protected == protected =>
            {
                Ok(CreateFirstLockOutcome::ExistingSame)
            }
            Some(_) => Ok(CreateFirstLockOutcome::Conflict),
        }
    }

    async fn load_claim_intent(
        &self,
        swap_id: &SwapId,
    ) -> Result<Option<(ClaimIntentV1, ProtectedClaimPayloadEnvelope)>, Self::Error> {
        Ok(self
            .claim_intents
            .lock()
            .expect("claim intents lock")
            .get(swap_id.as_str())
            .cloned())
    }

    async fn commit_revealing_claim_transition(
        &self,
        transition: &RevealingClaimTransitionV1,
    ) -> Result<FirstLockProjectionCommit, Self::Error> {
        let swap_id = transition.swap_id();
        let accepted = self.accepted(swap_id)?;
        let predecessor = transition.predecessor_revision();
        let coordinator = self.coordinator_at(&accepted, predecessor)?;
        transition
            .apply_to(accepted.agreement(), &coordinator, predecessor)
            .map_err(|error| TestPortError(error.to_string()))?;
        let key = (swap_id.as_str().to_owned(), predecessor);
        let record = RevealingClaimTransitionRecordV1::from(transition);
        if let Some((existing, _, _, _)) = self
            .revealing_claim_transitions
            .lock()
            .expect("revealing transitions lock")
            .get(&key)
        {
            if existing == &record {
                return Ok(FirstLockProjectionCommit::new(predecessor + 1, true));
            }
            return Err(TestPortError(
                "conflicting revealing claim transition".to_owned(),
            ));
        }
        let (intent, _) = self
            .claim_intents
            .lock()
            .expect("claim intents lock")
            .get(swap_id.as_str())
            .cloned()
            .ok_or_else(|| TestPortError("missing revealing claim intent".to_owned()))?;
        let intent_record = ClaimIntentRecordV1::from(&intent);
        let protected_preimage = self
            .claim_materials
            .lock()
            .expect("claim materials lock")
            .get(swap_id.as_str())
            .cloned()
            .ok_or_else(|| TestPortError("owner claim material is missing".to_owned()))?;
        let purpose = self
            .claim_material_purposes
            .lock()
            .expect("claim material purposes lock")
            .get(swap_id.as_str())
            .copied()
            .unwrap_or(ClaimMaterialPurpose::LocalFirstClaim);
        self.revealing_claim_transitions
            .lock()
            .expect("revealing transitions lock")
            .insert(key, (record, intent_record, protected_preimage, purpose));
        self.claim_intents
            .lock()
            .expect("claim intents lock")
            .remove(swap_id.as_str());
        Ok(FirstLockProjectionCommit::new(predecessor + 1, false))
    }

    async fn load_revealing_claim_transition(
        &self,
        swap_id: &SwapId,
        predecessor_revision: u64,
    ) -> Result<Option<RevealingClaimTransitionV1>, Self::Error> {
        let Some((record, intent, protected, purpose)) = self
            .revealing_claim_transitions
            .lock()
            .expect("revealing transitions lock")
            .get(&(swap_id.as_str().to_owned(), predecessor_revision))
            .cloned()
        else {
            return Ok(None);
        };
        let accepted = self.accepted(swap_id)?;
        let coordinator = self.coordinator_at(&accepted, predecessor_revision)?;
        let preimage = protected
            .decrypt(
                &memory_claim_key(),
                claim_material_context_for(&accepted, purpose),
            )
            .map_err(|error| TestPortError(error.to_string()))?;
        record
            .revalidate(
                &accepted,
                &coordinator,
                &intent,
                predecessor_revision,
                preimage,
            )
            .map(Some)
            .map_err(|error| TestPortError(error.to_string()))
    }

    async fn commit_observed_revealing_claim_transition(
        &self,
        transition: &ObservedRevealingClaimTransitionV1,
    ) -> Result<FirstLockProjectionCommit, Self::Error> {
        let swap_id = transition.swap_id();
        let accepted = self.accepted(swap_id)?;
        let predecessor = transition.predecessor_revision();
        let coordinator = self.coordinator_at(&accepted, predecessor)?;
        transition
            .apply_to(accepted.agreement(), &coordinator, predecessor)
            .map_err(|error| TestPortError(error.to_string()))?;
        let key = (swap_id.as_str().to_owned(), predecessor);
        let record = ObservedRevealingClaimTransitionRecordV1::from(transition);
        if let Some((existing, _, _)) = self
            .observed_revealing_claim_transitions
            .lock()
            .expect("observed revealing transitions lock")
            .get(&key)
        {
            if existing == &record {
                return Ok(FirstLockProjectionCommit::new(predecessor + 1, true));
            }
            return Err(TestPortError(
                "conflicting observed revealing claim transition".to_owned(),
            ));
        }
        if self
            .claim_materials
            .lock()
            .expect("claim materials lock")
            .contains_key(swap_id.as_str())
        {
            return Err(TestPortError(
                "observer unexpectedly already owns claim material".to_owned(),
            ));
        }
        let purpose = ClaimMaterialPurpose::ObservedFollowUpClaim;
        let protected = ProtectedClaimEnvelope::encrypt(
            transition.evidence().preimage(),
            &memory_claim_key(),
            self.next_claim_nonce(swap_id.as_str().as_bytes()),
            claim_material_context_for(&accepted, purpose),
        )
        .map_err(|error| TestPortError(error.to_string()))?;
        self.claim_materials
            .lock()
            .expect("claim materials lock")
            .insert(swap_id.as_str().to_owned(), protected.clone());
        self.claim_material_purposes
            .lock()
            .expect("claim material purposes lock")
            .insert(swap_id.as_str().to_owned(), purpose);
        self.observed_revealing_claim_transitions
            .lock()
            .expect("observed revealing transitions lock")
            .insert(key, (record, protected, purpose));
        Ok(FirstLockProjectionCommit::new(predecessor + 1, false))
    }

    async fn load_observed_revealing_claim_transition(
        &self,
        swap_id: &SwapId,
        predecessor_revision: u64,
    ) -> Result<Option<ObservedRevealingClaimTransitionV1>, Self::Error> {
        let Some((record, protected, purpose)) = self
            .observed_revealing_claim_transitions
            .lock()
            .expect("observed revealing transitions lock")
            .get(&(swap_id.as_str().to_owned(), predecessor_revision))
            .cloned()
        else {
            return Ok(None);
        };
        let accepted = self.accepted(swap_id)?;
        let coordinator = self.coordinator_at(&accepted, predecessor_revision)?;
        let preimage = protected
            .decrypt(
                &memory_claim_key(),
                claim_material_context_for(&accepted, purpose),
            )
            .map_err(|error| TestPortError(error.to_string()))?;
        record
            .revalidate(&accepted, &coordinator, predecessor_revision, preimage)
            .map(Some)
            .map_err(|error| TestPortError(error.to_string()))
    }

    async fn commit_followup_claim_transition(
        &self,
        transition: &FollowupClaimTransitionV1,
    ) -> Result<FirstLockProjectionCommit, Self::Error> {
        let swap_id = transition.swap_id();
        let accepted = self.accepted(swap_id)?;
        let predecessor = transition.predecessor_revision();
        let coordinator = self.coordinator_at(&accepted, predecessor)?;
        transition
            .apply_to(accepted.agreement(), &coordinator, predecessor)
            .map_err(|error| TestPortError(error.to_string()))?;
        let key = (swap_id.as_str().to_owned(), predecessor);
        let record = FollowupClaimTransitionRecordV1::from(transition);
        if let Some((existing, _)) = self
            .followup_claim_transitions
            .lock()
            .expect("follow-up transitions lock")
            .get(&key)
        {
            if existing == &record {
                return Ok(FirstLockProjectionCommit::new(predecessor + 1, true));
            }
            return Err(TestPortError(
                "conflicting follow-up claim transition".to_owned(),
            ));
        }
        let (intent, _) = self
            .claim_intents
            .lock()
            .expect("claim intents lock")
            .get(swap_id.as_str())
            .cloned()
            .ok_or_else(|| TestPortError("missing follow-up claim intent".to_owned()))?;
        self.followup_claim_transitions
            .lock()
            .expect("follow-up transitions lock")
            .insert(key, (record, ClaimIntentRecordV1::from(&intent)));
        self.claim_intents
            .lock()
            .expect("claim intents lock")
            .remove(swap_id.as_str());
        Ok(FirstLockProjectionCommit::new(predecessor + 1, false))
    }

    async fn load_followup_claim_transition(
        &self,
        swap_id: &SwapId,
        predecessor_revision: u64,
    ) -> Result<Option<FollowupClaimTransitionV1>, Self::Error> {
        let Some((record, intent)) = self
            .followup_claim_transitions
            .lock()
            .expect("follow-up transitions lock")
            .get(&(swap_id.as_str().to_owned(), predecessor_revision))
            .cloned()
        else {
            return Ok(None);
        };
        let accepted = self.accepted(swap_id)?;
        let coordinator = self.coordinator_at(&accepted, predecessor_revision)?;
        record
            .revalidate(&accepted, &coordinator, &intent, predecessor_revision)
            .map(Some)
            .map_err(|error| TestPortError(error.to_string()))
    }

    async fn commit_observed_followup_claim_transition(
        &self,
        transition: &ObservedFollowupClaimTransitionV1,
    ) -> Result<FirstLockProjectionCommit, Self::Error> {
        let swap_id = transition.swap_id();
        let accepted = self.accepted(swap_id)?;
        let predecessor = transition.predecessor_revision();
        let coordinator = self.coordinator_at(&accepted, predecessor)?;
        transition
            .apply_to(accepted.agreement(), &coordinator, predecessor)
            .map_err(|error| TestPortError(error.to_string()))?;
        let key = (swap_id.as_str().to_owned(), predecessor);
        let record = ObservedFollowupClaimTransitionRecordV1::from(transition);
        let mut transitions = self
            .observed_followup_claim_transitions
            .lock()
            .expect("observed follow-up transitions lock");
        match transitions.get(&key) {
            Some(existing) if existing == &record => {
                Ok(FirstLockProjectionCommit::new(predecessor + 1, true))
            }
            Some(_) => Err(TestPortError(
                "conflicting observed follow-up claim transition".to_owned(),
            )),
            None => {
                transitions.insert(key, record);
                Ok(FirstLockProjectionCommit::new(predecessor + 1, false))
            }
        }
    }

    async fn load_observed_followup_claim_transition(
        &self,
        swap_id: &SwapId,
        predecessor_revision: u64,
    ) -> Result<Option<ObservedFollowupClaimTransitionV1>, Self::Error> {
        let Some(record) = self
            .observed_followup_claim_transitions
            .lock()
            .expect("observed follow-up transitions lock")
            .get(&(swap_id.as_str().to_owned(), predecessor_revision))
            .cloned()
        else {
            return Ok(None);
        };
        let accepted = self.accepted(swap_id)?;
        let coordinator = self.coordinator_at(&accepted, predecessor_revision)?;
        record
            .revalidate(&accepted, &coordinator, predecessor_revision)
            .map(Some)
            .map_err(|error| TestPortError(error.to_string()))
    }
}

#[async_trait]
impl RefundRecoveryStore for MemoryStore {
    async fn create_refund_intent(
        &self,
        intent: &RefundIntentV1,
    ) -> Result<CreateFirstLockOutcome, Self::Error> {
        let key = intent.swap_id().as_str().to_owned();
        let mut intents = self.refund_intents.lock().expect("refund intents lock");
        match intents.get(&key) {
            None => {
                intents.insert(key, intent.clone());
                Ok(CreateFirstLockOutcome::Created)
            }
            Some(existing) if existing == intent => Ok(CreateFirstLockOutcome::ExistingSame),
            Some(_) => Ok(CreateFirstLockOutcome::Conflict),
        }
    }

    async fn load_refund_intent(
        &self,
        swap_id: &SwapId,
    ) -> Result<Option<RefundIntentV1>, Self::Error> {
        Ok(self
            .refund_intents
            .lock()
            .expect("refund intents lock")
            .get(swap_id.as_str())
            .cloned())
    }

    async fn commit_refund_transition(
        &self,
        transition: &RefundTransitionV1,
    ) -> Result<FirstLockProjectionCommit, Self::Error> {
        let mode = *self.transition_mode.lock().expect("transition mode lock");
        if mode == TransitionCommitMode::FailBeforeCommit {
            return Err(TestPortError("forced refund commit failure".to_owned()));
        }
        let predecessor = transition.predecessor_revision();
        let accepted = self.accepted(transition.swap_id())?;
        let coordinator = self.coordinator_at(&accepted, predecessor)?;
        transition
            .apply_to(accepted.agreement(), &coordinator, predecessor)
            .map_err(|error| TestPortError(error.to_string()))?;
        let key = (
            transition.swap_id().as_str().to_owned(),
            transition.predecessor_revision(),
        );
        let mut transitions = self
            .refund_transitions
            .lock()
            .expect("refund transitions lock");
        let was_replay = match transitions.get(&key) {
            None => {
                if transition.is_owned()
                    && !self
                        .refund_intents
                        .lock()
                        .expect("refund intents lock")
                        .contains_key(transition.swap_id().as_str())
                {
                    return Err(TestPortError("missing owned refund intent".to_owned()));
                }
                transitions.insert(key, transition.clone());
                false
            }
            Some(existing) if existing == transition => true,
            Some(_) => return Err(TestPortError("conflicting refund transition".to_owned())),
        };
        drop(transitions);
        if transition.is_owned() {
            self.refund_intents
                .lock()
                .expect("refund intents lock")
                .remove(transition.swap_id().as_str());
        }
        if mode == TransitionCommitMode::CommitThenReportFailure {
            return Err(TestPortError("unknown successful refund commit".to_owned()));
        }
        Ok(FirstLockProjectionCommit::new(predecessor + 1, was_replay))
    }

    async fn load_refund_transition(
        &self,
        swap_id: &SwapId,
        predecessor_revision: u64,
    ) -> Result<Option<RefundTransitionV1>, Self::Error> {
        Ok(self
            .refund_transitions
            .lock()
            .expect("refund transitions lock")
            .get(&(swap_id.as_str().to_owned(), predecessor_revision))
            .cloned())
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct NoopLez;

#[derive(Clone, Copy, Debug)]
pub(crate) struct NoopZcash;

#[derive(Clone, Copy, Debug)]
pub(crate) struct NoPrelockCapability;

#[derive(Clone, Debug)]
pub(crate) struct MemoryTakerLockObservation {
    pub(crate) response: Arc<Mutex<Result<TakerFirstLockObservationV1, TestPortError>>>,
    pub(crate) calls: Arc<Mutex<usize>>,
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
    pub(crate) fn respond(&self, response: Result<TakerFirstLockObservationV1, TestPortError>) {
        *self.response.lock().expect("observation response lock") = response;
    }

    pub(crate) fn calls(&self) -> usize {
        *self.calls.lock().expect("observation calls lock")
    }
}

#[derive(Clone, Debug)]
pub(crate) struct MemoryMakerLockObservation {
    pub(crate) response: Arc<Mutex<Result<MakerLockObservationV1, TestPortError>>>,
    pub(crate) calls: Arc<Mutex<usize>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ClaimPortEvent {
    PrepareReveal,
    ObserveReveal,
    ObserveFunding,
    SubmitReveal,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct MemoryClaimCorridor {
    pub(crate) submissions: Arc<Mutex<Vec<ClaimStepV1>>>,
    pub(crate) revealing_submission_bytes: Arc<Mutex<Vec<Vec<u8>>>>,
    pub(crate) revealing_preimage: Arc<Mutex<Option<[u8; 32]>>>,
    pub(crate) followup_confirmed: Arc<Mutex<bool>>,
    pub(crate) zcash_contexts: Arc<Mutex<Vec<ZcashClaimContextV1>>>,
    pub(crate) funding_contexts: Arc<Mutex<Vec<ZcashFundingContextV1>>>,
    pub(crate) funding_observation: Arc<Mutex<Option<ZcashFundingObservationV1>>>,
    pub(crate) port_events: Arc<Mutex<Vec<ClaimPortEvent>>>,
}

impl MemoryClaimCorridor {
    pub(crate) fn expose_funding_as(&self, observation: ZcashFundingObservationV1) {
        *self
            .funding_observation
            .lock()
            .expect("funding observation lock") = Some(observation);
    }

    pub(crate) fn configured_funding_observation(
        &self,
        agreement: &lez_zec_swap_sdk::ZecAgreementV1,
    ) -> ZcashFundingObservationV1 {
        self.funding_observation
            .lock()
            .expect("funding observation lock")
            .clone()
            .unwrap_or_else(|| exact_zcash_funding_observation(agreement))
    }

    pub(crate) fn port_events(&self) -> Vec<ClaimPortEvent> {
        self.port_events
            .lock()
            .expect("claim port events lock")
            .clone()
    }

    pub(crate) fn record_event(&self, event: ClaimPortEvent) {
        self.port_events
            .lock()
            .expect("claim port events lock")
            .push(event);
    }

    pub(crate) fn revealing_submission_bytes(&self) -> Vec<Vec<u8>> {
        self.revealing_submission_bytes
            .lock()
            .expect("revealing submission bytes lock")
            .clone()
    }

    pub(crate) fn funding_contexts(&self) -> Vec<ZcashFundingContextV1> {
        self.funding_contexts
            .lock()
            .expect("funding contexts lock")
            .clone()
    }

    pub(crate) fn record_funding_context(&self, context: &ZcashFundingContextV1) {
        self.funding_contexts
            .lock()
            .expect("funding contexts lock")
            .push(context.clone());
    }

    pub(crate) fn confirm_revealing_claim(&self, preimage: [u8; 32]) {
        *self
            .revealing_preimage
            .lock()
            .expect("revealing claim lock") = Some(preimage);
    }

    pub(crate) fn confirm_followup_claim(&self) {
        *self
            .followup_confirmed
            .lock()
            .expect("follow-up claim lock") = true;
    }

    pub(crate) fn submissions(&self) -> Vec<ClaimStepV1> {
        self.submissions
            .lock()
            .expect("claim submissions lock")
            .clone()
    }

    pub(crate) fn zcash_contexts(&self) -> Vec<ZcashClaimContextV1> {
        self.zcash_contexts
            .lock()
            .expect("Zcash claim contexts lock")
            .clone()
    }

    pub(crate) fn record_zcash_context(&self, context: &ZcashClaimContextV1) {
        self.zcash_contexts
            .lock()
            .expect("Zcash claim contexts lock")
            .push(context.clone());
    }

    pub(crate) fn record_submission(&self, step: ClaimStepV1) {
        self.submissions
            .lock()
            .expect("claim submissions lock")
            .push(step);
    }

    pub(crate) fn record_revealing_submission(&self, exact: &[u8]) {
        self.revealing_submission_bytes
            .lock()
            .expect("revealing submission bytes lock")
            .push(exact.to_vec());
    }
}

impl Default for MemoryMakerLockObservation {
    fn default() -> Self {
        Self {
            response: Arc::new(Mutex::new(Ok(MakerLockObservationV1::Absent))),
            calls: Arc::new(Mutex::new(0)),
        }
    }
}

impl MemoryMakerLockObservation {
    pub(crate) fn respond(&self, response: Result<MakerLockObservationV1, TestPortError>) {
        *self
            .response
            .lock()
            .expect("maker observation response lock") = response;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RefundPortEvent {
    Eligibility(RefundStepV1),
    Prepare(RefundStepV1),
    ObservePrepared(RefundStepV1),
    ObserveCounterparty(RefundStepV1),
    Submit(RefundStepV1),
}

pub(crate) type RefundSubmissions = Vec<(RefundStepV1, Vec<u8>)>;

#[derive(Clone, Debug, Default)]
pub(crate) struct MemoryRefundCorridor {
    pub(crate) eligibility: Arc<Mutex<HashMap<RefundStepV1, RefundEligibilityObservationV1>>>,
    pub(crate) confirmed: Arc<Mutex<Vec<RefundStepV1>>>,
    pub(crate) outcomes: Arc<Mutex<HashMap<RefundStepV1, RefundSubmitOutcomeV1>>>,
    pub(crate) submissions: Arc<Mutex<RefundSubmissions>>,
    pub(crate) events: Arc<Mutex<Vec<RefundPortEvent>>>,
}

impl MemoryRefundCorridor {
    pub(crate) fn set_eligibility(
        &self,
        step: RefundStepV1,
        observation: RefundEligibilityObservationV1,
    ) {
        self.eligibility
            .lock()
            .expect("refund eligibility lock")
            .insert(step, observation);
    }

    pub(crate) fn eligibility(&self, step: RefundStepV1) -> RefundEligibilityObservationV1 {
        self.record_event(RefundPortEvent::Eligibility(step));
        self.eligibility
            .lock()
            .expect("refund eligibility lock")
            .get(&step)
            .copied()
            .unwrap_or(RefundEligibilityObservationV1::Unstable)
    }

    pub(crate) fn confirm(&self, step: RefundStepV1) {
        let mut confirmed = self.confirmed.lock().expect("refund confirmations lock");
        if !confirmed.contains(&step) {
            confirmed.push(step);
        }
    }

    pub(crate) fn observe(
        &self,
        agreement: &lez_zec_swap_sdk::ZecAgreementV1,
        step: RefundStepV1,
        prepared: Option<&PreparedRefundSubmissionV1>,
    ) -> Result<RefundObservationV1, TestPortError> {
        if !self
            .confirmed
            .lock()
            .expect("refund confirmations lock")
            .contains(&step)
        {
            return Ok(RefundObservationV1::Absent);
        }
        let RefundEligibilityObservationV1::Canonical(position) = self
            .eligibility
            .lock()
            .expect("refund eligibility lock")
            .get(&step)
            .copied()
            .unwrap_or(RefundEligibilityObservationV1::Unstable)
        else {
            return Ok(RefundObservationV1::Unstable);
        };
        let expected = prepared.map_or_else(
            || refund_submission_id(step),
            |submission| *submission.expected_submission_id(),
        );
        RefundEvidenceV1::new(
            agreement,
            step,
            expected,
            match step {
                RefundStepV1::Lez => "deterministic-lez-refund",
                RefundStepV1::Zcash => "deterministic-zcash-refund",
            },
            position,
            3,
        )
        .map(RefundObservationV1::Confirmed)
        .map_err(|error| TestPortError(error.to_string()))
    }

    pub(crate) fn set_submit_outcome(&self, step: RefundStepV1, outcome: RefundSubmitOutcomeV1) {
        self.outcomes
            .lock()
            .expect("refund outcomes lock")
            .insert(step, outcome);
    }

    pub(crate) fn submit(
        &self,
        step: RefundStepV1,
        prepared: &PreparedRefundSubmissionV1,
    ) -> Result<RefundSubmitOutcomeV1, TestPortError> {
        if prepared.step() != step
            || prepared.expected_submission_id() != &refund_submission_id(step)
            || prepared.exact_submission() != refund_submission_bytes(step)
        {
            return Err(TestPortError(
                "unexpected deterministic refund submission".to_owned(),
            ));
        }
        self.record_event(RefundPortEvent::Submit(step));
        self.submissions
            .lock()
            .expect("refund submissions lock")
            .push((step, prepared.exact_submission().to_vec()));
        Ok(self
            .outcomes
            .lock()
            .expect("refund outcomes lock")
            .get(&step)
            .copied()
            .unwrap_or(RefundSubmitOutcomeV1::Accepted))
    }

    pub(crate) fn record_event(&self, event: RefundPortEvent) {
        self.events.lock().expect("refund events lock").push(event);
    }

    pub(crate) fn events(&self) -> Vec<RefundPortEvent> {
        self.events.lock().expect("refund events lock").clone()
    }

    pub(crate) fn submissions(&self) -> Vec<(RefundStepV1, Vec<u8>)> {
        self.submissions
            .lock()
            .expect("refund submissions lock")
            .clone()
    }
}

pub(crate) const fn refund_submission_id(step: RefundStepV1) -> [u8; 32] {
    match step {
        RefundStepV1::Lez => [0xd1; 32],
        RefundStepV1::Zcash => [0xd2; 32],
    }
}

pub(crate) const fn refund_submission_bytes(step: RefundStepV1) -> &'static [u8] {
    match step {
        RefundStepV1::Lez => b"deterministic-lez-refund-v1",
        RefundStepV1::Zcash => b"deterministic-zcash-refund-v1",
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct MemoryLezTakerLockObservation(
    pub(crate) MemoryTakerLockObservation,
    pub(crate) Arc<Mutex<Vec<Option<[u8; 32]>>>>,
    pub(crate) MemoryFirstLockPort,
    pub(crate) MemoryMakerLockObservation,
    pub(crate) MemoryClaimCorridor,
    pub(crate) MemoryRefundCorridor,
);

#[async_trait]
impl LezTakerFirstLockObservationPort for MemoryLezTakerLockObservation {
    type Error = TestPortError;

    async fn observe_taker_first_lock(
        &self,
        _agreement: &lez_zec_swap_sdk::ZecAgreementV1,
        previous: Option<&CanonicalLezEscrowObservationV1>,
    ) -> Result<TakerFirstLockObservationV1, Self::Error> {
        *self.0.calls.lock().expect("observation calls lock") += 1;
        self.1
            .lock()
            .expect("LEZ previous-head lock")
            .push(previous.map(|canonical| *canonical.transaction_id()));
        self.0
            .response
            .lock()
            .expect("observation response lock")
            .clone()
    }
}

#[async_trait]
impl LezFirstLockPort for MemoryLezTakerLockObservation {
    type Error = TestPortError;

    async fn observe_first_lock(
        &self,
        _agreement: &lez_zec_swap_sdk::ZecAgreementV1,
        submission: &PreparedFirstLockSubmissionV1,
    ) -> Result<FirstLockObservation, Self::Error> {
        Ok(self
            .2
            .observations
            .lock()
            .expect("observations lock")
            .get(&submission.step())
            .cloned()
            .unwrap_or(FirstLockObservation::Absent))
    }

    async fn submit_first_lock(
        &self,
        _agreement: &lez_zec_swap_sdk::ZecAgreementV1,
        submission: &PreparedFirstLockSubmissionV1,
    ) -> Result<FirstLockSubmitOutcome, Self::Error> {
        self.2.submit(submission)
    }
}

#[async_trait]
impl LezMakerLockObservationPort for MemoryLezTakerLockObservation {
    type Error = TestPortError;

    async fn observe_maker_lock(
        &self,
        _agreement: &lez_zec_swap_sdk::ZecAgreementV1,
    ) -> Result<MakerLockObservationV1, Self::Error> {
        *self.3.calls.lock().expect("maker observation calls lock") += 1;
        self.3
            .response
            .lock()
            .expect("maker observation response lock")
            .clone()
    }
}

#[async_trait]
impl LezClaimPort for MemoryLezTakerLockObservation {
    type Error = TestPortError;

    async fn prepare_revealing_claim(
        &self,
        _agreement: &lez_zec_swap_sdk::ZecAgreementV1,
        preimage: &ClaimPreimage,
    ) -> Result<PreparedClaimSubmissionV1, Self::Error> {
        self.4.record_event(ClaimPortEvent::PrepareReveal);
        let mut exact = vec![0x6c, 0x65, 0x7a];
        exact.extend_from_slice(preimage.expose_secret());
        PreparedClaimSubmissionV1::new(ClaimStepV1::RevealingLez, [0xc1; 32], exact)
            .map_err(|error| TestPortError(error.to_string()))
    }

    async fn observe_prepared_revealing_claim(
        &self,
        agreement: &lez_zec_swap_sdk::ZecAgreementV1,
        prepared: &PreparedClaimSubmissionV1,
    ) -> Result<RevealingClaimObservationV1, Self::Error> {
        self.4.record_event(ClaimPortEvent::ObserveReveal);
        if prepared.step() != ClaimStepV1::RevealingLez
            || prepared.expected_submission_id() != &[0xc1; 32]
        {
            return Err(TestPortError(
                "unexpected prepared LEZ claim identity".to_owned(),
            ));
        }
        let preimage = *self
            .4
            .revealing_preimage
            .lock()
            .expect("revealing claim lock");
        preimage.map_or(Ok(RevealingClaimObservationV1::Absent), |preimage| {
            RevealingClaimEvidenceV1::from_prepared_lez_claim_snapshot(
                agreement,
                prepared,
                canonical_lez_claim_snapshot(agreement, preimage),
            )
            .map(RevealingClaimObservationV1::Confirmed)
            .map_err(|error| TestPortError(error.to_string()))
        })
    }

    async fn observe_counterparty_revealing_claim(
        &self,
        agreement: &lez_zec_swap_sdk::ZecAgreementV1,
    ) -> Result<RevealingClaimObservationV1, Self::Error> {
        let preimage = *self
            .4
            .revealing_preimage
            .lock()
            .expect("revealing claim lock");
        preimage.map_or(Ok(RevealingClaimObservationV1::Absent), |preimage| {
            RevealingClaimEvidenceV1::from_lez_claim_snapshot(
                agreement,
                canonical_lez_claim_snapshot(agreement, preimage),
            )
            .map(RevealingClaimObservationV1::Confirmed)
            .map_err(|error| TestPortError(error.to_string()))
        })
    }

    async fn submit_revealing_claim(
        &self,
        _agreement: &lez_zec_swap_sdk::ZecAgreementV1,
        prepared: &PreparedClaimSubmissionV1,
    ) -> Result<(), Self::Error> {
        if prepared.step() != ClaimStepV1::RevealingLez
            || prepared.expected_submission_id() != &[0xc1; 32]
            || !prepared.exact_submission().starts_with(&[0x6c, 0x65, 0x7a])
        {
            return Err(TestPortError(
                "unexpected deterministic LEZ claim bytes".to_owned(),
            ));
        }
        self.4.record_event(ClaimPortEvent::SubmitReveal);
        self.4
            .record_revealing_submission(prepared.exact_submission());
        self.4.record_submission(ClaimStepV1::RevealingLez);
        Ok(())
    }
}

#[async_trait]
impl LezRefundPort for MemoryLezTakerLockObservation {
    type Error = TestPortError;

    async fn observe_refund_eligibility(
        &self,
        _agreement: &lez_zec_swap_sdk::ZecAgreementV1,
    ) -> Result<RefundEligibilityObservationV1, Self::Error> {
        Ok(self.5.eligibility(RefundStepV1::Lez))
    }

    async fn prepare_refund(
        &self,
        _agreement: &lez_zec_swap_sdk::ZecAgreementV1,
    ) -> Result<PreparedRefundSubmissionV1, Self::Error> {
        self.5
            .record_event(RefundPortEvent::Prepare(RefundStepV1::Lez));
        PreparedRefundSubmissionV1::new(
            RefundStepV1::Lez,
            refund_submission_id(RefundStepV1::Lez),
            refund_submission_bytes(RefundStepV1::Lez).to_vec(),
        )
        .map_err(|error| TestPortError(error.to_string()))
    }

    async fn observe_prepared_refund(
        &self,
        agreement: &lez_zec_swap_sdk::ZecAgreementV1,
        prepared: &PreparedRefundSubmissionV1,
    ) -> Result<RefundObservationV1, Self::Error> {
        self.5
            .record_event(RefundPortEvent::ObservePrepared(RefundStepV1::Lez));
        self.5.observe(agreement, RefundStepV1::Lez, Some(prepared))
    }

    async fn observe_counterparty_refund(
        &self,
        agreement: &lez_zec_swap_sdk::ZecAgreementV1,
    ) -> Result<RefundObservationV1, Self::Error> {
        self.5
            .record_event(RefundPortEvent::ObserveCounterparty(RefundStepV1::Lez));
        self.5.observe(agreement, RefundStepV1::Lez, None)
    }

    async fn submit_refund(
        &self,
        _agreement: &lez_zec_swap_sdk::ZecAgreementV1,
        prepared: &PreparedRefundSubmissionV1,
    ) -> Result<RefundSubmitOutcomeV1, Self::Error> {
        self.5.submit(RefundStepV1::Lez, prepared)
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct MemoryZcashTakerLockObservation(
    pub(crate) MemoryTakerLockObservation,
    pub(crate) MemoryFirstLockPort,
    pub(crate) MemoryMakerLockObservation,
    pub(crate) MemoryClaimCorridor,
    pub(crate) Arc<Mutex<Vec<Option<CanonicalZcashOutputObservation>>>>,
    pub(crate) MemoryRefundCorridor,
);

#[async_trait]
impl ZcashTakerFirstLockObservationPort for MemoryZcashTakerLockObservation {
    type Error = TestPortError;

    async fn observe_taker_first_lock(
        &self,
        _agreement: &lez_zec_swap_sdk::ZecAgreementV1,
        previous: Option<&CanonicalZcashOutputObservation>,
    ) -> Result<TakerFirstLockObservationV1, Self::Error> {
        *self.0.calls.lock().expect("observation calls lock") += 1;
        self.4
            .lock()
            .expect("Zcash previous-head lock")
            .push(previous.cloned());
        self.0
            .response
            .lock()
            .expect("observation response lock")
            .clone()
    }
}

pub(crate) fn take_zcash_previous_heads(
    port: &MemoryZcashTakerLockObservation,
) -> Vec<Option<CanonicalZcashOutputObservation>> {
    std::mem::take(&mut *port.4.lock().expect("Zcash previous-head history"))
}

#[async_trait]
impl ZcashFirstLockPort for MemoryZcashTakerLockObservation {
    type Error = TestPortError;

    async fn observe_first_lock(
        &self,
        _agreement: &lez_zec_swap_sdk::ZecAgreementV1,
        submission: &PreparedFirstLockSubmissionV1,
    ) -> Result<FirstLockObservation, Self::Error> {
        Ok(self
            .1
            .observations
            .lock()
            .expect("observations lock")
            .get(&submission.step())
            .cloned()
            .unwrap_or(FirstLockObservation::Absent))
    }

    async fn submit_first_lock(
        &self,
        _agreement: &lez_zec_swap_sdk::ZecAgreementV1,
        submission: &PreparedFirstLockSubmissionV1,
    ) -> Result<(), Self::Error> {
        self.1.submit(submission).map(|_| ())
    }
}

#[async_trait]
impl ZcashMakerLockObservationPort for MemoryZcashTakerLockObservation {
    type Error = TestPortError;

    async fn observe_maker_lock(
        &self,
        _agreement: &lez_zec_swap_sdk::ZecAgreementV1,
    ) -> Result<MakerLockObservationV1, Self::Error> {
        *self.2.calls.lock().expect("maker observation calls lock") += 1;
        self.2
            .response
            .lock()
            .expect("maker observation response lock")
            .clone()
    }
}

#[async_trait]
impl ZcashClaimPort for MemoryZcashTakerLockObservation {
    type Error = TestPortError;

    async fn observe_funding_before_reveal(
        &self,
        agreement: &lez_zec_swap_sdk::ZecAgreementV1,
        context: &ZcashFundingContextV1,
    ) -> Result<ZcashFundingObservationV1, Self::Error> {
        self.3.record_event(ClaimPortEvent::ObserveFunding);
        self.3.record_funding_context(context);
        Ok(self.3.configured_funding_observation(agreement))
    }

    async fn prepare_followup_claim(
        &self,
        _agreement: &lez_zec_swap_sdk::ZecAgreementV1,
        context: &ZcashClaimContextV1,
        preimage: &ClaimPreimage,
    ) -> Result<PreparedClaimSubmissionV1, Self::Error> {
        self.3.record_zcash_context(context);
        let mut exact = vec![0x7a, 0x65, 0x63];
        exact.extend_from_slice(preimage.expose_secret());
        PreparedClaimSubmissionV1::new(ClaimStepV1::FollowupZcash, [0xc2; 32], exact)
            .map_err(|error| TestPortError(error.to_string()))
    }

    async fn observe_prepared_followup_claim(
        &self,
        agreement: &lez_zec_swap_sdk::ZecAgreementV1,
        context: &ZcashClaimContextV1,
        prepared: &PreparedClaimSubmissionV1,
    ) -> Result<FollowupClaimObservationV1, Self::Error> {
        self.3.record_zcash_context(context);
        if prepared.step() != ClaimStepV1::FollowupZcash
            || prepared.expected_submission_id() != &[0xc2; 32]
        {
            return Err(TestPortError(
                "unexpected prepared Zcash claim identity".to_owned(),
            ));
        }
        self.followup_observation(agreement)
    }

    async fn observe_counterparty_followup_claim(
        &self,
        agreement: &lez_zec_swap_sdk::ZecAgreementV1,
        context: &ZcashClaimContextV1,
    ) -> Result<FollowupClaimObservationV1, Self::Error> {
        self.3.record_zcash_context(context);
        self.followup_observation(agreement)
    }

    async fn submit_followup_claim(
        &self,
        _agreement: &lez_zec_swap_sdk::ZecAgreementV1,
        context: &ZcashClaimContextV1,
        prepared: &PreparedClaimSubmissionV1,
    ) -> Result<(), Self::Error> {
        self.3.record_zcash_context(context);
        if prepared.step() != ClaimStepV1::FollowupZcash
            || prepared.expected_submission_id() != &[0xc2; 32]
            || !prepared.exact_submission().starts_with(&[0x7a, 0x65, 0x63])
        {
            return Err(TestPortError(
                "unexpected deterministic Zcash claim bytes".to_owned(),
            ));
        }
        self.3.record_submission(ClaimStepV1::FollowupZcash);
        Ok(())
    }
}

#[async_trait]
impl ZcashRefundPort for MemoryZcashTakerLockObservation {
    type Error = TestPortError;

    async fn observe_refund_eligibility(
        &self,
        agreement: &lez_zec_swap_sdk::ZecAgreementV1,
        context: &ZcashFundingContextV1,
    ) -> Result<RefundEligibilityObservationV1, Self::Error> {
        if context.zcash_funder() != agreement.lez_claimant() {
            return Err(TestPortError("unexpected Zcash refund funder".to_owned()));
        }
        Ok(self.5.eligibility(RefundStepV1::Zcash))
    }

    async fn prepare_refund(
        &self,
        agreement: &lez_zec_swap_sdk::ZecAgreementV1,
        context: &ZcashFundingContextV1,
    ) -> Result<PreparedRefundSubmissionV1, Self::Error> {
        if context.zcash_funder() != agreement.lez_claimant() {
            return Err(TestPortError("unexpected Zcash refund funder".to_owned()));
        }
        self.5
            .record_event(RefundPortEvent::Prepare(RefundStepV1::Zcash));
        PreparedRefundSubmissionV1::new(
            RefundStepV1::Zcash,
            refund_submission_id(RefundStepV1::Zcash),
            refund_submission_bytes(RefundStepV1::Zcash).to_vec(),
        )
        .map_err(|error| TestPortError(error.to_string()))
    }

    async fn observe_prepared_refund(
        &self,
        agreement: &lez_zec_swap_sdk::ZecAgreementV1,
        context: &ZcashFundingContextV1,
        prepared: &PreparedRefundSubmissionV1,
    ) -> Result<RefundObservationV1, Self::Error> {
        if context.zcash_funder() != agreement.lez_claimant() {
            return Err(TestPortError("unexpected Zcash refund funder".to_owned()));
        }
        self.5
            .record_event(RefundPortEvent::ObservePrepared(RefundStepV1::Zcash));
        self.5
            .observe(agreement, RefundStepV1::Zcash, Some(prepared))
    }

    async fn observe_counterparty_refund(
        &self,
        agreement: &lez_zec_swap_sdk::ZecAgreementV1,
        context: &ZcashFundingContextV1,
    ) -> Result<RefundObservationV1, Self::Error> {
        if context.zcash_funder() != agreement.lez_claimant() {
            return Err(TestPortError("unexpected Zcash refund funder".to_owned()));
        }
        self.5
            .record_event(RefundPortEvent::ObserveCounterparty(RefundStepV1::Zcash));
        self.5.observe(agreement, RefundStepV1::Zcash, None)
    }

    async fn submit_refund(
        &self,
        agreement: &lez_zec_swap_sdk::ZecAgreementV1,
        context: &ZcashFundingContextV1,
        prepared: &PreparedRefundSubmissionV1,
    ) -> Result<RefundSubmitOutcomeV1, Self::Error> {
        if context.zcash_funder() != agreement.lez_claimant() {
            return Err(TestPortError("unexpected Zcash refund funder".to_owned()));
        }
        self.5.submit(RefundStepV1::Zcash, prepared)
    }
}

impl MemoryZcashTakerLockObservation {
    pub(crate) fn followup_observation(
        &self,
        agreement: &lez_zec_swap_sdk::ZecAgreementV1,
    ) -> Result<FollowupClaimObservationV1, TestPortError> {
        if *self
            .3
            .followup_confirmed
            .lock()
            .expect("follow-up claim lock")
        {
            FollowupClaimEvidenceV1::new(agreement, [0xc2; 32], "actor-zcash-followup-claim", 100)
                .map(FollowupClaimObservationV1::Confirmed)
                .map_err(|error| TestPortError(error.to_string()))
        } else {
            Ok(FollowupClaimObservationV1::Absent)
        }
    }
}

pub(crate) type FirstLockSubmissions = Vec<(FirstLockStepV1, Vec<u8>)>;

#[derive(Clone, Debug, Default)]
pub(crate) struct MemoryFirstLockPort {
    pub(crate) observations: Arc<Mutex<HashMap<FirstLockStepV1, FirstLockObservation>>>,
    pub(crate) submissions: Arc<Mutex<FirstLockSubmissions>>,
    pub(crate) fail_after_accept: Arc<Mutex<Vec<FirstLockStepV1>>>,
    /// Steps whose planned nonce the fake node reports as taken: (planned, live).
    pub(crate) superseded: Arc<Mutex<HashMap<FirstLockStepV1, (u128, u128)>>>,
}

impl MemoryFirstLockPort {
    pub(crate) fn observe_as(&self, step: FirstLockStepV1, observation: FirstLockObservation) {
        self.observations
            .lock()
            .expect("observations lock")
            .insert(step, observation);
    }

    pub(crate) fn submissions(&self) -> Vec<(FirstLockStepV1, Vec<u8>)> {
        self.submissions.lock().expect("submissions lock").clone()
    }

    pub(crate) fn fail_after_accept(&self, step: FirstLockStepV1) {
        self.fail_after_accept
            .lock()
            .expect("submit-failure lock")
            .push(step);
    }

    /// The next submission of `step` is refused as superseded, like a sidecar
    /// that sees the account past the planned nonce; nothing is recorded as
    /// submitted.
    pub(crate) fn supersede_as(
        &self,
        step: FirstLockStepV1,
        planned_nonce: u128,
        live_nonce: u128,
    ) {
        self.superseded
            .lock()
            .expect("superseded lock")
            .insert(step, (planned_nonce, live_nonce));
    }

    pub(crate) fn clear_superseded(&self, step: FirstLockStepV1) {
        self.superseded
            .lock()
            .expect("superseded lock")
            .remove(&step);
    }

    pub(crate) fn submit(
        &self,
        submission: &PreparedFirstLockSubmissionV1,
    ) -> Result<FirstLockSubmitOutcome, TestPortError> {
        if let Some((planned_nonce, live_nonce)) = self
            .superseded
            .lock()
            .expect("superseded lock")
            .get(&submission.step())
            .copied()
        {
            return Ok(FirstLockSubmitOutcome::Superseded {
                planned_nonce,
                live_nonce,
            });
        }
        self.submissions
            .lock()
            .expect("submissions lock")
            .push((submission.step(), submission.exact_submission().to_vec()));
        if self
            .fail_after_accept
            .lock()
            .expect("submit-failure lock")
            .contains(&submission.step())
        {
            Err(TestPortError(
                "node accepted before transport failure".to_owned(),
            ))
        } else {
            Ok(FirstLockSubmitOutcome::Accepted)
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct MemoryLezPort(pub(crate) MemoryFirstLockPort);

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
            .cloned()
            .unwrap_or(FirstLockObservation::Absent))
    }

    async fn submit_first_lock(
        &self,
        _agreement: &lez_zec_swap_sdk::ZecAgreementV1,
        submission: &PreparedFirstLockSubmissionV1,
    ) -> Result<FirstLockSubmitOutcome, Self::Error> {
        self.0.submit(submission)
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct MemoryZcashPort(pub(crate) MemoryFirstLockPort);

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
            .cloned()
            .unwrap_or(FirstLockObservation::Absent))
    }

    async fn submit_first_lock(
        &self,
        _agreement: &lez_zec_swap_sdk::ZecAgreementV1,
        submission: &PreparedFirstLockSubmissionV1,
    ) -> Result<(), Self::Error> {
        self.0.submit(submission).map(|_| ())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{0}")]
pub(crate) struct TestPortError(pub(crate) String);

pub(crate) fn canonical_lez_taker_lock(
    agreement: &lez_zec_swap_sdk::ZecAgreementV1,
) -> CanonicalLezEscrowObservationV1 {
    CanonicalLezEscrowObservationV1::validate(
        agreement,
        &canonical_lez_snapshot(agreement, LezSnapshotMutation::Pending),
    )
    .expect("agreement-bound canonical LEZ observation")
}

pub(crate) fn canonical_lez_claim_snapshot(
    agreement: &lez_zec_swap_sdk::ZecAgreementV1,
    preimage: [u8; 32],
) -> LezClaimNodeSnapshotV1 {
    let terms = agreement.lez_terms();
    let LezAssetV1::Native {
        authenticated_transfer_program_id,
    } = terms.asset()
    else {
        panic!("lifecycle fixture uses native LEZ")
    };
    let depositor = *agreement.lez_account(agreement.lez_depositor());
    let claimant = *agreement.lez_account(agreement.lez_claimant());
    let metadata = LezEscrowMetadataSnapshotV1::new(
        2,
        *agreement.onchain_swap_id(),
        *agreement.agreement_commitment(),
        *agreement.secret_digest(),
        depositor,
        depositor,
        claimant,
        claimant,
        *terms.custody_account(),
        *authenticated_transfer_program_id,
        *authenticated_transfer_program_id,
        [0; 32],
        terms.amount(),
        agreement.lez_refund_at_ms(),
        LezEscrowStatusV1::Claimed,
    );
    LezClaimNodeSnapshotV1::new(
        terms.chain().environment(),
        *terms.chain().channel_id(),
        *terms.chain().genesis_block_hash(),
        LezStableTipV1::new([0xc3; 32], 199, [0xc3; 32], 199),
        LezClaimTransactionSnapshotV1::new(
            [0xc1; 32],
            [0xc1; 32],
            *terms.escrow_program_id(),
            claimant,
            vec![
                *terms.metadata_account(),
                *terms.custody_account(),
                claimant,
            ],
            LezClaimInstructionV1::Native {
                swap_id: *agreement.onchain_swap_id(),
                preimage: ClaimPreimage::new(preimage),
            },
            true,
            true,
            100,
            [0xc2; 32],
            [0xc2; 32],
            LezInclusionStatusV1::Finalized,
        ),
        *terms.escrow_program_id(),
        *terms.metadata_account(),
        metadata,
        *terms.custody_account(),
        LezCustodySnapshotV1::Native {
            program_owner: *authenticated_transfer_program_id,
            balance: 0,
        },
    )
}

pub(crate) fn deeper_finalized_lez(
    agreement: &lez_zec_swap_sdk::ZecAgreementV1,
) -> CanonicalLezEscrowObservationV1 {
    CanonicalLezEscrowObservationV1::validate(
        agreement,
        &canonical_lez_snapshot(agreement, LezSnapshotMutation::DeeperFinalized),
    )
    .expect("same-inclusion finalized update")
}

pub(crate) fn canonical_lez_removal_for(
    agreement: &lez_zec_swap_sdk::ZecAgreementV1,
    previous: &CanonicalLezEscrowObservationV1,
    tip_hash: [u8; 32],
    tip_height: u64,
) -> CanonicalLezEscrowRemovalV1 {
    let chain = agreement.lez_terms().chain();
    CanonicalLezEscrowRemovalV1::validate(
        previous,
        &LezNodeRemovalSnapshotV1::new(
            chain.environment(),
            *chain.channel_id(),
            *chain.genesis_block_hash(),
            [0x61; 32],
            LezStableTipV1::new(tip_hash, tip_height, tip_hash, tip_height),
        ),
    )
    .expect("affirmative nonfinal LEZ removal")
}

#[derive(Clone, Copy)]
pub(crate) enum LezSnapshotMutation {
    Environment,
    Channel,
    Genesis,
    TipDrift,
    InclusionBlock,
    InclusionAboveTip,
    Program,
    Instruction,
    Signer,
    Accounts,
    SwapId,
    NonPublic,
    InvalidSignature,
    MetadataOwner,
    MetadataAccount,
    MetadataAmount,
    CustodyAccount,
    CustodyAmount,
    Pending,
    DeeperSafe,
    DeeperFinalized,
    ReplacementSafe,
    TipRegressionSafe,
}

// Keeping every one-field deviation in one factory makes the negative matrix
// auditable against the valid fixture.
#[allow(clippy::too_many_lines)]
pub(crate) fn canonical_lez_snapshot(
    agreement: &lez_zec_swap_sdk::ZecAgreementV1,
    mutation: LezSnapshotMutation,
) -> LezNodeSnapshotV1 {
    let terms = agreement.lez_terms();
    let LezAssetV1::Native {
        authenticated_transfer_program_id,
    } = terms.asset()
    else {
        panic!("lifecycle fixture uses native LEZ")
    };
    let depositor = *agreement.lez_account(agreement.lez_depositor());
    let claimant = *agreement.lez_account(agreement.lez_claimant());
    let environment = if matches!(mutation, LezSnapshotMutation::Environment) {
        LezEnvironmentV1::PublicTestnetV0_2
    } else {
        terms.chain().environment()
    };
    let genesis = if matches!(mutation, LezSnapshotMutation::Genesis) {
        [0x99; 32]
    } else {
        *terms.chain().genesis_block_hash()
    };
    let channel_id = if matches!(mutation, LezSnapshotMutation::Channel) {
        [0x98; 32]
    } else {
        *terms.chain().channel_id()
    };
    let metadata_amount = if matches!(mutation, LezSnapshotMutation::MetadataAmount) {
        terms.amount() + 1
    } else {
        terms.amount()
    };
    let metadata = LezEscrowMetadataSnapshotV1::new(
        2,
        *agreement.onchain_swap_id(),
        *agreement.agreement_commitment(),
        *agreement.secret_digest(),
        depositor,
        depositor,
        claimant,
        claimant,
        *terms.custody_account(),
        *authenticated_transfer_program_id,
        *authenticated_transfer_program_id,
        [0; 32],
        metadata_amount,
        agreement.lez_refund_at_ms(),
        LezEscrowStatusV1::Funded,
    );
    let program_id = if matches!(mutation, LezSnapshotMutation::Program) {
        [0x77; 8]
    } else {
        *terms.escrow_program_id()
    };
    let signer = if matches!(mutation, LezSnapshotMutation::Signer) {
        claimant
    } else {
        depositor
    };
    let accounts = if matches!(mutation, LezSnapshotMutation::Accounts) {
        vec![
            *terms.custody_account(),
            *terms.metadata_account(),
            depositor,
        ]
    } else {
        vec![
            *terms.metadata_account(),
            *terms.custody_account(),
            depositor,
        ]
    };
    let swap_id = if matches!(mutation, LezSnapshotMutation::SwapId) {
        [0x88; 32]
    } else {
        *agreement.onchain_swap_id()
    };
    let inclusion_height = match mutation {
        LezSnapshotMutation::InclusionAboveTip => 103,
        LezSnapshotMutation::ReplacementSafe => 101,
        _ => 100,
    };
    let canonical_block_hash = if matches!(mutation, LezSnapshotMutation::InclusionBlock) {
        [0x43; 32]
    } else {
        [0x41; 32]
    };
    let transaction_id = if matches!(mutation, LezSnapshotMutation::ReplacementSafe) {
        [0x32; 32]
    } else {
        [0x31; 32]
    };
    let inclusion_block_hash = if matches!(mutation, LezSnapshotMutation::ReplacementSafe) {
        [0x51; 32]
    } else {
        [0x41; 32]
    };
    let instruction = if matches!(mutation, LezSnapshotMutation::Instruction) {
        LezFundInstructionV1::Token { swap_id }
    } else {
        LezFundInstructionV1::Native { swap_id }
    };
    let transaction = LezFundTransactionSnapshotV1::new(
        transaction_id,
        program_id,
        signer,
        accounts,
        instruction,
        !matches!(mutation, LezSnapshotMutation::NonPublic),
        !matches!(mutation, LezSnapshotMutation::InvalidSignature),
        inclusion_height,
        inclusion_block_hash,
        if matches!(mutation, LezSnapshotMutation::ReplacementSafe) {
            inclusion_block_hash
        } else {
            canonical_block_hash
        },
        match mutation {
            LezSnapshotMutation::Pending => LezInclusionStatusV1::Pending,
            LezSnapshotMutation::DeeperSafe
            | LezSnapshotMutation::ReplacementSafe
            | LezSnapshotMutation::TipRegressionSafe => LezInclusionStatusV1::Safe,
            _ => LezInclusionStatusV1::Finalized,
        },
    );
    let tip = match mutation {
        LezSnapshotMutation::TipDrift => LezStableTipV1::new([0x42; 32], 102, [0x43; 32], 103),
        LezSnapshotMutation::DeeperSafe | LezSnapshotMutation::DeeperFinalized => {
            LezStableTipV1::new([0x44; 32], 103, [0x44; 32], 103)
        }
        LezSnapshotMutation::ReplacementSafe => {
            LezStableTipV1::new([0xbb; 32], 104, [0xbb; 32], 104)
        }
        _ => LezStableTipV1::new([0x42; 32], 102, [0x42; 32], 102),
    };
    let metadata_program_owner = if matches!(mutation, LezSnapshotMutation::MetadataOwner) {
        [0x66; 8]
    } else {
        *terms.escrow_program_id()
    };
    let metadata_account = if matches!(mutation, LezSnapshotMutation::MetadataAccount) {
        [0x55; 32]
    } else {
        *terms.metadata_account()
    };
    let custody_amount = if matches!(mutation, LezSnapshotMutation::CustodyAmount) {
        terms.amount() + 1
    } else {
        terms.amount()
    };
    let custody_account = if matches!(mutation, LezSnapshotMutation::CustodyAccount) {
        [0x54; 32]
    } else {
        *terms.custody_account()
    };
    LezNodeSnapshotV1::new(
        environment,
        channel_id,
        genesis,
        tip,
        transaction,
        metadata_program_owner,
        metadata_account,
        metadata,
        custody_account,
        LezCustodySnapshotV1::Native {
            program_owner: *authenticated_transfer_program_id,
            balance: custody_amount,
        },
    )
}

pub(crate) fn confirmed_observed_taker_lock(
    step: FirstLockStepV1,
    transaction_id: &str,
    confirmations: u32,
) -> TakerFirstLockObservationV1 {
    TakerFirstLockObservationV1::Confirmed(
        ObservedTakerFirstLockEvidenceV1::new(step, transaction_id, confirmations)
            .expect("well-formed primitive evidence"),
    )
}

pub(crate) fn canonical_zcash_taker_lock(
    agreement: &lez_zec_swap_sdk::ZecAgreementV1,
) -> CanonicalZcashOutputObservation {
    canonical_zcash_taker_lock_at_depth(agreement, [9; 32], 3)
}

pub(crate) fn exact_zcash_funding_observation(
    agreement: &lez_zec_swap_sdk::ZecAgreementV1,
) -> ZcashFundingObservationV1 {
    let canonical = canonical_zcash_taker_lock(agreement);
    let tip = ZcashStableTip::new(
        canonical.tip_block_hash(),
        canonical.tip_height(),
        canonical.tip_block_hash(),
        canonical.tip_height(),
    );
    let unspent = ZcashUnspentOutputSnapshotV1::new(
        canonical.outpoint().clone(),
        canonical.output().clone(),
        tip,
    );
    ZcashFundingObservationV1::confirmed(canonical, unspent)
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum FundingMutation {
    CanonicalTransactionId,
    CanonicalValue,
    CanonicalScript,
    Network,
    ConsensusBranch,
    InsufficientConfirmations,
    UnstableUnspentTip,
    UnspentTip,
    UnspentTransactionId,
    UnspentOutputIndex,
    UnspentValue,
    UnspentScript,
    RefundHeightReached,
}

pub(crate) fn mutated_zcash_funding_observation(
    agreement: &lez_zec_swap_sdk::ZecAgreementV1,
    mutation: FundingMutation,
) -> ZcashFundingObservationV1 {
    let agreed = agreement.binding().expected_output();
    let alternate = alternate_expected_funding(agreement, mutation);
    let tip_height = if matches!(mutation, FundingMutation::RefundHeightReached) {
        BlockHeight::from_u32(agreement.zcash_refund_at_height())
    } else if matches!(mutation, FundingMutation::InsufficientConfirmations) {
        BlockHeight::from_u32(100)
    } else {
        BlockHeight::from_u32(102)
    };
    let input_transaction_id = if matches!(mutation, FundingMutation::CanonicalTransactionId) {
        [8; 32]
    } else {
        [9; 32]
    };
    let canonical = canonical_zcash_taker_lock_fixture_for_expected(
        alternate.as_ref().unwrap_or(agreed),
        input_transaction_id,
        BlockHash([0x44; 32]),
        BlockHash([0xaa; 32]),
        tip_height,
    );
    mutated_unspent_funding(canonical, mutation)
}

pub(crate) fn alternate_expected_funding(
    agreement: &lez_zec_swap_sdk::ZecAgreementV1,
    mutation: FundingMutation,
) -> Option<ExpectedBip199Output> {
    let agreed = agreement.binding().expected_output();
    match mutation {
        FundingMutation::CanonicalValue => Some(ExpectedBip199Output::new(
            agreed.network(),
            agreed.consensus_branch_id(),
            Zatoshis::from_u64(
                u64::from(agreed.value())
                    .checked_add(1)
                    .expect("alternate funding value"),
            )
            .expect("alternate funding value"),
            agreed.contract().clone(),
        )),
        FundingMutation::CanonicalScript => Some(ExpectedBip199Output::new(
            agreed.network(),
            agreed.consensus_branch_id(),
            agreed.value(),
            Bip199Contract::new(
                agreed.contract().refund_lock_time(),
                [0x31; 20],
                *agreement.secret_digest(),
                [0x32; 20],
            ),
        )),
        FundingMutation::Network => Some(ExpectedBip199Output::new(
            NetworkType::Test,
            agreed.consensus_branch_id(),
            agreed.value(),
            agreed.contract().clone(),
        )),
        FundingMutation::ConsensusBranch => Some(ExpectedBip199Output::new(
            agreed.network(),
            BranchId::Nu6_1,
            agreed.value(),
            agreed.contract().clone(),
        )),
        _ => None,
    }
}

pub(crate) fn mutated_unspent_funding(
    canonical: CanonicalZcashOutputObservation,
    mutation: FundingMutation,
) -> ZcashFundingObservationV1 {
    let mut outpoint = canonical.outpoint().clone();
    if matches!(mutation, FundingMutation::UnspentTransactionId) {
        outpoint = OutPoint::new([0x77; 32], 0);
    } else if matches!(mutation, FundingMutation::UnspentOutputIndex) {
        outpoint = OutPoint::new(*canonical.transaction_id().as_ref(), 1);
    }
    let output = match mutation {
        FundingMutation::UnspentValue => TxOut::new(
            Zatoshis::from_u64(
                u64::from(canonical.output().value())
                    .checked_add(1)
                    .expect("alternate UTXO value"),
            )
            .expect("alternate UTXO value"),
            canonical.output().script_pubkey().clone(),
        ),
        FundingMutation::UnspentScript => {
            let key = SecretKey::from_slice(&[8; 32]).expect("alternate UTXO owner key");
            let public_key = PublicKey::from_secret_key(&Secp256k1::new(), &key);
            TxOut::new(
                canonical.output().value(),
                TransparentAddress::from_pubkey(&public_key).script().into(),
            )
        }
        _ => canonical.output().clone(),
    };
    let unspent_tip_height = if matches!(mutation, FundingMutation::UnspentTip) {
        BlockHeight::from_u32(u32::from(canonical.tip_height()) + 1)
    } else {
        canonical.tip_height()
    };
    let after_tip_hash = if matches!(mutation, FundingMutation::UnstableUnspentTip) {
        BlockHash([0xbb; 32])
    } else {
        canonical.tip_block_hash()
    };
    let unspent = ZcashUnspentOutputSnapshotV1::new(
        outpoint,
        output,
        ZcashStableTip::new(
            canonical.tip_block_hash(),
            unspent_tip_height,
            after_tip_hash,
            unspent_tip_height,
        ),
    );
    ZcashFundingObservationV1::confirmed(canonical, unspent)
}

pub(crate) fn canonical_zcash_taker_lock_with_input(
    agreement: &lez_zec_swap_sdk::ZecAgreementV1,
    input_transaction_id: [u8; 32],
) -> CanonicalZcashOutputObservation {
    canonical_zcash_taker_lock_at_depth(agreement, input_transaction_id, 3)
}

pub(crate) fn canonical_zcash_taker_lock_at_depth(
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

pub(crate) fn canonical_zcash_replacement(
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

pub(crate) fn canonical_zcash_replacement_depth_update(
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

pub(crate) fn canonical_zcash_taker_lock_fixture(
    agreement: &lez_zec_swap_sdk::ZecAgreementV1,
    input_transaction_id: [u8; 32],
    transaction_block_hash: BlockHash,
    tip_block_hash: BlockHash,
    tip_height: BlockHeight,
) -> CanonicalZcashOutputObservation {
    let expected = agreement.binding().expected_output();
    canonical_zcash_taker_lock_fixture_for_expected(
        expected,
        input_transaction_id,
        transaction_block_hash,
        tip_block_hash,
        tip_height,
    )
}

pub(crate) fn canonical_zcash_taker_lock_fixture_for_expected(
    expected: &ExpectedBip199Output,
    input_transaction_id: [u8; 32],
    transaction_block_hash: BlockHash,
    tip_block_hash: BlockHash,
    tip_height: BlockHeight,
) -> CanonicalZcashOutputObservation {
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

pub(crate) fn canonical_zcash_removal(
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

pub(crate) fn actor_sdk(
    participant: Participant,
    wire: Vec<u8>,
    lez: MemoryLezTakerLockObservation,
    zcash: MemoryZcashTakerLockObservation,
    store: MemoryStore,
) -> ZecPairSdk<
    MemoryDiscovery,
    MemoryNegotiation,
    MemoryLezTakerLockObservation,
    MemoryZcashTakerLockObservation,
    MemoryStore,
> {
    ZecPairSdk::new(
        participant,
        MemoryDiscovery::default(),
        MemoryNegotiation { wire },
        lez,
        zcash,
        store,
    )
}

pub(crate) fn sdk(
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

pub(crate) fn first_lock_sdk(
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

pub(crate) fn maker_lock_sdk(
    wire: Vec<u8>,
    lez: MemoryLezTakerLockObservation,
    zcash: MemoryZcashTakerLockObservation,
    store: MemoryStore,
) -> ZecPairSdk<
    MemoryDiscovery,
    MemoryNegotiation,
    MemoryLezTakerLockObservation,
    MemoryZcashTakerLockObservation,
    MemoryStore,
> {
    ZecPairSdk::new(
        Participant::Maker,
        MemoryDiscovery::default(),
        MemoryNegotiation { wire },
        lez,
        zcash,
        store,
    )
}

pub(crate) fn zcash_first_lock_plan(
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

pub(crate) fn lez_first_lock_plan(
    initialize_id: [u8; 32],
    initialize_submission: Vec<u8>,
    fund_id: [u8; 32],
    fund_submission: Vec<u8>,
) -> FirstLockPlanV1 {
    FirstLockPlanV1::lez(
        PreparedFirstLockSubmissionV1::new(
            FirstLockStepV1::LezInitialize,
            initialize_id,
            initialize_submission,
        )
        .expect("initialize"),
        PreparedFirstLockSubmissionV1::new(FirstLockStepV1::LezFund, fund_id, fund_submission)
            .expect("fund"),
    )
    .expect("LEZ plan")
}

pub(crate) fn confirmed_zcash_first_lock(
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

pub(crate) fn confirmed_first_lock(
    step: FirstLockStepV1,
    expected_submission_id: [u8; 32],
    transaction_id: &str,
) -> FirstLockConfirmedEvidenceV1 {
    FirstLockConfirmedEvidenceV1::from_observation(
        step,
        expected_submission_id,
        transaction_id.to_owned(),
        100,
    )
    .expect("confirmed evidence")
}

pub(crate) fn confirmed_first_lock_observation(
    step: FirstLockStepV1,
    expected_submission_id: [u8; 32],
    transaction_id: &str,
) -> FirstLockObservation {
    FirstLockObservation::Confirmed(confirmed_first_lock(
        step,
        expected_submission_id,
        transaction_id,
    ))
}

pub(crate) fn confirmed_lez_initialize(
    expected_submission_id: [u8; 32],
    transaction_id: &str,
) -> FirstLockObservation {
    confirmed_first_lock_observation(
        FirstLockStepV1::LezInitialize,
        expected_submission_id,
        transaction_id,
    )
}

pub(crate) fn envelope(
    wire: Vec<u8>,
    participant: Participant,
    revision: u64,
) -> AcceptedZecAgreementEnvelopeV1 {
    AcceptedZecAgreementEnvelopeV1::from_durable_parts(wire, ACCEPTED_AT, participant, revision)
}

#[derive(Clone, Copy)]
pub(crate) enum FixtureVariant {
    Local,
    ChangedTranscript,
    Public,
    CorruptCommitment,
}

pub(crate) fn agreement_wire(
    id: &str,
    direction: SwapDirection,
    variant: FixtureVariant,
) -> Vec<u8> {
    agreement_wire_with_digest(id, direction, variant, [9; 32])
}

pub(crate) fn agreement_wire_with_digest(
    id: &str,
    direction: SwapDirection,
    variant: FixtureVariant,
    digest: [u8; 32],
) -> Vec<u8> {
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
            LezChainIdentityV1::new(environment, [8; 32], [7; 32]),
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
        ZEC_CONCRETE_AGREEMENT_SCHEMA_V2,
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

pub(crate) struct FixtureDeployment {
    pub(crate) profile: ZecProfileId,
    pub(crate) environment: LezEnvironmentV1,
    pub(crate) network: NetworkType,
    pub(crate) zcash_anchor: u32,
    pub(crate) zcash_refund_lock: u32,
    pub(crate) earlier_latest_ms: u64,
    pub(crate) later_earliest: u64,
}

pub(crate) fn fixture_deployment(is_public: bool) -> FixtureDeployment {
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

pub(crate) fn fixture_binding(
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

pub(crate) fn pubkey_hash(bytes: &[u8; 33]) -> [u8; 20] {
    match TransparentAddress::from_pubkey(&PublicKey::from_slice(bytes).expect("fixture pubkey")) {
        TransparentAddress::PublicKeyHash(hash) => hash,
        TransparentAddress::ScriptHash(_) => unreachable!("public keys produce P2PKH"),
    }
}
