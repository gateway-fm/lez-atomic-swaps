//! Async application ports for the complete LEZ/ZEC lifecycle.

use std::error::Error;

use async_trait::async_trait;
use lez_swap_core::{Participant, SwapId};

use crate::ZecAgreement;

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
/// Implementors return only a countersigned immutable agreement. Raw peer
/// messages never enter the deterministic coordinator.
#[async_trait]
pub trait NegotiationChannel: Send + Sync {
    /// Adapter error with its structured source retained by the SDK.
    type Error: Error + Send + Sync + 'static;
    /// Typed LEZ terms produced by the generated escrow client.
    type LezTerms: Clone + std::fmt::Debug + Eq + Send + Sync + 'static;
    /// Role-local negotiation proposal.
    type LocalProposal: Send + Sync;
    /// Offer reference accepted by this channel.
    type OfferRef: Clone + Send + Sync;

    /// Produces the same countersigned agreement for both independent roles.
    async fn negotiate(
        &self,
        local_participant: Participant,
        offer: &Self::OfferRef,
        proposal: Self::LocalProposal,
    ) -> Result<ZecAgreement<Self::LezTerms>, Self::Error>;
}

/// Durable role-local agreement storage used before any external lock.
#[async_trait]
pub trait RecoveryStore<LezTerms>: Clone + Send + Sync
where
    LezTerms: Clone + std::fmt::Debug + Eq + Send + Sync + 'static,
{
    /// Store error with its structured source retained by the SDK.
    type Error: Error + Send + Sync + 'static;

    /// Atomically creates the immutable agreement and returns its revision.
    async fn create_agreement(
        &self,
        local_participant: Participant,
        agreement: &ZecAgreement<LezTerms>,
    ) -> Result<u64, Self::Error>;

    /// Loads one independently persisted role-local agreement.
    async fn load_agreement(
        &self,
        local_participant: Participant,
        swap_id: &SwapId,
    ) -> Result<Option<(u64, ZecAgreement<LezTerms>)>, Self::Error>;
}
