//! JSON-RPC composition for the owner-local Taker service.

use std::{
    fs,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use jsonrpsee::{RpcModule, core::RegisterMethodError, types::ErrorObjectOwned};
use lez_bridge_protocol::RequestId;
use lez_swap_core::{Phase, SwapId};
use lez_swap_store::{
    MakerActorHeldLock, TakerActionAdmissionV1, TakerFacadeActionV1, TakerFacadeStoreError,
    TakerInitiationAdmissionV1, TakerInitiationFactsV1,
};
use lez_zec_swap_sdk::ZecLifecycleAction;
use secp256k1::PublicKey;
use serde_json::json;
use sha2::{Digest as _, Sha256};
use zec_reference_actor::{
    ActorCommand, ActorCommandOutputV1, ActorConfig, ActorRole, ActorStatusProjectionV1,
    execute_actor_command,
};

use crate::{
    ConfiguredTakerFacadeBackend, ConfiguredTakerInitiationContext, ConfiguredTakerServiceContext,
    PreparedZecTakerInitiationV1, TakerActionCommitV1, TakerBackendError, TakerClaimRequestV1,
    TakerHealthRequestV1, TakerInitiationCommitV1, TakerOfferListRequestV1, TakerRefundRequestV1,
    TakerSwapInitiateRequestV1, TakerSwapListRequestV1, TakerSwapListV1, TakerSwapMonitorRequestV1,
    TakerSwapStateV1, TakerSwapViewV1, TakerTerminalActionV1,
    secure_file::read_private_file_snapshot,
    zec_taker_accept::{
        ZecTakeInput, load_taker_actor_from_receipt_for_monitor,
        take_zec_with_authenticated_offer_and_actor_config,
    },
};

const INVALID_PARAMS_CODE: i32 = -32_602;
const INTERNAL_ERROR_CODE: i32 = -32_603;
const DEPENDENCY_UNAVAILABLE_CODE: i32 = -32_010;
const RESULT_LIMIT_EXCEEDED_CODE: i32 = -32_011;
const AUTHENTICATED_OFFER_CONFLICT_CODE: i32 = -32_012;
const INITIATION_CONFLICT_CODE: i32 = -32_013;
const SWAP_NOT_FOUND_CODE: i32 = -32_014;
const PROGRESS_GENERATION_CONFLICT_CODE: i32 = -32_015;
const ACTION_UNAVAILABLE_CODE: i32 = -32_016;
const ACTION_CONFLICT_CODE: i32 = -32_017;
const MAXIMUM_MONITORED_SWAPS: usize = 256;
const MAXIMUM_PREPARED_INPUT_BYTES: u64 = 256 * 1024;
const SIGNING_KEY_BYTES: u64 = 32;

struct TakerServiceState {
    backend: ConfiguredTakerFacadeBackend,
    initiation: Option<Arc<Mutex<ConfiguredTakerInitiationContext>>>,
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
                health.with_zec_lifecycle_registered()
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
        let list_state = Arc::clone(&state);
        module.register_async_method("taker_swap_list_v1", move |params, _, _| {
            let state = Arc::clone(&list_state);
            async move {
                let request: TakerSwapListRequestV1 = params.one().map_err(|_| {
                    rpc_error(INVALID_PARAMS_CODE, "Invalid params", "invalid_params")
                })?;
                list_swaps(state, request)
                    .await
                    .map_err(map_monitoring_error)
            }
        })?;

        let monitor_state = Arc::clone(&state);
        module.register_async_method("taker_swap_monitor_v1", move |params, _, _| {
            let state = Arc::clone(&monitor_state);
            async move {
                let request: TakerSwapMonitorRequestV1 = params.one().map_err(|_| {
                    rpc_error(INVALID_PARAMS_CODE, "Invalid params", "invalid_params")
                })?;
                monitor_swap(state, request)
                    .await
                    .map_err(map_monitoring_error)
            }
        })?;

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

        register_terminal_methods(&mut module, &state)?;
    }

    Ok(module)
}

