//! JSON-RPC composition for the owner-local Taker service.

use std::sync::{Arc, Mutex};

use jsonrpsee::{RpcModule, core::RegisterMethodError, types::ErrorObjectOwned};
use lez_swap_store::{TakerFacadeStoreError, TakerInitiationAdmissionV1, TakerInitiationFactsV1};
use serde_json::json;

use crate::{
    ConfiguredTakerFacadeBackend, ConfiguredTakerInitiationContext, ConfiguredTakerServiceContext,
    TakerBackendError, TakerHealthRequestV1, TakerInitiationCommitV1, TakerOfferListRequestV1,
    TakerSwapInitiateRequestV1, TakerSwapStateV1, TakerSwapViewV1,
};

const INVALID_PARAMS_CODE: i32 = -32_602;
const INTERNAL_ERROR_CODE: i32 = -32_603;
const DEPENDENCY_UNAVAILABLE_CODE: i32 = -32_010;
const RESULT_LIMIT_EXCEEDED_CODE: i32 = -32_011;
const AUTHENTICATED_OFFER_CONFLICT_CODE: i32 = -32_012;
const INITIATION_CONFLICT_CODE: i32 = -32_013;

struct TakerServiceState {
    backend: ConfiguredTakerFacadeBackend,
    initiation: Option<Arc<Mutex<ConfiguredTakerInitiationContext>>>,
}

