//! Async application ports for the evolving LEZ/ZEC SDK lifecycle.

use std::error::Error;

use async_trait::async_trait;
use lez_swap_core::{Participant, SwapId};

use crate::{
    AcceptedZecAgreementEnvelopeV1, CreateFirstLockOutcome, FirstLockIntentV1,
    FirstLockObservation, PreparedFirstLockSubmissionV1,
};

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
}