fn register_terminal_methods(
    module: &mut RpcModule<()>,
    state: &Arc<TakerServiceState>,
) -> Result<(), RegisterMethodError> {
    let claim_state = Arc::clone(state);
    module.register_async_method("taker_swap_claim_v1", move |params, _, _| {
        let state = Arc::clone(&claim_state);
        async move {
            let request: TakerClaimRequestV1 = params
                .one()
                .map_err(|_| rpc_error(INVALID_PARAMS_CODE, "Invalid params", "invalid_params"))?;
            request
                .validate_schema_version()
                .map_err(|_| map_action_error(ActionError::UnsupportedSchemaVersion))?;
            terminal_action(
                state,
                request.request_id,
                request.swap_id,
                request.expected_generation,
                TakerTerminalActionV1::Claim,
            )
            .await
            .map_err(map_action_error)
        }
    })?;

    let refund_state = Arc::clone(state);
    module.register_async_method("taker_swap_refund_v1", move |params, _, _| {
        let state = Arc::clone(&refund_state);
        async move {
            let request: TakerRefundRequestV1 = params
                .one()
                .map_err(|_| rpc_error(INVALID_PARAMS_CODE, "Invalid params", "invalid_params"))?;
            request
                .validate_schema_version()
                .map_err(|_| map_action_error(ActionError::UnsupportedSchemaVersion))?;
            terminal_action(
                state,
                request.request_id,
                request.swap_id,
                request.expected_generation,
                TakerTerminalActionV1::Refund,
            )
            .await
            .map_err(map_action_error)
        }
    })?;
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum ActionError {
    UnsupportedSchemaVersion,
    NotFound,
    ProgressChanged,
    Unavailable,
    Conflict,
    DependencyUnavailable,
    RegistryUnavailable,
}

async fn terminal_action(
    state: Arc<TakerServiceState>,
    request_id: RequestId,
    swap_id: SwapId,
    expected_generation: u64,
    action: TakerTerminalActionV1,
) -> Result<TakerActionCommitV1, ActionError> {
    let initiation = state
        .initiation
        .clone()
        .ok_or(ActionError::RegistryUnavailable)?;
    let (prepared, receipt_binding) = resolve_action_authority(&initiation, &swap_id).await?;
    let receipt_binding = receipt_binding.ok_or(ActionError::DependencyUnavailable)?;
    let (config, held_lock) = load_bound_action_actor(&prepared, receipt_binding).await?;

    // Exact durable replay deliberately precedes current actor progress. The
    // original command can then reconcile its own persist-before-send journal
    // after a response loss or process restart.
    let replay = lookup_action_replay(
        &initiation,
        &request_id,
        &swap_id,
        action,
        expected_generation,
    )
    .await?;
    if let Some(admission) = replay {
        if replay_actor_effect_is_required(&config, &held_lock, action, expected_generation).await?
        {
            execute_terminal_actor_command(&config, &held_lock, action).await?;
        }
        revalidate_action_custody(&prepared, receipt_binding, &config, &held_lock).await?;
        return Ok(action_commit(&admission));
    }

    let output = execute_actor_command(&config, ActorCommand::Status)
        .await
        .map_err(|_| ActionError::DependencyUnavailable)?;
    held_lock
        .validate_for_state(config.swap_id(), config.role_state_db())
        .map_err(|_| ActionError::DependencyUnavailable)?;
    let ActorCommandOutputV1::Status(status) = output else {
        return Err(ActionError::DependencyUnavailable);
    };
    let ActorStatusProjectionV1::Active {
        phase,
        revision,
        next_action,
    } = status.projection()
    else {
        return Err(ActionError::Unavailable);
    };
    if revision != expected_generation {
        return Err(ActionError::ProgressChanged);
    }
    let (_, available_action, _) = normalized_actor_progress(phase, next_action);
    if available_action != Some(action) {
        return Err(ActionError::Unavailable);
    }

    let admitted_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ActionError::RegistryUnavailable)?
        .as_secs();
    let admission = admit_terminal_action(
        &initiation,
        &request_id,
        &swap_id,
        action,
        expected_generation,
        admitted_at,
    )
    .await?;
    execute_terminal_actor_command(&config, &held_lock, action).await?;
    revalidate_action_custody(&prepared, receipt_binding, &config, &held_lock).await?;
    Ok(action_commit(&admission))
}

async fn resolve_action_authority(
    initiation: &Arc<Mutex<ConfiguredTakerInitiationContext>>,
    swap_id: &SwapId,
) -> Result<
    (
        PreparedZecTakerInitiationV1,
        Option<crate::taker_service_config::PreparedReceiptBindingV1>,
    ),
    ActionError,