/// Builds the exact JSON-RPC module enabled by one validated service context.
///
/// Health and authenticated offer listing are always registered. Initiation is
/// registered only when the owner supplied a validated prepared authority and
/// existing registry; no lifecycle or terminal-effect method is implied.
///
/// # Errors
///
/// Returns an error if one of the fixed method names cannot be registered.
pub fn taker_service_rpc_module(
    context: ConfiguredTakerServiceContext,
) -> Result<RpcModule<()>, RegisterMethodError> {
    let (backend, initiation) = context.into_parts();
    let initiation = initiation.map(|value| Arc::new(Mutex::new(value)));
    let state = Arc::new(TakerServiceState {
        backend,
        initiation,
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
            Ok::<_, ErrorObjectOwned>(if state.initiation.is_some() {
                health.with_initiation_registered()
            } else {
                health
            })
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

    if state.initiation.is_some() {
        let initiate_state = Arc::clone(&state);
        module.register_async_method("taker_swap_initiate_v1", move |params, _, _| {
            let state = Arc::clone(&initiate_state);
            async move {
                let request: TakerSwapInitiateRequestV1 = params.one().map_err(|_| {
                    rpc_error(INVALID_PARAMS_CODE, "Invalid params", "invalid_params")
                })?;
                initiate(state, request).await.map_err(map_initiation_error)
            }
        })?;
    }

    Ok(module)
}

#[derive(Clone, Copy, Debug)]
enum InitiationError {
    UnsupportedSchemaVersion,
    SelectionMismatch,
    Conflict,
    Backend(TakerBackendError),
    Internal,
}

async fn initiate(
    state: Arc<TakerServiceState>,
    request: TakerSwapInitiateRequestV1,
) -> Result<TakerInitiationCommitV1, InitiationError> {
    request
        .validate_schema_version()
        .map_err(|_| InitiationError::UnsupportedSchemaVersion)?;
    let initiation = state.initiation.clone().ok_or(InitiationError::Internal)?;

    // Durable lookup deliberately precedes catalog selection, time, and Delivery.
    let replay_id = request.request_id.clone();
    let replay_context = Arc::clone(&initiation);
    let replay = tokio::task::spawn_blocking(move || {
        replay_context
            .lock()
            .map_err(|_| InitiationError::Internal)?
            .registry_mut()
            .lookup_initiation(&replay_id)
            .map_err(map_store_error)
    })
    .await
    .map_err(|_| InitiationError::Internal)??;
    if let Some(facts) = replay {
        if !request_matches_facts(&request, &facts) {
            return Err(InitiationError::Conflict);
        }
        return Ok(commit_from_facts(&facts, true));
    }

    // Clone private authority under the synchronous mutex, then release it
    // before authenticated Delivery performs asynchronous I/O.
    let offer_id = request.offer_id.clone();
    let selection_context = Arc::clone(&initiation);
    let prepared = tokio::task::spawn_blocking(move || {
        selection_context
            .lock()
            .map_err(|_| InitiationError::Internal)?
            .prepared_zec_for_offer(&offer_id)
            .cloned()
            .ok_or(InitiationError::SelectionMismatch)
    })
    .await
    .map_err(|_| InitiationError::Internal)??;
    if !request_matches_facts(&request, prepared.facts()) {
        return Err(InitiationError::SelectionMismatch);
    }

    let offer_request = TakerOfferListRequestV1 {
        schema_version: 1,
        route: Some(request.route),
    };
    let now = state
        .backend
        .trusted_now_for_offer_list(&offer_request)
        .map_err(InitiationError::Backend)?;
    let offers = state
        .backend
        .offer_list_at(&offer_request, now)
        .await
        .map_err(InitiationError::Backend)?;
    let live_match = offers.offers.iter().any(|candidate| {
        candidate.offer.id() == &request.offer_id
            && candidate.offer.route() == request.route
            && candidate.maker_identity == request.maker_identity
            && candidate.signed_envelope_sha256 == request.signed_envelope_sha256
            && candidate
                .offer
                .quote_foreign_amount(request.foreign_units)
                .ok()
                == Some(request.expected_lez_units)
    });
    if !live_match {
        return Err(InitiationError::SelectionMismatch);
    }

    let request_id = request.request_id;
    let facts = prepared.facts().clone();
    let authority = prepared.authority().clone();
    let admission_context = Arc::clone(&initiation);
    let admission = tokio::task::spawn_blocking(move || {
        admission_context
            .lock()
            .map_err(|_| InitiationError::Internal)?
            .registry_mut()
            .admit_initiation(&request_id, &facts, &authority, now)
            .map_err(map_store_error)
    })
    .await
    .map_err(|_| InitiationError::Internal)??;
    Ok(commit_from_admission(&admission))
}

fn request_matches_facts(
    request: &TakerSwapInitiateRequestV1,
    facts: &TakerInitiationFactsV1,
) -> bool {
    facts.offer_id() == &request.offer_id
        && facts.route() == request.route
        && facts.maker_identity() == request.maker_identity.as_bytes()
        && facts.signed_envelope_sha256() == &request.signed_envelope_sha256
        && facts.foreign_units() == request.foreign_units
        && facts.lez_units() == request.expected_lez_units
}

fn commit_from_admission(admission: &TakerInitiationAdmissionV1) -> TakerInitiationCommitV1 {
    commit_from_facts(admission.facts(), admission.was_replay())
}

fn commit_from_facts(facts: &TakerInitiationFactsV1, was_replay: bool) -> TakerInitiationCommitV1 {
    TakerInitiationCommitV1 {
        schema_version: 1,
        swap: TakerSwapViewV1 {
            schema_version: 1,
            swap_id: facts.swap_id().clone(),
            offer_id: facts.offer_id().clone(),
            route: facts.route(),
            foreign_units: facts.foreign_units(),
            lez_units: facts.lez_units(),
            progress_generation: 0,
            state: TakerSwapStateV1::Initiating,
            available_action: None,
            privacy_guidance: None,
        },
        was_replay,
    }
}

fn map_store_error(error: TakerFacadeStoreError) -> InitiationError {
    match error {
        TakerFacadeStoreError::RequestConflict | TakerFacadeStoreError::SwapConflict => {
            InitiationError::Conflict
        }
        TakerFacadeStoreError::DatabaseUnavailable
        | TakerFacadeStoreError::UnsafeDatabaseFile
        | TakerFacadeStoreError::DatabaseAlreadyExists
        | TakerFacadeStoreError::ForeignSchema
        | TakerFacadeStoreError::FutureSchema
        | TakerFacadeStoreError::CorruptState
        | TakerFacadeStoreError::InvalidInput
        | TakerFacadeStoreError::StorageUnavailable => InitiationError::Internal,
    }
}

fn map_initiation_error(error: InitiationError) -> ErrorObjectOwned {
    match error {
        InitiationError::UnsupportedSchemaVersion => rpc_error(
            INVALID_PARAMS_CODE,
            "Invalid params",
            "unsupported_schema_version",
        ),
        InitiationError::SelectionMismatch => rpc_error(
            INVALID_PARAMS_CODE,
            "Invalid params",
            "initiation_selection_mismatch",
        ),
        InitiationError::Conflict => rpc_error(
            INITIATION_CONFLICT_CODE,
            "Taker initiation conflict",
            "initiation_conflict",
        ),
        InitiationError::Backend(error) => map_backend_error(error),
        InitiationError::Internal => rpc_error(
            INTERNAL_ERROR_CODE,
            "Internal error",
            "initiation_registry_unavailable",
        ),
    }
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
