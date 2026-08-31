//! Authenticated local JSON-RPC boundary for the role-fixed Taker Node.

mod taker_backend;
mod taker_facade;
mod taker_rpc;
mod taker_service;
mod taker_service_config;

#[doc(hidden)]
pub mod zec_taker_accept;

pub use lez_node_common::*;
pub use taker_backend::{
    MAX_TAKER_DELIVERY_SOURCES_V1, MAX_TAKER_OFFER_RESULTS_V1, TakerBackendError,
    TakerDependencyProbe, TakerFacadeBackend, TakerTrustedTimeSource,
};
pub use taker_facade::{
    TAKER_FACADE_METHODS_V1, TAKER_FACADE_SCHEMA_VERSION_V1, TakerActionCommitV1,
    TakerClaimRequestV1, TakerDependencyStateV1, TakerFacadeSchemaVersionError,
    TakerHealthRequestV1, TakerHealthV1, TakerInitiationCapabilityV1, TakerInitiationCommitV1,
    TakerMonitoringCapabilityV1, TakerOfferListRequestV1, TakerOfferListV1, TakerOfferViewV1,
    TakerPairCapabilityV1, TakerPrivacyGuidanceV1, TakerRefundRequestV1, TakerRegisteredMethodsV1,
    TakerSwapInitiateRequestV1, TakerSwapListRequestV1, TakerSwapListV1, TakerSwapMonitorRequestV1,
    TakerSwapStateV1, TakerSwapViewV1, TakerTerminalActionCapabilityV1, TakerTerminalActionV1,
    taker_pair_capabilities_v1,
};
pub use taker_rpc::taker_read_only_rpc_module;
pub use taker_service::taker_service_rpc_module;
pub use taker_service_config::{
    ConfiguredTakerFacadeBackend, ConfiguredTakerInitiationContext, ConfiguredTakerServiceContext,
    OwnerChatSocketProbe, PreparedZecExecutionV1, PreparedZecTakerInitiationV1,
    SystemTakerTrustedTime, TakerServiceStartupError, load_taker_service_backend,
    load_taker_service_context,
};