> {
    let context = Arc::clone(initiation);
    let swap_id = swap_id.clone();
    tokio::task::spawn_blocking(move || {
        let mut context = context
            .lock()
            .map_err(|_| ActionError::RegistryUnavailable)?;
        let prepared = context
            .prepared_zec_for_swap(&swap_id)
            .cloned()
            .ok_or(ActionError::NotFound)?;
        let facts = context
            .registry_mut()
            .lookup_initiation_for_monitor(&swap_id, prepared.authority())
            .map_err(|_| ActionError::RegistryUnavailable)?
            .ok_or(ActionError::NotFound)?;
        if &facts != prepared.facts() {
            return Err(ActionError::RegistryUnavailable);
        }
        let receipt_binding = prepared.execution().receipt_binding();
        Ok((prepared, receipt_binding))
    })
    .await
    .map_err(|_| ActionError::RegistryUnavailable)?
}

async fn load_bound_action_actor(
    prepared: &PreparedZecTakerInitiationV1,
    receipt_binding: crate::taker_service_config::PreparedReceiptBindingV1,
) -> Result<(ActorConfig, MakerActorHeldLock), ActionError> {
    let receipt = prepared.execution().receipt_output().to_path_buf();
    let actor_root = prepared.execution().actor_root().to_path_buf();
    let swap_id = prepared.swap_id().clone();
    let receipt_sha256 = receipt_binding.sha256();
    let receipt_identity = receipt_binding.identity();
    tokio::task::spawn_blocking(move || {
        let before = load_taker_actor_from_receipt_for_monitor(
            &receipt,
            &actor_root,
            &swap_id,
            receipt_sha256,
            receipt_identity.device(),
            receipt_identity.inode(),
        )
        .map_err(|_| ActionError::DependencyUnavailable)?;
        let held_lock = MakerActorHeldLock::acquire_for(before.swap_id(), before.role_state_db())
            .map_err(|_| ActionError::DependencyUnavailable)?;
        let config = load_taker_actor_from_receipt_for_monitor(
            &receipt,
            &actor_root,
            &swap_id,
            receipt_sha256,
            receipt_identity.device(),
            receipt_identity.inode(),
        )
        .map_err(|_| ActionError::DependencyUnavailable)?;
        if before.swap_id() != config.swap_id()
            || before.role_state_db() != config.role_state_db()
            || before.bridge_journal_db() != config.bridge_journal_db()
            || before.signed_agreement_sha256() != config.signed_agreement_sha256()
        {
            return Err(ActionError::DependencyUnavailable);
        }
        held_lock
            .validate_for_state(config.swap_id(), config.role_state_db())
            .map_err(|_| ActionError::DependencyUnavailable)?;
        Ok((config, held_lock))
    })
    .await
    .map_err(|_| ActionError::DependencyUnavailable)?
}

async fn revalidate_action_custody(
    prepared: &PreparedZecTakerInitiationV1,
    receipt_binding: crate::taker_service_config::PreparedReceiptBindingV1,
    config: &ActorConfig,
    held_lock: &MakerActorHeldLock,
) -> Result<(), ActionError> {
    let receipt = prepared.execution().receipt_output().to_path_buf();
    let actor_root = prepared.execution().actor_root().to_path_buf();
    let swap_id = prepared.swap_id().clone();
    let receipt_sha256 = receipt_binding.sha256();
    let receipt_identity = receipt_binding.identity();
    let after = tokio::task::spawn_blocking(move || {
        load_taker_actor_from_receipt_for_monitor(
            &receipt,
            &actor_root,
            &swap_id,
            receipt_sha256,
            receipt_identity.device(),
            receipt_identity.inode(),
        )
        .map_err(|_| ActionError::DependencyUnavailable)
    })
    .await
    .map_err(|_| ActionError::DependencyUnavailable)??;
    if after.swap_id() != config.swap_id()
        || after.role_state_db() != config.role_state_db()
        || after.bridge_journal_db() != config.bridge_journal_db()
        || after.signed_agreement_sha256() != config.signed_agreement_sha256()
    {
        return Err(ActionError::DependencyUnavailable);
    }
    held_lock
        .validate_for_state(config.swap_id(), config.role_state_db())
        .map_err(|_| ActionError::DependencyUnavailable)
}

