//! Transport-free implementation of the first read-only Taker facade methods.
//!
//! Delivery locations, pinned identities, dependency probes, and trusted time
//! are injected by the owner process. Neither successful responses nor errors
//! expose those private authority details.

use std::{collections::BTreeMap, fmt};

use lez_swap_sdk_core::OfferDiscovery as _;
use lez_swap_store::MakerRouteV1;
use thiserror::Error;

use crate::{
    DeliveryOfferQueryV1, RunLocalDelivery, TAKER_FACADE_SCHEMA_VERSION_V1, TakerDependencyStateV1,
    TakerHealthRequestV1, TakerHealthV1, TakerMakerIdentityV1, TakerOfferListRequestV1,
    TakerOfferListV1, TakerOfferViewV1, taker_pair_capabilities_v1,
};

/// Maximum configured pinned Delivery sources accepted by one backend.
pub const MAX_TAKER_DELIVERY_SOURCES_V1: usize = 32;

/// Maximum unique authenticated offers one backend response may contain.
pub const MAX_TAKER_OFFER_RESULTS_V1: usize = 1_024;

/// Owner-injected source of trusted local Unix time.
pub trait TakerTrustedTimeSource {
    /// Returns current trusted Unix seconds, or `None` when time is unavailable.
    fn now_unix_seconds(&self) -> Option<u64>;
}

/// Owner-injected, payload-free health probe for the configured Chat boundary.
pub trait TakerDependencyProbe {
    /// Returns whether the configured dependency is currently available.
    fn is_available(&self) -> bool;
}

/// Fixed path-free failures from the read-only Taker facade backend.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum TakerBackendError {
    /// A request used a schema other than the exact supported version.
    #[error("unsupported Taker facade schema version")]
    UnsupportedSchemaVersion,
    /// A caller requested a pair/direction that has no role-fixed adapter.
    #[error("unsupported Taker facade route")]
    UnsupportedRoute,
    /// Trusted local time was unavailable for TTL filtering.
    #[error("trusted Taker time is unavailable")]
    TrustedTimeUnavailable,
    /// A configured pinned Delivery source failed closed.
    #[error("authenticated Taker Delivery is unavailable")]
    DeliveryUnavailable,
    /// The unique authenticated result cap would be exceeded.
    #[error("authenticated Taker offer limit exceeded")]
    OfferLimitExceeded,
    /// The same Maker and offer identity authenticated conflicting immutable facts.
    #[error("conflicting authenticated Taker offer")]
    ConflictingAuthenticatedOffer,
    /// Trusted backend configuration exceeded its conservative bounds.
    #[error("invalid Taker backend configuration")]
    InvalidConfiguration,
}

/// Read-only backend for health and authenticated offer discovery.
///
/// This type has no mutation dispatcher and cannot register RPC methods. A
/// transport adapter must expose only the concrete methods it actually wires.
pub struct TakerFacadeBackend<Clock, Chat> {
    delivery_sources: Vec<RunLocalDelivery>,
    clock: Clock,
    chat: Option<Chat>,
    maximum_offers: usize,
}

