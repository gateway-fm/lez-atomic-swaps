//! Explicit JSON-RPC adapter for the implemented read-only Taker facade.

use jsonrpsee::{RpcModule, core::RegisterMethodError, types::ErrorObjectOwned};
use serde_json::json;

use crate::{
    TakerBackendError, TakerDependencyProbe, TakerFacadeBackend, TakerHealthRequestV1,
    TakerOfferListRequestV1, TakerTrustedTimeSource,
};

const INVALID_PARAMS_CODE: i32 = -32_602;
const INTERNAL_ERROR_CODE: i32 = -32_603;
const DEPENDENCY_UNAVAILABLE_CODE: i32 = -32_010;
const RESULT_LIMIT_EXCEEDED_CODE: i32 = -32_011;
const AUTHENTICATED_OFFER_CONFLICT_CODE: i32 = -32_012;

/// Builds the exact JSON-RPC module implemented by the read-only Taker backend.
///
/// Only `taker_health` and `taker_offer_list_v1` are registered. Mutation,
/// lifecycle, generic dispatch, and placeholder methods are deliberately absent.
///
/// # Errors
///
/// Returns an error if either fixed method name cannot be registered.
pub fn taker_read_only_rpc_module<Clock, Chat>(
    backend: TakerFacadeBackend<Clock, Chat>,
) -> Result<RpcModule<TakerFacadeBackend<Clock, Chat>>, RegisterMethodError>
where
    Clock: TakerTrustedTimeSource + Send + Sync + 'static,
    Chat: TakerDependencyProbe + Send + Sync + 'static,
{
    let mut module = RpcModule::new(backend);
    module.register_async_method("taker_health", |params, backend, _| async move {
        let request: TakerHealthRequestV1 = params
            .one()
            .map_err(|_| rpc_error(INVALID_PARAMS_CODE, "Invalid params", "invalid_params"))?;
        backend.health(&request).await.map_err(map_backend_error)
    })?;
    module.register_async_method("taker_offer_list_v1", |params, backend, _| async move {
        let request: TakerOfferListRequestV1 = params
            .one()
            .map_err(|_| rpc_error(INVALID_PARAMS_CODE, "Invalid params", "invalid_params"))?;
        backend
            .offer_list(&request)
            .await
            .map_err(map_backend_error)
    })?;
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
