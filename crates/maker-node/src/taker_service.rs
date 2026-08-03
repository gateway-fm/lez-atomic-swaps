//! JSON-RPC composition for the owner-local Taker service.

use std::{
    fs,
    sync::{Arc, Mutex},
};

use jsonrpsee::{RpcModule, core::RegisterMethodError, types::ErrorObjectOwned};
use lez_bridge_protocol::RequestId;
use lez_swap_store::{TakerFacadeStoreError, TakerInitiationAdmissionV1, TakerInitiationFactsV1};
use secp256k1::PublicKey;
use serde_json::json;
use sha2::{Digest as _, Sha256};
use zec_reference_actor::{ActorConfig, ActorRole};

use crate::{
    ConfiguredTakerFacadeBackend, ConfiguredTakerInitiationContext, ConfiguredTakerServiceContext,
    PreparedZecTakerInitiationV1, TakerBackendError, TakerHealthRequestV1, TakerInitiationCommitV1,
    TakerOfferListRequestV1, TakerSwapInitiateRequestV1, TakerSwapStateV1, TakerSwapViewV1,
    secure_file::read_private_file_snapshot,
    zec_taker_accept::{ZecTakeInput, take_zec_with_authenticated_offer_and_actor_config},
};

const INVALID_PARAMS_CODE: i32 = -32_602;
const INTERNAL_ERROR_CODE: i32 = -32_603;
const DEPENDENCY_UNAVAILABLE_CODE: i32 = -32_010;
const RESULT_LIMIT_EXCEEDED_CODE: i32 = -32_011;
const AUTHENTICATED_OFFER_CONFLICT_CODE: i32 = -32_012;
const INITIATION_CONFLICT_CODE: i32 = -32_013;
const MAXIMUM_PREPARED_INPUT_BYTES: u64 = 256 * 1024;
const SIGNING_KEY_BYTES: u64 = 32;

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
    ExecutionUnavailable,
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
    let replay = lookup_replay(&initiation, &request.request_id).await?;
    if let Some((facts, admitted_at)) = replay {
        if !request_matches_facts(&request, &facts) {
            return Err(InitiationError::Conflict);
        }
        let (prepared, execution_enabled, receipt_present) =
            prepared_execution_for_offer(&initiation, &request.offer_id).await?;
        verify_replay_authority(
            &initiation,
            &request.request_id,
            &facts,
            &prepared,
            admitted_at,
        )
        .await?;
        let state = if execution_enabled || receipt_present {
            execute_prepared_zec(&prepared, admitted_at).await?;
            TakerSwapStateV1::NotActivated
        } else {
            TakerSwapStateV1::Initiating
        };
        return Ok(commit_from_facts(&facts, true, state));
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
    let (admission, execution_enabled) = tokio::task::spawn_blocking(move || {
        let mut context = admission_context
            .lock()
            .map_err(|_| InitiationError::Internal)?;
        let execution_enabled = context.execution_enabled();
        let admission = context
            .registry_mut()
            .admit_initiation(&request_id, &facts, &authority, now)
            .map_err(map_store_error)?;
        Ok::<_, InitiationError>((admission, execution_enabled))
    })
    .await
    .map_err(|_| InitiationError::Internal)??;
    let progress = if execution_enabled {
        execute_prepared_zec(&prepared, now).await?;
        TakerSwapStateV1::NotActivated
    } else {
        TakerSwapStateV1::Initiating
    };
    Ok(commit_from_admission(&admission, progress))
}

async fn lookup_replay(
    initiation: &Arc<Mutex<ConfiguredTakerInitiationContext>>,
    request_id: &RequestId,
) -> Result<Option<(TakerInitiationFactsV1, u64)>, InitiationError> {
    let context = Arc::clone(initiation);
    let request_id = request_id.clone();
    tokio::task::spawn_blocking(move || {
        let mut context = context.lock().map_err(|_| InitiationError::Internal)?;
        let registry = context.registry_mut();
        let facts = registry
            .lookup_initiation(&request_id)
            .map_err(map_store_error)?;
        let admitted_at = facts
            .as_ref()
            .map(|_| {
                registry
                    .lookup_initiation_admitted_at(&request_id)
                    .map_err(map_store_error)?
                    .ok_or(InitiationError::Internal)
            })
            .transpose()?;
        Ok::<_, InitiationError>(facts.zip(admitted_at))
    })
    .await
    .map_err(|_| InitiationError::Internal)?
}