async fn replay_actor_effect_is_required(
    config: &ActorConfig,
    held_lock: &MakerActorHeldLock,
    action: TakerTerminalActionV1,
    expected_generation: u64,
) -> Result<bool, ActionError> {
    let output = execute_actor_command(config, ActorCommand::Status)
        .await
        .map_err(|_| ActionError::DependencyUnavailable)?;
    held_lock
        .validate_for_state(config.swap_id(), config.role_state_db())
        .map_err(|_| ActionError::DependencyUnavailable)?;
    let ActorCommandOutputV1::Status(status) = output else {
        return Err(ActionError::DependencyUnavailable);
    };
    replay_actor_effect_required(status.projection(), expected_generation, action)
}

fn replay_actor_effect_required(
    status: ActorStatusProjectionV1,
    expected_generation: u64,
    action: TakerTerminalActionV1,
) -> Result<bool, ActionError> {
    let ActorStatusProjectionV1::Active {
        phase,
        revision,
        next_action,
    } = status
    else {
        return Err(ActionError::DependencyUnavailable);
    };
    if revision > expected_generation {
        return Ok(false);
    }
    if revision < expected_generation {
        return Err(ActionError::ProgressChanged);
    }
    let (_, available_action, _) = normalized_actor_progress(phase, next_action);
    if available_action != Some(action) {
        return Err(ActionError::Unavailable);
    }
    Ok(true)
}

async fn lookup_action_replay(
    initiation: &Arc<Mutex<ConfiguredTakerInitiationContext>>,
    request_id: &RequestId,
    swap_id: &SwapId,
    action: TakerTerminalActionV1,
    expected_generation: u64,
) -> Result<Option<TakerActionAdmissionV1>, ActionError> {
    let context = Arc::clone(initiation);
    let request_id = request_id.clone();
    let swap_id = swap_id.clone();
    tokio::task::spawn_blocking(move || {
        let mut context = context
            .lock()
            .map_err(|_| ActionError::RegistryUnavailable)?;
        context
            .registry_mut()
            .lookup_exact_action(
                &request_id,
                &swap_id,
                facade_action(action),
                expected_generation,
            )
            .map_err(map_action_store_error)
    })
    .await
    .map_err(|_| ActionError::RegistryUnavailable)?
}

async fn admit_terminal_action(
    initiation: &Arc<Mutex<ConfiguredTakerInitiationContext>>,
    request_id: &RequestId,
    swap_id: &SwapId,
    action: TakerTerminalActionV1,
    expected_generation: u64,
    admitted_at: u64,
) -> Result<TakerActionAdmissionV1, ActionError> {
    let context = Arc::clone(initiation);
    let request_id = request_id.clone();
    let swap_id = swap_id.clone();
    tokio::task::spawn_blocking(move || {
        let mut context = context
            .lock()
            .map_err(|_| ActionError::RegistryUnavailable)?;
        context
            .registry_mut()
            .admit_action(
                &request_id,
                &swap_id,
                facade_action(action),
                expected_generation,
                admitted_at,
            )
            .map_err(map_action_store_error)
    })
    .await
    .map_err(|_| ActionError::RegistryUnavailable)?
}

async fn execute_terminal_actor_command(
    config: &ActorConfig,
    held_lock: &MakerActorHeldLock,
    action: TakerTerminalActionV1,
) -> Result<(), ActionError> {
    let command = match action {
        TakerTerminalActionV1::Claim => ActorCommand::Claim,
        TakerTerminalActionV1::Refund => ActorCommand::Recover,
    };
    let output = execute_actor_command(config, command)
        .await
        .map_err(|_| ActionError::DependencyUnavailable)?;
    let ActorCommandOutputV1::Effect(_) = output else {
        return Err(ActionError::DependencyUnavailable);
    };
    held_lock
        .validate_for_state(config.swap_id(), config.role_state_db())
        .map_err(|_| ActionError::DependencyUnavailable)
}

const fn facade_action(action: TakerTerminalActionV1) -> TakerFacadeActionV1 {
    match action {
        TakerTerminalActionV1::Claim => TakerFacadeActionV1::Claim,
        TakerTerminalActionV1::Refund => TakerFacadeActionV1::Refund,
    }
}

fn action_commit(admission: &TakerActionAdmissionV1) -> TakerActionCommitV1 {
    TakerActionCommitV1 {
        schema_version: 1,
        swap_id: admission.swap_id().clone(),
        action: match admission.action() {
            TakerFacadeActionV1::Claim => TakerTerminalActionV1::Claim,
            TakerFacadeActionV1::Refund => TakerTerminalActionV1::Refund,
        },
        requested_after_generation: admission.requested_after_generation(),
        was_replay: admission.was_replay(),
    }
}

