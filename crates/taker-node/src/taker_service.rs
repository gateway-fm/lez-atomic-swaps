//! JSON-RPC composition for the owner-local Taker Node.
//!
//! Health and authenticated offer discovery are always registered. The
//! prepared-route lifecycle lives in [`lifecycle`]: Bitcoin swaps always,
//! Zcash swaps with the `pair-zec` feature.

mod lifecycle;

use std::sync::{Arc, Mutex};

use jsonrpsee::{RpcModule, core::RegisterMethodError, types::ErrorObjectOwned};
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
}

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
    let (backend, initiation) = context.into_parts();
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
