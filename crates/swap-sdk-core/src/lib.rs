//! Adapter-independent contracts shared by the dedicated LEZ swap SDKs.
//!
//! This crate defines vocabulary, not a coordinator. Pair crates keep concrete
//! terms, chain evidence, transaction templates, and recovery actions while
//! implementing the synchronous [`SwapProtocol`] lifecycle. Applications may
//! use [`OfferDiscovery`] and [`NegotiationChannel`] before a lock is published;
//! neither port is part of the post-lock protocol engine.
//!
//! The public-effect vocabulary records complete public transaction bytes and
//! their expected chain identities in a stable order. Durability, submission,
//! and observation remain responsibilities of a recovery store and chain
//! adapter outside this crate.
//!
//! # Type-state example
//!
//! ```
//! use lez_swap_sdk_core::{ClaimLeg, ClaimOrder};
//!
//! let order = ClaimOrder::new(ClaimLeg::Lez, ClaimLeg::Foreign)
//!     .expect("the two claim legs are distinct");
//! assert_eq!(order.revealing(), ClaimLeg::Lez);
//! assert_eq!(order.followup(), ClaimLeg::Foreign);
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod error;
mod lifecycle;
mod ports;
mod public_effect;
mod version;

pub use error::{ErrorCategory, ErrorDisposition, ProtocolError};
pub use lifecycle::{ClaimLeg, ClaimOrder, ClaimOrderError, SwapProtocol};
pub use ports::{NegotiationChannel, OfferDiscovery};
pub use public_effect::{
    EXACT_PUBLIC_EFFECT_PLAN_SCHEMA_V1, ExactPublicEffectBytes, ExactPublicEffectPlanV1,
    ExpectedPublicEffectId, PublicEffectPlanError, PublicEffectStepId, PublicEffectStepV1,
};
pub use version::{ProtocolVersion, SchemaVersion, VersionError};

pub use lez_swap_core::{Pair, Participant, SwapDirection, SwapId};
