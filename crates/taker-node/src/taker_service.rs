//! JSON-RPC composition for the owner-local Taker Node.
//!
//! Health and authenticated offer discovery are always registered. The
//! prepared-route lifecycle lives in [`lifecycle`]: Bitcoin swaps always,
//! Zcash swaps with the `pair-zec` feature.

mod btc_dynamic;
mod lifecycle;
#[cfg(feature = "pair-zec")]
mod zec_supervisor;

pub(crate) use btc_dynamic::DynamicBtcRole;

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use jsonrpsee::{RpcModule, core::RegisterMethodError, types::ErrorObjectOwned};
use lez_actor_supervision::LastOutcomes;
use lez_btc_role_lifecycle::{DEFAULT_LEZ_FINALITY_STALL_SECONDS, LezFinalityWatch};
use lez_swap_core::Pair;
use serde_json::json;

use crate::{
    ConfiguredTakerFacadeBackend, ConfiguredTakerInitiationContext, ConfiguredTakerServiceContext,
    TakerBackendError, TakerHealthRequestV1, TakerOfferListRequestV1,
};

const INVALID_PARAMS_CODE: i32 = -32_602;
const INTERNAL_ERROR_CODE: i32 = -32_603;
const DEPENDENCY_UNAVAILABLE_CODE: i32 = -32_010;
const RESULT_LIMIT_EXCEEDED_CODE: i32 = -32_011;
const AUTHENTICATED_OFFER_CONFLICT_CODE: i32 = -32_012;

struct TakerServiceState {
    backend: ConfiguredTakerFacadeBackend,
    initiation: Option<Arc<Mutex<ConfiguredTakerInitiationContext>>>,
    /// Which pairs the prepared catalog holds, fixed at startup.
    btc_registered: bool,
    zec_registered: bool,
    /// How long the Bitcoin observer waits between two sweeps.
    observer_sweep_interval: Duration,
    /// The LEZ chain's progress for `taker_health`, from the Node-owned
    /// lifecycle's indexer and sequencer.
    lez_finality: Option<LezFinalityWatch>,
    /// What each swap's actor last did or waits for, from the observer's,
    /// the Zcash supervisor's and the owner's attempts (`last_outcome`).
    last_outcomes: LastOutcomes,
}

/// The Taker observer's production pause between two sweeps of the Node-owned
/// Bitcoin swaps: 15 s.
pub const DEFAULT_OBSERVER_SWEEP_INTERVAL: Duration = Duration::from_secs(15);

/// Builds the exact JSON-RPC module enabled by one validated service context.
///
/// Health and authenticated offer listing are always registered. A validated
/// prepared catalog plus existing registry registers initiation, receipt-bound reads,
/// and generation-fenced claim/refund methods that invoke only the role actor.
///
/// # Errors
///
/// Returns an error if one of the fixed method names cannot be registered.
pub fn taker_service_rpc_module(
    context: ConfiguredTakerServiceContext,
) -> Result<RpcModule<()>, RegisterMethodError> {
    taker_service_rpc_module_with_sweep(context, DEFAULT_OBSERVER_SWEEP_INTERVAL)
}

/// As [`taker_service_rpc_module`], with the Bitcoin observer sweeping every
/// `observer_sweep_interval` instead of [`DEFAULT_OBSERVER_SWEEP_INTERVAL`]
/// (`--observer-sweep-seconds`; a devnet's timing profiles sweep sooner).
///
/// # Errors
///
/// Returns an error if one of the fixed method names cannot be registered.
pub fn taker_service_rpc_module_with_sweep(
    context: ConfiguredTakerServiceContext,
    observer_sweep_interval: Duration,
) -> Result<RpcModule<()>, RegisterMethodError> {
    taker_service_rpc_module_with_timing(
        context,
        observer_sweep_interval,
        DEFAULT_LEZ_FINALITY_STALL_SECONDS,
    )
}