fn map_action_store_error(error: TakerFacadeStoreError) -> ActionError {
    match error {
        TakerFacadeStoreError::RequestConflict => ActionError::Conflict,
        TakerFacadeStoreError::ActionGenerationConflict => ActionError::Unavailable,
        TakerFacadeStoreError::SwapUnavailable => ActionError::NotFound,
        TakerFacadeStoreError::SwapConflict
        | TakerFacadeStoreError::DatabaseUnavailable
        | TakerFacadeStoreError::UnsafeDatabaseFile
        | TakerFacadeStoreError::DatabaseAlreadyExists
        | TakerFacadeStoreError::ForeignSchema
        | TakerFacadeStoreError::FutureSchema
        | TakerFacadeStoreError::CorruptState
        | TakerFacadeStoreError::InvalidInput
        | TakerFacadeStoreError::StorageUnavailable => ActionError::RegistryUnavailable,
    }
}

fn map_action_error(error: ActionError) -> ErrorObjectOwned {
    match error {
        ActionError::UnsupportedSchemaVersion => rpc_error(
            INVALID_PARAMS_CODE,
            "Invalid params",
            "unsupported_schema_version",
        ),
        ActionError::NotFound => rpc_error(
            SWAP_NOT_FOUND_CODE,
            "Taker swap not found",
            "swap_not_found",
        ),
        ActionError::ProgressChanged => rpc_error(
            PROGRESS_GENERATION_CONFLICT_CODE,
            "Taker swap progress changed",
            "progress_generation_conflict",
        ),
        ActionError::Unavailable => rpc_error(
            ACTION_UNAVAILABLE_CODE,
            "Taker action unavailable",
            "taker_action_unavailable",
        ),
        ActionError::Conflict => rpc_error(
            ACTION_CONFLICT_CODE,
            "Taker action conflict",
            "taker_action_conflict",
        ),
        ActionError::DependencyUnavailable => rpc_error(
            DEPENDENCY_UNAVAILABLE_CODE,
            "Taker dependency unavailable",
            "taker_action_execution_unavailable",
        ),
        ActionError::RegistryUnavailable => rpc_error(
            INTERNAL_ERROR_CODE,
            "Internal error",
            "action_registry_unavailable",
        ),
    }
}

#[derive(Clone, Copy, Debug)]
enum MonitoringError {
    UnsupportedSchemaVersion,
    NotFound,
    DependencyUnavailable,
    RegistryUnavailable,
    ResultLimitExceeded,
}

async fn list_swaps(
    state: Arc<TakerServiceState>,
    request: TakerSwapListRequestV1,
) -> Result<TakerSwapListV1, MonitoringError> {
    request
        .validate_schema_version()
        .map_err(|_| MonitoringError::UnsupportedSchemaVersion)?;
    let initiation = state
        .initiation
        .clone()
        .ok_or(MonitoringError::RegistryUnavailable)?;
    let lookup = Arc::clone(&initiation);
    let swap_ids = tokio::task::spawn_blocking(move || {
        let mut context = lookup
            .lock()
            .map_err(|_| MonitoringError::RegistryUnavailable)?;
        let facts = context
            .registry_mut()
            .list_initiations()
            .map_err(|_| MonitoringError::RegistryUnavailable)?;
        if facts.len() > MAXIMUM_MONITORED_SWAPS {
            return Err(MonitoringError::ResultLimitExceeded);
        }
        Ok::<_, MonitoringError>(
            facts
                .into_iter()
                .map(|facts| facts.swap_id().clone())
                .collect::<Vec<_>>(),
        )
    })
    .await
    .map_err(|_| MonitoringError::RegistryUnavailable)??;

    let mut swaps = Vec::with_capacity(swap_ids.len());
    for swap_id in swap_ids {
        swaps.push(project_swap(&initiation, &swap_id).await?);
    }
    Ok(TakerSwapListV1 {
        schema_version: 1,
        swaps,
    })
}

async fn monitor_swap(
    state: Arc<TakerServiceState>,
    request: TakerSwapMonitorRequestV1,
) -> Result<TakerSwapViewV1, MonitoringError> {
    request
        .validate_schema_version()
        .map_err(|_| MonitoringError::UnsupportedSchemaVersion)?;
    let initiation = state
        .initiation
        .clone()
        .ok_or(MonitoringError::RegistryUnavailable)?;
    project_swap(&initiation, &request.swap_id).await
}

