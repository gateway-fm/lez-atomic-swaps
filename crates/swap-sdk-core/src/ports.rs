//! Optional pre-lock application ports.

use std::error::Error;

use async_trait::async_trait;
use lez_swap_core::Participant;

/// Authenticated, expiring offer discovery supplied by a Delivery adapter.
///
/// Implementors authenticate publishers and reject expired offers before
/// returning a reference. Queries cannot change protocol state, and the handle
/// is not required after negotiation.
#[async_trait]
pub trait OfferDiscovery: Send + Sync {
    /// Adapter error retaining its structured source.
    type Error: Error + Send + Sync + 'static;
    /// Versioned offer published by a maker.
    type Offer: Send + Sync;
    /// Authenticated reference returned to discovery clients.
    type OfferRef: Clone + Send + Sync;
    /// Adapter-specific read-only discovery query.
    type Query: Send + Sync;

    /// Publishes one authenticated, expiring maker offer.
    async fn publish(&self, offer: Self::Offer) -> Result<Self::OfferRef, Self::Error>;

    /// Returns only authenticated, unexpired offer references.
    async fn discover(&self, query: &Self::Query) -> Result<Vec<Self::OfferRef>, Self::Error>;
}

/// Mutually authenticated pre-lock negotiation supplied by a Chat adapter.
///
/// The returned bytes remain untrusted. A pair SDK must enforce its wire-size
/// bound, decode the exact schema version, verify both roles' signatures, and
/// validate the transcript at a trusted local time. This port is deliberately
/// absent from [`crate::SwapProtocol`].
#[async_trait]
pub trait NegotiationChannel: Send + Sync {
    /// Adapter error retaining its structured source.
    type Error: Error + Send + Sync + 'static;
    /// Role-local proposal consumed by negotiation.
    type LocalProposal: Send + Sync;
    /// Authenticated offer reference accepted by this channel.
    type OfferRef: Clone + Send + Sync;

    /// Produces the same untrusted countersigned wire record for both roles.
    async fn negotiate(
        &self,
        local_participant: Participant,
        offer: &Self::OfferRef,
        proposal: Self::LocalProposal,
    ) -> Result<Vec<u8>, Self::Error>;
}