impl<Clock, Chat> fmt::Debug for TakerFacadeBackend<Clock, Chat> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TakerFacadeBackend")
            .field("delivery_source_count", &self.delivery_sources.len())
            .field("chat_configured", &self.chat.is_some())
            .field("maximum_offers", &self.maximum_offers)
            .finish_non_exhaustive()
    }
}
impl<Clock, Chat> TakerFacadeBackend<Clock, Chat>
where
    Clock: TakerTrustedTimeSource,
    Chat: TakerDependencyProbe,
{
    /// Builds one bounded backend from already validated private dependencies.
    ///
    /// # Errors
    ///
    /// Rejects zero or excessive offer caps and excessive Delivery sources.
    pub fn new(
        delivery_sources: Vec<RunLocalDelivery>,
        clock: Clock,
        chat: Option<Chat>,
        maximum_offers: usize,
    ) -> Result<Self, TakerBackendError> {
        if delivery_sources.len() > MAX_TAKER_DELIVERY_SOURCES_V1
            || maximum_offers == 0
            || maximum_offers > MAX_TAKER_OFFER_RESULTS_V1
        {
            return Err(TakerBackendError::InvalidConfiguration);
        }
        Ok(Self {
            delivery_sources,
            clock,
            chat,
            maximum_offers,
        })
    }

    /// Reports inspectable readiness and current read-only dependencies.
    ///
    /// A configured Delivery source is available only when every pinned source
    /// performs authenticated, TTL-aware discovery successfully. Dependency
    /// failures degrade health instead of disclosing their underlying details.
    ///
    /// # Errors
    ///
    /// Rejects an unsupported request schema or unavailable trusted time when
    /// at least one Delivery source must be checked.
    pub async fn health(
        &self,
        request: &TakerHealthRequestV1,
    ) -> Result<TakerHealthV1, TakerBackendError> {
        validate_schema(request.validate_schema_version())?;
        let delivery = if self.delivery_sources.is_empty() {
            TakerDependencyStateV1::Disabled
        } else {
            let now = self
                .clock
                .now_unix_seconds()
                .ok_or(TakerBackendError::TrustedTimeUnavailable)?;
            let query = DeliveryOfferQueryV1::all(now);
            let mut available = true;
            for source in &self.delivery_sources {
                if source.discover(&query).await.is_err() {
                    available = false;
                    break;
                }
            }
            if available {
                TakerDependencyStateV1::Available
            } else {
                TakerDependencyStateV1::Unavailable
            }
        };
        let chat = self
            .chat
            .as_ref()
            .map_or(TakerDependencyStateV1::Disabled, |probe| {
                if probe.is_available() {
                    TakerDependencyStateV1::Available
                } else {
                    TakerDependencyStateV1::Unavailable
                }
            });
        Ok(TakerHealthV1::new(true, delivery, chat))
    }

    /// Lists bounded, authenticated public offers from every pinned source.
    ///
    /// Exact duplicates collapse by Maker identity and offer ID. Conflicting
    /// duplicates fail closed. Results have deterministic identity/ID order.
    /// Raw signed envelopes remain private inside Delivery.
    ///
    /// # Errors
    ///
    /// Rejects an unsupported schema or route, unavailable trusted time or
    /// Delivery, conflicting immutable offers, or a unique-result overflow.
    pub async fn offer_list(
        &self,
        request: &TakerOfferListRequestV1,
    ) -> Result<TakerOfferListV1, TakerBackendError> {
        let now = self.trusted_now_for_offer_list(request)?;
        self.offer_list_at(request, now).await
    }

    /// Captures one trusted-time snapshot for a valid offer-list request.
    ///
    /// This lets an owner service reuse the exact discovery time for a subsequent
    /// admission decision instead of sampling time twice. Schema and route are
    /// checked before the injected clock is accessed.
    ///
    /// # Errors
    ///
    /// Rejects an unsupported schema or route, or unavailable trusted time.
    pub fn trusted_now_for_offer_list(
        &self,
        request: &TakerOfferListRequestV1,
    ) -> Result<u64, TakerBackendError> {
        validate_offer_list_request(*request)?;
        self.clock
            .now_unix_seconds()
            .ok_or(TakerBackendError::TrustedTimeUnavailable)
    }

    /// Lists bounded, authenticated public offers at one trusted-time snapshot.
    ///
    /// The caller is responsible for obtaining `now_unix_seconds` from a trusted
    /// owner-controlled source. This method does not access the backend clock.
    ///
    /// # Errors
    ///
    /// Rejects an unsupported schema or route, unavailable Delivery, conflicting
    /// immutable offers, or a unique-result overflow.
    pub async fn offer_list_at(
        &self,
        request: &TakerOfferListRequestV1,
        now_unix_seconds: u64,
    ) -> Result<TakerOfferListV1, TakerBackendError> {
        validate_offer_list_request(*request)?;
        let query = request.route.map_or_else(
            || DeliveryOfferQueryV1::all(now_unix_seconds),
            |route| DeliveryOfferQueryV1::for_route(route, now_unix_seconds),
        );
        let mut unique = BTreeMap::<([u8; 33], Box<str>), TakerOfferViewV1>::new();
        for source in &self.delivery_sources {
            let discovered = source
                .discover(&query)
                .await
                .map_err(|_| TakerBackendError::DeliveryUnavailable)?;
            for authenticated in discovered {
                let maker_bytes = *authenticated.maker_identity();
                let maker_identity = TakerMakerIdentityV1::new(maker_bytes)
                    .map_err(|_| TakerBackendError::DeliveryUnavailable)?;
                let view = TakerOfferViewV1 {
                    offer: authenticated.offer().clone(),
                    maker_identity,
                    signed_envelope_sha256: authenticated.commitment(),
                };
                let key = (maker_bytes, view.offer.id().as_str().into());
                match unique.get(&key) {
                    Some(existing) if existing == &view => {}
                    Some(_) => return Err(TakerBackendError::ConflictingAuthenticatedOffer),
                    None if unique.len() >= self.maximum_offers => {
                        return Err(TakerBackendError::OfferLimitExceeded);
                    }
                    None => {
                        unique.insert(key, view);
                    }
                }
            }
        }
        Ok(TakerOfferListV1 {
            schema_version: TAKER_FACADE_SCHEMA_VERSION_V1,
            offers: unique.into_values().collect(),
        })
    }
}

fn supported_route(route: MakerRouteV1) -> bool {
    taker_pair_capabilities_v1().iter().any(|capability| {
        capability.pair() == route.pair()
            && capability.supported_direction() == route.direction()
            && capability.authenticated_offer_browsing()
    })
}

fn validate_offer_list_request(request: TakerOfferListRequestV1) -> Result<(), TakerBackendError> {
    validate_schema(request.validate_schema_version())?;
    if request.route.is_some_and(|route| !supported_route(route)) {
        return Err(TakerBackendError::UnsupportedRoute);
    }
    Ok(())
}

fn validate_schema<T>(result: Result<(), T>) -> Result<(), TakerBackendError> {
    result.map_err(|_| TakerBackendError::UnsupportedSchemaVersion)
}