async fn project_swap(
    initiation: &Arc<Mutex<ConfiguredTakerInitiationContext>>,
    swap_id: &SwapId,
) -> Result<TakerSwapViewV1, MonitoringError> {
    let lookup = Arc::clone(initiation);
    let swap_id = swap_id.clone();
    let (prepared, facts, receipt_binding) = tokio::task::spawn_blocking(move || {
        let mut context = lookup
            .lock()
            .map_err(|_| MonitoringError::RegistryUnavailable)?;
        let prepared = context
            .prepared_zec_for_swap(&swap_id)
            .cloned()
            .ok_or(MonitoringError::NotFound)?;
        let facts = context
            .registry_mut()
            .lookup_initiation_for_monitor(&swap_id, prepared.authority())
            .map_err(|_| MonitoringError::RegistryUnavailable)?
            .ok_or(MonitoringError::NotFound)?;
        if &facts != prepared.facts() {
            return Err(MonitoringError::RegistryUnavailable);
        }
        let receipt_binding = prepared.execution().receipt_binding();
        Ok::<_, MonitoringError>((prepared, facts, receipt_binding))
    })
    .await
    .map_err(|_| MonitoringError::RegistryUnavailable)??;

    match fs::symlink_metadata(prepared.execution().receipt_output()) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && receipt_binding.is_none() => {
            Ok(commit_from_facts(&facts, false, TakerSwapStateV1::Initiating).swap)
        }
        Err(_) => Err(MonitoringError::DependencyUnavailable),
        Ok(_) => {
            let receipt_binding = receipt_binding.ok_or(MonitoringError::DependencyUnavailable)?;
            project_receipt_bound_swap(initiation, &prepared, &facts, receipt_binding).await
        }
    }
}

async fn project_receipt_bound_swap(
    initiation: &Arc<Mutex<ConfiguredTakerInitiationContext>>,
    prepared: &PreparedZecTakerInitiationV1,
    facts: &TakerInitiationFactsV1,
    receipt_binding: crate::taker_service_config::PreparedReceiptBindingV1,
) -> Result<TakerSwapViewV1, MonitoringError> {
    let receipt = prepared.execution().receipt_output().to_path_buf();
    let actor_root = prepared.execution().actor_root().to_path_buf();
    let swap_id = prepared.swap_id().clone();
    let receipt_sha256 = receipt_binding.sha256();
    let receipt_identity = receipt_binding.identity();
    let (config, held_lock) = tokio::task::spawn_blocking(move || {
        let before = load_taker_actor_from_receipt_for_monitor(
            &receipt,
            &actor_root,
            &swap_id,
            receipt_sha256,
            receipt_identity.device(),
            receipt_identity.inode(),
        )
        .map_err(|_| MonitoringError::DependencyUnavailable)?;
        let held_lock = MakerActorHeldLock::acquire_for(before.swap_id(), before.role_state_db())
            .map_err(|_| MonitoringError::DependencyUnavailable)?;
        let config = load_taker_actor_from_receipt_for_monitor(
            &receipt,
            &actor_root,
            &swap_id,
            receipt_sha256,
            receipt_identity.device(),
            receipt_identity.inode(),
        )
        .map_err(|_| MonitoringError::DependencyUnavailable)?;
        if before.swap_id() != config.swap_id()
            || before.role_state_db() != config.role_state_db()
            || before.bridge_journal_db() != config.bridge_journal_db()
            || before.signed_agreement_sha256() != config.signed_agreement_sha256()
        {
            return Err(MonitoringError::DependencyUnavailable);
        }
        held_lock
            .validate_for_state(config.swap_id(), config.role_state_db())
            .map_err(|_| MonitoringError::DependencyUnavailable)?;
        Ok::<_, MonitoringError>((config, held_lock))
    })
    .await
    .map_err(|_| MonitoringError::DependencyUnavailable)??;

    let admitted_action = lookup_monitored_action(initiation, config.swap_id()).await?;
    let output = execute_actor_command(&config, ActorCommand::Status)
        .await
        .map_err(|_| MonitoringError::DependencyUnavailable)?;
    held_lock
        .validate_for_state(config.swap_id(), config.role_state_db())
        .map_err(|_| MonitoringError::DependencyUnavailable)?;
    let ActorCommandOutputV1::Status(status) = output else {
        return Err(MonitoringError::DependencyUnavailable);
    };
    overlay_admitted_action(
        view_from_actor_status(facts, status.projection()),
        admitted_action.as_ref(),
    )
}

