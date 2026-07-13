//! Async application ports for the evolving LEZ/ZEC SDK lifecycle.

use std::error::Error;

use async_trait::async_trait;
use lez_swap_core::{Participant, SwapId};

use crate::{
    AcceptedZecAgreementEnvelopeV1, ClaimIntentV1, ClaimPreimage, CreateFirstLockOutcome,
    FirstLockIntentV1, FirstLockObservation, FirstLockProjectionCommit, FirstLockTransitionV1,
    FollowupClaimObservationV1, FollowupClaimTransitionV1, MakerLockIntentV1,
    MakerLockObservationV1, MakerLockTransitionV1, ObservedFollowupClaimTransitionV1,
    ObservedMakerLockTransitionV1, ObservedRevealingClaimTransitionV1,
    ObservedTakerFirstLockTransitionV1, PreparedClaimSubmissionV1, PreparedFirstLockSubmissionV1,
    ProtectedClaimPayloadEnvelope, RevealingClaimObservationV1, RevealingClaimTransitionV1,
    TakerFirstLockObservationV1, ZecAgreementV1,
};

/// Narrow LEZ boundary for the preimage-revealing first claim.
#[async_trait]
pub trait LezClaimPort: Send + Sync {
    /// Structured adapter, RPC, or signing error retained by the SDK.
    type Error: Error + Send + Sync + 'static;
    /// Derives and signs exact agreement-bound LEZ claim bytes.
    async fn prepare_revealing_claim(
        &self,
        agreement: &ZecAgreementV1,
        preimage: &ClaimPreimage,
    ) -> Result<PreparedClaimSubmissionV1, Self::Error>;
    /// Observes the exact durable LEZ claim identity before any rebroadcast.
    async fn observe_prepared_revealing_claim(
        &self,
        agreement: &ZecAgreementV1,
        prepared: &PreparedClaimSubmissionV1,
    ) -> Result<RevealingClaimObservationV1, Self::Error>;
    /// Observes the counterparty's agreement-bound LEZ reveal without a local plan.
    async fn observe_counterparty_revealing_claim(
        &self,
        agreement: &ZecAgreementV1,
    ) -> Result<RevealingClaimObservationV1, Self::Error>;
    /// Submits exact bytes reopened from protected durable storage.
    async fn submit_revealing_claim(
        &self,
        agreement: &ZecAgreementV1,
        prepared: &PreparedClaimSubmissionV1,
    ) -> Result<(), Self::Error>;
}

/// Narrow Zcash boundary for the preimage-consuming follow-up claim.
#[async_trait]
pub trait ZcashClaimPort: Send + Sync {
    /// Structured adapter, RPC, or signing error retained by the SDK.
    type Error: Error + Send + Sync + 'static;
    /// Derives and signs exact agreement-bound Zcash claim bytes.
    async fn prepare_followup_claim(
        &self,
        agreement: &ZecAgreementV1,
        preimage: &ClaimPreimage,
    ) -> Result<PreparedClaimSubmissionV1, Self::Error>;
    /// Observes the exact durable Zcash claim identity before any rebroadcast.
    async fn observe_prepared_followup_claim(
        &self,
        agreement: &ZecAgreementV1,
        prepared: &PreparedClaimSubmissionV1,
    ) -> Result<FollowupClaimObservationV1, Self::Error>;
    /// Observes the counterparty's agreement-bound Zcash follow-up without a local plan.
    async fn observe_counterparty_followup_claim(
        &self,
        agreement: &ZecAgreementV1,
    ) -> Result<FollowupClaimObservationV1, Self::Error>;
    /// Submits exact bytes reopened from protected durable storage.
    async fn submit_followup_claim(
        &self,
        agreement: &ZecAgreementV1,
        prepared: &PreparedClaimSubmissionV1,
    ) -> Result<(), Self::Error>;
}

/// Authenticated, expiring offer discovery supplied by a Delivery adapter.
///
/// The SDK deliberately does not invent a Delivery wire protocol. Implementors
/// must authenticate and expiry-check every returned offer reference.
#[async_trait]
pub trait OfferDiscovery: Send + Sync {
    /// Adapter error with its structured source retained by the SDK.
    type Error: Error + Send + Sync + 'static;
    /// Offer published by the maker.
    type Offer: Send + Sync;
    /// Authenticated reference returned to discovery clients.
    type OfferRef: Clone + Send + Sync;
    /// Adapter-specific query that cannot affect protocol state.
    type Query: Send + Sync;

    /// Publishes one authenticated, expiring maker offer.
    async fn publish(&self, offer: Self::Offer) -> Result<Self::OfferRef, Self::Error>;

    /// Discovers authenticated, unexpired offer references.
    async fn discover(&self, query: &Self::Query) -> Result<Vec<Self::OfferRef>, Self::Error>;
}