/// As [`taker_service_rpc_module_with_sweep`], with `taker_health` reporting
/// `lez_finality_stalled` once the finalized LEZ block id has stood still for
/// `lez_finality_stall_seconds` while the sequencer's tip moved
/// (`--lez-finality-stall-seconds`; the devnet passes its heartbeat's
/// threshold). The chain is read only with the Node-owned lifecycle.
///
/// # Errors
///
/// Returns an error if one of the fixed method names cannot be registered.
pub fn taker_service_rpc_module_with_timing(
    context: ConfiguredTakerServiceContext,
    observer_sweep_interval: Duration,
    lez_finality_stall_seconds: u64,
) -> Result<RpcModule<()>, RegisterMethodError> {
    let (backend, initiation) = context.into_parts();
    let lez_finality = initiation
        .as_ref()
        .and_then(ConfiguredTakerInitiationContext::dynamic_btc)
        .map(|dynamic| dynamic.lez_finality_watch(lez_finality_stall_seconds));
    let btc_registered = initiation
        .as_ref()
        .is_some_and(|context| context.has_pair(Pair::Bitcoin));
    let zec_registered = initiation
        .as_ref()
        .is_some_and(|context| context.has_pair(Pair::Zcash));
    let state = Arc::new(TakerServiceState {
        backend,
        initiation: initiation.map(|value| Arc::new(Mutex::new(value))),
        btc_registered,
        zec_registered,
        observer_sweep_interval,
        lez_finality,
        last_outcomes: LastOutcomes::new(),
    });
    let mut module = RpcModule::new(());

    let health_state = Arc::clone(&state);
    module.register_async_method("taker_health", move |params, _, _| {
        let state = Arc::clone(&health_state);
        async move {
            let request: TakerHealthRequestV1 = params
                .one()
                .map_err(|_| rpc_error(INVALID_PARAMS_CODE, "Invalid params", "invalid_params"))?;
            let health = state
                .backend
                .health(&request)
                .await
                .map_err(map_backend_error)?;
            let mut health = health;
            if state.zec_registered {
                health = health.with_zec_lifecycle_registered();
            }
            if state.btc_registered {
                health = health.with_btc_lifecycle_registered();
            }
            // The LEZ chain's progress (#112); absent without the lifecycle.
            if let Some(watch) = state.lez_finality.as_ref() {
                health = health.with_lez_chain(Some(watch.progress().await));
            }
            Ok::<_, ErrorObjectOwned>(health)
        }
    })?;

    let offer_state = Arc::clone(&state);
    module.register_async_method("taker_offer_list_v1", move |params, _, _| {
        let state = Arc::clone(&offer_state);
        async move {
            let request: TakerOfferListRequestV1 = params
                .one()
                .map_err(|_| rpc_error(INVALID_PARAMS_CODE, "Invalid params", "invalid_params"))?;
            state
                .backend
                .offer_list(&request)
                .await
                .map_err(map_backend_error)
        }
    })?;

    lifecycle::register(&mut module, &state)?;

    Ok(module)
}

fn map_backend_error(error: TakerBackendError) -> ErrorObjectOwned {
    match error {
        TakerBackendError::UnsupportedSchemaVersion => rpc_error(
            INVALID_PARAMS_CODE,
            "Invalid params",
            "unsupported_schema_version",
        ),
        TakerBackendError::UnsupportedRoute => {
            rpc_error(INVALID_PARAMS_CODE, "Invalid params", "unsupported_route")
        }
        TakerBackendError::TrustedTimeUnavailable => rpc_error(
            DEPENDENCY_UNAVAILABLE_CODE,
            "Taker dependency unavailable",
            "trusted_time_unavailable",
        ),
        TakerBackendError::DeliveryUnavailable => rpc_error(
            DEPENDENCY_UNAVAILABLE_CODE,
            "Taker dependency unavailable",
            "authenticated_delivery_unavailable",
        ),
        TakerBackendError::OfferLimitExceeded => rpc_error(
            RESULT_LIMIT_EXCEEDED_CODE,
            "Taker result limit exceeded",
            "offer_limit_exceeded",
        ),
        TakerBackendError::ConflictingAuthenticatedOffer => rpc_error(
            AUTHENTICATED_OFFER_CONFLICT_CODE,
            "Authenticated offer conflict",
            "conflicting_authenticated_offer",
        ),
        TakerBackendError::InvalidConfiguration => rpc_error(
            INTERNAL_ERROR_CODE,
            "Internal error",
            "invalid_backend_configuration",
        ),
    }
}

fn rpc_error(code: i32, message: &'static str, category: &'static str) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(code, message, Some(json!({ "category": category })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_failures_map_to_fixed_codes_and_categories_without_private_details() {
        let expectations = [
            (
                TakerBackendError::UnsupportedSchemaVersion,
                INVALID_PARAMS_CODE,
                "unsupported_schema_version",
            ),
            (
                TakerBackendError::UnsupportedRoute,
                INVALID_PARAMS_CODE,
                "unsupported_route",
            ),
            (
                TakerBackendError::TrustedTimeUnavailable,
                DEPENDENCY_UNAVAILABLE_CODE,
                "trusted_time_unavailable",
            ),
            (
                TakerBackendError::DeliveryUnavailable,
                DEPENDENCY_UNAVAILABLE_CODE,
                "authenticated_delivery_unavailable",
            ),
            (
                TakerBackendError::OfferLimitExceeded,
                RESULT_LIMIT_EXCEEDED_CODE,
                "offer_limit_exceeded",
            ),
            (
                TakerBackendError::ConflictingAuthenticatedOffer,
                AUTHENTICATED_OFFER_CONFLICT_CODE,
                "conflicting_authenticated_offer",
            ),
            (
                TakerBackendError::InvalidConfiguration,
                INTERNAL_ERROR_CODE,
                "invalid_backend_configuration",
            ),
        ];
        for (error, code, category) in expectations {
            let mapped = map_backend_error(error);
            assert_eq!(mapped.code(), code, "{error}");
            let data = mapped
                .data()
                .map_or_else(String::new, |raw| raw.get().to_owned());
            assert_eq!(data, format!("{{\"category\":\"{category}\"}}"), "{error}");
            let wire = format!("{} {data}", mapped.message()).to_ascii_lowercase();
            for forbidden in ["/", "path", "file", "socket", "endpoint", "credential"] {
                assert!(!wire.contains(forbidden), "{error}: {wire}");
            }
        }
    }
}