async fn lookup_monitored_action(
    initiation: &Arc<Mutex<ConfiguredTakerInitiationContext>>,
    swap_id: &SwapId,
) -> Result<Option<TakerActionAdmissionV1>, MonitoringError> {
    let context = Arc::clone(initiation);
    let swap_id = swap_id.clone();
    tokio::task::spawn_blocking(move || {
        let mut context = context
            .lock()
            .map_err(|_| MonitoringError::RegistryUnavailable)?;
        context
            .registry_mut()
            .lookup_action_for_swap(&swap_id)
            .map_err(|_| MonitoringError::RegistryUnavailable)
    })
    .await
    .map_err(|_| MonitoringError::RegistryUnavailable)?
}

fn overlay_admitted_action(
    mut view: TakerSwapViewV1,
    admission: Option<&TakerActionAdmissionV1>,
) -> Result<TakerSwapViewV1, MonitoringError> {
    let Some(admission) = admission else {
        return Ok(view);
    };
    if admission.requested_after_generation() > view.progress_generation {
        return Err(MonitoringError::DependencyUnavailable);
    }
    if matches!(
        view.state,
        TakerSwapStateV1::Completed | TakerSwapStateV1::Refunded
    ) {
        return Ok(view);
    }
    view.state = match admission.action() {
        TakerFacadeActionV1::Claim => TakerSwapStateV1::ClaimInProgress,
        TakerFacadeActionV1::Refund => TakerSwapStateV1::RefundInProgress,
    };
    view.available_action = None;
    view.privacy_guidance = None;
    Ok(view)
}

fn view_from_actor_status(
    facts: &TakerInitiationFactsV1,
    status: ActorStatusProjectionV1,
) -> TakerSwapViewV1 {
    let (state, progress_generation, available_action, privacy_guidance) = match status {
        ActorStatusProjectionV1::NotActivated => (TakerSwapStateV1::NotActivated, 0, None, None),
        ActorStatusProjectionV1::Active {
            phase,
            revision,
            next_action,
        } => {
            let (state, action, guidance) = normalized_actor_progress(phase, next_action);
            (state, revision, action, guidance)
        }
    };
    TakerSwapViewV1 {
        schema_version: 1,
        swap_id: facts.swap_id().clone(),
        offer_id: facts.offer_id().clone(),
        route: facts.route(),
        foreign_units: facts.foreign_units(),
        lez_units: facts.lez_units(),
        progress_generation,
        state,
        available_action,
        privacy_guidance,
    }
}