/// Mutually authenticated pre-lock negotiation supplied by a Chat adapter.
///
/// Implementors return untrusted bounded-wire candidates. The SDK, rather than
/// the transport adapter, validates every byte at a trusted local timestamp.
#[async_trait]
pub trait NegotiationChannel: Send + Sync {
    /// Adapter error with its structured source retained by the SDK.
    type Error: Error + Send + Sync + 'static;
    /// Role-local negotiation proposal.
    type LocalProposal: Send + Sync;
    /// Offer reference accepted by this channel.
    type OfferRef: Clone + Send + Sync;

    /// Produces the same untrusted countersigned wire record for both roles.
    async fn negotiate(
        &self,
        local_participant: Participant,
        offer: &Self::OfferRef,
        proposal: Self::LocalProposal,
    ) -> Result<Vec<u8>, Self::Error>;
}

/// Typed LEZ first-lock action and observation boundary.
///
/// Implementors must decode the exact submission, recompute deployment and
/// account identities from the selected chain, and report `Confirmed` only for
/// stable canonical inclusion at the agreement-required depth.
#[async_trait]
pub trait LezFirstLockPort: Send + Sync {
    /// Structured adapter/RPC error retained by the SDK.
    type Error: Error + Send + Sync + 'static;

    /// Observes the exact expected identity before any possible rebroadcast.
    async fn observe_first_lock(
        &self,
        agreement: &crate::ZecAgreementV1,
        submission: &PreparedFirstLockSubmissionV1,
    ) -> Result<FirstLockObservation, Self::Error>;

    /// Submits byte-identical durable material after a stable absence.
    async fn submit_first_lock(
        &self,
        agreement: &crate::ZecAgreementV1,
        submission: &PreparedFirstLockSubmissionV1,
    ) -> Result<(), Self::Error>;
}

/// Typed Zcash first-lock action and observation boundary.
///
/// Implementors must decode the exact V5 transaction, recompute its txid and
/// agreement policy, and report `Confirmed` only for stable canonical inclusion
/// at the agreement-required depth.
#[async_trait]
pub trait ZcashFirstLockPort: Send + Sync {
    /// Structured adapter/RPC error retained by the SDK.
    type Error: Error + Send + Sync + 'static;

    /// Observes the exact expected txid before any possible rebroadcast.
    async fn observe_first_lock(
        &self,
        agreement: &crate::ZecAgreementV1,
        submission: &PreparedFirstLockSubmissionV1,
    ) -> Result<FirstLockObservation, Self::Error>;

    /// Submits byte-identical durable transaction bytes after a stable absence.
    async fn submit_first_lock(
        &self,
        agreement: &crate::ZecAgreementV1,
        submission: &PreparedFirstLockSubmissionV1,
    ) -> Result<(), Self::Error>;
}

/// Observation-only LEZ boundary used by the maker for the taker's first lock.
///
/// Implementors must derive every account and funded-state expectation from
/// the accepted agreement and return confirmed evidence only for stable
/// canonical inclusion. This port has no submission operation.
#[async_trait]
pub trait LezTakerFirstLockObservationPort: Send + Sync {
    /// Structured adapter/RPC error retained by the SDK.
    type Error: Error + Send + Sync + 'static;

    /// Observes the taker's agreement-bound LEZ escrow funding.
    async fn observe_taker_first_lock(
        &self,
        agreement: &crate::ZecAgreementV1,
        previous: Option<&crate::CanonicalLezEscrowObservationV1>,
    ) -> Result<TakerFirstLockObservationV1, Self::Error>;
}

/// Observation-only Zcash boundary used by the maker for the taker's first lock.
///
/// Implementors must validate the canonical transaction/output against the
/// accepted BIP-199 binding and stable tip. This port has no submission
/// operation.
#[async_trait]
pub trait ZcashTakerFirstLockObservationPort: Send + Sync {
    /// Structured adapter/RPC error retained by the SDK.
    type Error: Error + Send + Sync + 'static;

    /// Observes the taker's agreement-bound transparent funding output.
    async fn observe_taker_first_lock(
        &self,
        agreement: &crate::ZecAgreementV1,
    ) -> Result<TakerFirstLockObservationV1, Self::Error>;
}

/// Observation-only LEZ boundary used by the taker for the maker's second lock.
///
/// The adapter must validate stable canonical inclusion against the accepted
/// agreement. Its expected submission identity is adapter-asserted because the
/// taker has neither the maker's exact plan nor its role-local durable intent.
#[async_trait]
pub trait LezMakerLockObservationPort: Send + Sync {
    /// Structured adapter/RPC error retained by the SDK.
    type Error: Error + Send + Sync + 'static;