async fn verify_replay_authority(
    initiation: &Arc<Mutex<ConfiguredTakerInitiationContext>>,
    request_id: &RequestId,
    facts: &TakerInitiationFactsV1,
    prepared: &PreparedZecTakerInitiationV1,
    admitted_at: u64,
) -> Result<(), InitiationError> {
    if prepared.facts() != facts {
        return Err(InitiationError::Conflict);
    }
    let context = Arc::clone(initiation);
    let request_id = request_id.clone();
    let facts = facts.clone();
    let authority = prepared.authority().clone();
    tokio::task::spawn_blocking(move || {
        let mut context = context.lock().map_err(|_| InitiationError::Internal)?;
        let replay = context
            .registry_mut()
            .admit_initiation(&request_id, &facts, &authority, admitted_at)
            .map_err(map_store_error)?;
        if !replay.was_replay() || replay.facts() != &facts {
            return Err(InitiationError::Internal);
        }
        Ok(())
    })
    .await
    .map_err(|_| InitiationError::Internal)?
}

async fn prepared_execution_for_offer(
    initiation: &Arc<Mutex<ConfiguredTakerInitiationContext>>,
    offer_id: &lez_swap_store::MakerOfferId,
) -> Result<(PreparedZecTakerInitiationV1, bool, bool), InitiationError> {
    let context = Arc::clone(initiation);
    let offer_id = offer_id.clone();
    tokio::task::spawn_blocking(move || {
        let context = context.lock().map_err(|_| InitiationError::Internal)?;
        let execution_enabled = context.execution_enabled();
        let prepared = context
            .prepared_zec_for_offer(&offer_id)
            .cloned()
            .ok_or(InitiationError::SelectionMismatch)?;
        let receipt_present = match fs::symlink_metadata(prepared.execution().receipt_output()) {
            Ok(_) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(_) => return Err(InitiationError::ExecutionUnavailable),
        };
        Ok((prepared, execution_enabled, receipt_present))
    })
    .await
    .map_err(|_| InitiationError::Internal)?
}

async fn execute_prepared_zec(
    prepared: &PreparedZecTakerInitiationV1,
    admitted_at: u64,
) -> Result<(), InitiationError> {
    let execution = prepared.execution();
    let draft = read_private_file_snapshot(
        execution.unsigned_draft_path(),
        MAXIMUM_PREPARED_INPUT_BYTES,
        "prepared Taker unsigned draft",
    )
    .map_err(|_| InitiationError::ExecutionUnavailable)?;
    if draft.bytes() != execution.unsigned_draft()
        || Sha256::digest(draft.bytes()).as_slice() != execution.unsigned_draft_sha256()
    {
        return Err(InitiationError::ExecutionUnavailable);
    }
    let signing_key = read_private_file_snapshot(
        execution.signing_key_path(),
        SIGNING_KEY_BYTES,
        "prepared Taker signing key",
    )
    .map_err(|_| InitiationError::ExecutionUnavailable)?;
    if signing_key.bytes() != execution.signing_key() {
        return Err(InitiationError::ExecutionUnavailable);
    }
    let actor = ActorConfig::load_private_pinned_sha256(
        execution.source_config_path(),
        execution.source_config_sha256(),
    )
    .map_err(|_| InitiationError::ExecutionUnavailable)?;
    if actor.role() != ActorRole::Taker || actor.swap_id() != prepared.swap_id() {
        return Err(InitiationError::ExecutionUnavailable);
    }
    let maker = PublicKey::from_slice(prepared.maker_identity())
        .map_err(|_| InitiationError::ExecutionUnavailable)?;
    take_zec_with_authenticated_offer_and_actor_config(
        ZecTakeInput {
            delivery: None,
            expected_maker: &maker,
            now_unix_seconds: admitted_at,
            offer_id: prepared.offer_id().as_str(),
            chat_socket: execution.chat_socket(),
            reservation_id: prepared.reservation_id().as_str(),
            foreign_units: prepared.facts().foreign_units(),
            unsigned_draft_file: execution.unsigned_draft_path(),
            source_taker_config_file: execution.source_config_path(),
            taker_actor_root: execution.actor_root(),
            acceptance_receipt_file: execution.receipt_output(),
            taker_signing_key_file: execution.signing_key_path(),
            agreement_output_file: execution.agreement_output(),
        },
        execution.authenticated_offer(),
        &actor,
    )
    .await
    .map_err(|_| InitiationError::ExecutionUnavailable)?;
    Ok(())
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

fn commit_from_admission(
    admission: &TakerInitiationAdmissionV1,
    state: TakerSwapStateV1,
) -> TakerInitiationCommitV1 {
    commit_from_facts(admission.facts(), admission.was_replay(), state)
}

fn commit_from_facts(
    facts: &TakerInitiationFactsV1,
    was_replay: bool,
    state: TakerSwapStateV1,
) -> TakerInitiationCommitV1 {
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
            state,
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
        InitiationError::ExecutionUnavailable => rpc_error(
            DEPENDENCY_UNAVAILABLE_CODE,
            "Taker dependency unavailable",
            "zec_acceptance_unavailable",
        ),
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