fn normalized_actor_progress(
    phase: Phase,
    next_action: ZecLifecycleAction,
) -> (
    TakerSwapStateV1,
    Option<crate::TakerTerminalActionV1>,
    Option<crate::TakerPrivacyGuidanceV1>,
) {
    use crate::{TakerPrivacyGuidanceV1, TakerTerminalActionV1};
    match phase {
        Phase::Offered => (TakerSwapStateV1::AwaitingFirstLock, None, None),
        Phase::AwaitingTakerConfirmations
        | Phase::TakerLockConfirmed
        | Phase::AwaitingMakerConfirmations
        | Phase::BothLegsLocked
        | Phase::TakerLockReorged
        | Phase::MakerLockReorged => (
            TakerSwapStateV1::RefundAvailable,
            Some(TakerTerminalActionV1::Refund),
            None,
        ),
        Phase::ClaimEvidenceAvailable if next_action == ZecLifecycleAction::ClaimZcash => (
            TakerSwapStateV1::ClaimAvailable,
            Some(TakerTerminalActionV1::Claim),
            None,
        ),
        Phase::Completed => (
            TakerSwapStateV1::Completed,
            None,
            Some(TakerPrivacyGuidanceV1::ShieldReceivedTransparentZecSeparately),
        ),
        Phase::Refunded => (TakerSwapStateV1::Refunded, None, None),
        Phase::ClaimEvidenceAvailable
        | Phase::MakerLegRefunded
        | Phase::TakerLegRefunded
        | Phase::MakerRecoveryAvailable => (TakerSwapStateV1::AttentionRequired, None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TakerPrivacyGuidanceV1, TakerTerminalActionV1};

    #[test]
    fn zec_taker_projection_advertises_only_role_valid_terminal_actions() {
        assert_eq!(
            normalized_actor_progress(Phase::Offered, ZecLifecycleAction::CreateAndFundLez),
            (TakerSwapStateV1::AwaitingFirstLock, None, None)
        );
        assert_eq!(
            normalized_actor_progress(Phase::BothLegsLocked, ZecLifecycleAction::Wait),
            (
                TakerSwapStateV1::RefundAvailable,
                Some(TakerTerminalActionV1::Refund),
                None,
            )
        );
        assert_eq!(
            normalized_actor_progress(Phase::AwaitingMakerConfirmations, ZecLifecycleAction::Wait,),
            (
                TakerSwapStateV1::RefundAvailable,
                Some(TakerTerminalActionV1::Refund),
                None,
            )
        );
        assert_eq!(
            normalized_actor_progress(
                Phase::ClaimEvidenceAvailable,
                ZecLifecycleAction::ClaimZcash,
            ),
            (
                TakerSwapStateV1::ClaimAvailable,
                Some(TakerTerminalActionV1::Claim),
                None,
            )
        );
        assert_eq!(
            normalized_actor_progress(Phase::MakerLegRefunded, ZecLifecycleAction::RefundZcash,),
            (TakerSwapStateV1::AttentionRequired, None, None)
        );
        assert_eq!(
            normalized_actor_progress(Phase::Completed, ZecLifecycleAction::Complete),
            (
                TakerSwapStateV1::Completed,
                None,
                Some(TakerPrivacyGuidanceV1::ShieldReceivedTransparentZecSeparately),
            )
        );
    }

    #[test]
    fn exact_action_replay_only_reenters_an_unadvanced_matching_effect() {
        let claim_available = ActorStatusProjectionV1::Active {
            phase: Phase::ClaimEvidenceAvailable,
            revision: 3,
            next_action: ZecLifecycleAction::ClaimZcash,
        };
        assert!(
            replay_actor_effect_required(claim_available, 3, TakerTerminalActionV1::Claim,)
                .unwrap()
        );

        let completed = ActorStatusProjectionV1::Active {
            phase: Phase::Completed,
            revision: 4,
            next_action: ZecLifecycleAction::Complete,
        };
        assert!(
            !replay_actor_effect_required(completed, 3, TakerTerminalActionV1::Claim,).unwrap()
        );

        assert!(
            replay_actor_effect_required(
                ActorStatusProjectionV1::NotActivated,
                3,
                TakerTerminalActionV1::Claim,
            )
            .is_err()
        );
    }
}

fn map_monitoring_error(error: MonitoringError) -> ErrorObjectOwned {
    match error {
        MonitoringError::UnsupportedSchemaVersion => rpc_error(
            INVALID_PARAMS_CODE,
            "Invalid params",
            "unsupported_schema_version",
        ),
        MonitoringError::NotFound => rpc_error(
            SWAP_NOT_FOUND_CODE,
            "Taker swap not found",
            "swap_not_found",
        ),
        MonitoringError::DependencyUnavailable => rpc_error(
            DEPENDENCY_UNAVAILABLE_CODE,
            "Taker dependency unavailable",
            "taker_monitor_unavailable",
        ),
        MonitoringError::RegistryUnavailable => rpc_error(
            INTERNAL_ERROR_CODE,
            "Internal error",
            "taker_registry_unavailable",
        ),
        MonitoringError::ResultLimitExceeded => rpc_error(
            RESULT_LIMIT_EXCEEDED_CODE,
            "Taker result limit exceeded",
            "swap_limit_exceeded",
        ),
    }
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
            bind_prepared_receipt(&initiation, prepared.swap_id()).await?;
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
        bind_prepared_receipt(&initiation, prepared.swap_id()).await?;
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

async fn bind_prepared_receipt(
    initiation: &Arc<Mutex<ConfiguredTakerInitiationContext>>,
    swap_id: &SwapId,
) -> Result<(), InitiationError> {
    let context = Arc::clone(initiation);
    let swap_id = swap_id.clone();
    tokio::task::spawn_blocking(move || {
        context
            .lock()
            .map_err(|_| InitiationError::Internal)?
            .bind_prepared_zec_receipt(&swap_id)
            .map_err(|()| InitiationError::ExecutionUnavailable)
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
        TakerFacadeStoreError::SwapUnavailable
        | TakerFacadeStoreError::ActionGenerationConflict
        | TakerFacadeStoreError::DatabaseUnavailable
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