    /// Observes the agreement-directed maker LEZ funding transaction.
    async fn observe_maker_lock(
        &self,
        agreement: &crate::ZecAgreementV1,
    ) -> Result<MakerLockObservationV1, Self::Error>;
}

/// Observation-only Zcash boundary used by the taker for the maker's second lock.
///
/// The adapter validates the stable canonical output and asserts its expected
/// identity. The taker binds that assertion durably before applying it.
#[async_trait]
pub trait ZcashMakerLockObservationPort: Send + Sync {
    /// Structured adapter/RPC error retained by the SDK.
    type Error: Error + Send + Sync + 'static;

    /// Observes the agreement-directed maker Zcash funding transaction.
    async fn observe_maker_lock(
        &self,
        agreement: &crate::ZecAgreementV1,
    ) -> Result<MakerLockObservationV1, Self::Error>;
}

/// Atomic result of creating one immutable role-local agreement record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CreateAgreementOutcome {
    /// No record existed and the exact envelope was durably created.
    Created,
    /// The exact same envelope was already durable; retry is idempotent.
    ExistingSame,
    /// The same role-local swap key contains different immutable bytes.
    Conflict,
}

/// Durable role-local agreement storage used before any external lock.
#[async_trait]
pub trait RecoveryStore: Clone + Send + Sync {
    /// Store error with its structured source retained by the SDK.
    type Error: Error + Send + Sync + 'static;

    /// Atomically creates the exact immutable envelope.
    ///
    /// `ExistingSame` is permitted only for an exact replay of every field.
    /// Changed wire, acceptance time, local role, or revision is Conflict.
    async fn create_agreement(
        &self,
        envelope: &AcceptedZecAgreementEnvelopeV1,
    ) -> Result<CreateAgreementOutcome, Self::Error>;

    /// Loads one untrusted durable envelope for the requested application ID.
    ///
    /// The SDK revalidates wire, role, revision, and requested ID.
    async fn load_agreement(
        &self,
        swap_id: &SwapId,
    ) -> Result<Option<AcceptedZecAgreementEnvelopeV1>, Self::Error>;

    /// Atomically creates the exact role-local first-lock plan before any node call.
    async fn create_first_lock_intent(
        &self,
        intent: &FirstLockIntentV1,
    ) -> Result<CreateFirstLockOutcome, Self::Error>;

    /// Loads the exact pending first-lock recovery plan after restart.
    async fn load_first_lock_intent(
        &self,
        swap_id: &SwapId,
    ) -> Result<Option<FirstLockIntentV1>, Self::Error>;

    /// Atomically commits exact confirmed evidence with the next aggregate
    /// revision and closes the matching pending intent.
    async fn commit_first_lock_transition(
        &self,
        transition: &FirstLockTransitionV1,
    ) -> Result<FirstLockProjectionCommit, Self::Error>;

    /// Probes one exact predecessor slot after an unknown commit outcome and
    /// loads it during restart replay.
    async fn load_first_lock_transition(
        &self,
        swap_id: &SwapId,
        predecessor_revision: u64,
    ) -> Result<Option<FirstLockTransitionV1>, Self::Error>;

    /// Atomically commits the maker's independent taker-lock observation.
    async fn commit_observed_taker_first_lock_transition(
        &self,
        transition: &ObservedTakerFirstLockTransitionV1,
    ) -> Result<FirstLockProjectionCommit, Self::Error>;

    /// Loads one exact maker observation predecessor slot for probe/restart.
    async fn load_observed_taker_first_lock_transition(
        &self,
        swap_id: &SwapId,
        predecessor_revision: u64,
    ) -> Result<Option<ObservedTakerFirstLockTransitionV1>, Self::Error>;

    /// Atomically commits the taker's independent maker-lock observation.
    async fn commit_observed_maker_lock_transition(
        &self,
        transition: &ObservedMakerLockTransitionV1,
    ) -> Result<FirstLockProjectionCommit, Self::Error>;

    /// Loads one exact taker observation predecessor slot for probe/restart.
    async fn load_observed_maker_lock_transition(
        &self,
        swap_id: &SwapId,
        predecessor_revision: u64,
    ) -> Result<Option<ObservedMakerLockTransitionV1>, Self::Error>;

    /// Atomically creates the maker's exact opposite-chain plan before any node call.
    async fn create_maker_lock_intent(
        &self,
        intent: &MakerLockIntentV1,
    ) -> Result<CreateFirstLockOutcome, Self::Error>;

    /// Loads the pending maker plan; its staging revision may precede the active head.
    async fn load_maker_lock_intent(
        &self,
        swap_id: &SwapId,
    ) -> Result<Option<MakerLockIntentV1>, Self::Error>;

    /// Atomically commits maker funding and closes its retained intent.
    async fn commit_maker_lock_transition(
        &self,
        transition: &MakerLockTransitionV1,
    ) -> Result<FirstLockProjectionCommit, Self::Error>;

    /// Loads one exact maker-funding predecessor slot for probe/restart replay.
    async fn load_maker_lock_transition(
        &self,
        swap_id: &SwapId,
        predecessor_revision: u64,
    ) -> Result<Option<MakerLockTransitionV1>, Self::Error>;
}

/// Recovery storage that can atomically bind an agreement to local claim material.
///
/// Implementors must protect the plaintext preimage before durable storage.
/// Returning `Created` or `ExistingSame` guarantees that the exact agreement and
/// its matching protected material are both recoverable.
#[async_trait]
pub trait ClaimRecoveryStore: RecoveryStore {
    /// Atomically creates the immutable agreement and protected local preimage.
    ///
    /// Exact retry is idempotent. A changed agreement or changed claim
    /// material under the same role-local swap key is a conflict.
    async fn create_agreement_with_local_claim_material(
        &self,
        envelope: &AcceptedZecAgreementEnvelopeV1,
        preimage: &ClaimPreimage,
    ) -> Result<CreateAgreementOutcome, Self::Error>;

    /// Loads and authenticates local claim material for a durable agreement.
    async fn load_claim_material(
        &self,
        swap_id: &SwapId,
    ) -> Result<Option<ClaimPreimage>, Self::Error>;

    /// Encrypts exact claim bytes with fresh nonce and canonical context.
    async fn protect_claim_submission(
        &self,
        agreement: &ZecAgreementV1,
        local_participant: Participant,
        staged_revision: u64,
        prepared: &PreparedClaimSubmissionV1,
    ) -> Result<ProtectedClaimPayloadEnvelope, Self::Error>;

    /// Authenticates and decrypts the envelope bound by a durable intent.
    async fn open_claim_submission(
        &self,
        agreement: &ZecAgreementV1,
        intent: &ClaimIntentV1,
        protected: &ProtectedClaimPayloadEnvelope,
    ) -> Result<PreparedClaimSubmissionV1, Self::Error>;

    /// Atomically creates one claim intent and protected exact submission.
    async fn create_claim_intent(
        &self,
        intent: &ClaimIntentV1,
        protected: &ProtectedClaimPayloadEnvelope,
    ) -> Result<CreateFirstLockOutcome, Self::Error>;

    /// Loads the pending intent and its protected exact submission.
    async fn load_claim_intent(
        &self,
        swap_id: &SwapId,
    ) -> Result<Option<(ClaimIntentV1, ProtectedClaimPayloadEnvelope)>, Self::Error>;

    /// Commits revealing evidence, closes its intent, and atomically protects
    /// an observed preimage when local material does not yet exist.
    async fn commit_revealing_claim_transition(
        &self,
        transition: &RevealingClaimTransitionV1,
    ) -> Result<FirstLockProjectionCommit, Self::Error>;

    /// Loads the exact revealing predecessor slot for restart replay.
    async fn load_revealing_claim_transition(
        &self,
        swap_id: &SwapId,
        predecessor_revision: u64,
    ) -> Result<Option<RevealingClaimTransitionV1>, Self::Error>;

    /// Atomically commits an independently observed LEZ reveal together with
    /// protected extracted material before advancing the role-local revision.
    async fn commit_observed_revealing_claim_transition(
        &self,
        transition: &ObservedRevealingClaimTransitionV1,
    ) -> Result<FirstLockProjectionCommit, Self::Error>;

    /// Loads the exact independently observed LEZ reveal predecessor slot.
    async fn load_observed_revealing_claim_transition(
        &self,
        swap_id: &SwapId,
        predecessor_revision: u64,
    ) -> Result<Option<ObservedRevealingClaimTransitionV1>, Self::Error>;

    /// Atomically commits follow-up evidence and closes its intent.
    async fn commit_followup_claim_transition(
        &self,
        transition: &FollowupClaimTransitionV1,
    ) -> Result<FirstLockProjectionCommit, Self::Error>;

    /// Loads the exact follow-up predecessor slot for restart replay.
    async fn load_followup_claim_transition(
        &self,
        swap_id: &SwapId,
        predecessor_revision: u64,
    ) -> Result<Option<FollowupClaimTransitionV1>, Self::Error>;

    /// Atomically commits the first claimant's independent observation of the
    /// counterparty Zcash follow-up and advances its role-local revision.
    async fn commit_observed_followup_claim_transition(
        &self,
        transition: &ObservedFollowupClaimTransitionV1,
    ) -> Result<FirstLockProjectionCommit, Self::Error>;

    /// Loads the exact independently observed Zcash follow-up predecessor slot.
    async fn load_observed_followup_claim_transition(
        &self,
        swap_id: &SwapId,
        predecessor_revision: u64,
    ) -> Result<Option<ObservedFollowupClaimTransitionV1>, Self::Error>;
}
