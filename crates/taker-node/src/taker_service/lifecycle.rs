//! Prepared-route Taker lifecycle: initiation, monitoring, and generation-fenced
//! claim/refund over the role actor of the swap's pair. Bitcoin runs on the BTC
//! reference actor; Zcash (with `pair-zec`) on the ZEC reference actor. The five
//! method names are shared and dispatch on the pair the registry recorded.

use super::{
    DEPENDENCY_UNAVAILABLE_CODE, INTERNAL_ERROR_CODE, INVALID_PARAMS_CODE,
    RESULT_LIMIT_EXCEEDED_CODE, TakerServiceState, map_backend_error, rpc_error,
};
use std::{
    fs,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use jsonrpsee::{RpcModule, core::RegisterMethodError, types::ErrorObjectOwned};
use lez_bridge_protocol::RequestId;
use lez_swap_core::{Pair, Phase, SwapDirection, SwapId};
use lez_swap_store::{
    ActorHeldLock, MakerOfferStatus, TakerActionAdmissionV1, TakerFacadeActionV1,
    TakerFacadeStoreError, TakerInitiationAdmissionV1, TakerInitiationFactsV1,
};
#[cfg(feature = "pair-zec")]
use lez_zec_swap_sdk::ZecLifecycleAction;
#[cfg(feature = "pair-zec")]
use secp256k1::PublicKey;
use serde_json::json;
use sha2::{Digest as _, Sha256};
use std::path::{Path, PathBuf};

use super::btc_dynamic;
#[cfg(feature = "pair-zec")]
use crate::zec_taker_accept::{
    ZecTakeInput, load_taker_actor_from_receipt_for_monitor,
    take_zec_with_authenticated_offer_and_actor_config,
};
use crate::{
    ConfiguredTakerInitiationContext, PreparedTakerInitiationV1, TakerActionCommitV1,
    TakerBackendError, TakerClaimRequestV1, TakerHealthRequestV1, TakerInitiationCommitV1,
    TakerLockCommitV1, TakerLockRequestV1, TakerOfferListRequestV1, TakerPrivacyGuidanceV1,
    TakerRefundRequestV1, TakerSwapInitiateRequestV1, TakerSwapListRequestV1, TakerSwapListV1,
    TakerSwapMonitorRequestV1, TakerSwapStateV1, TakerSwapViewV1, TakerTerminalActionV1,
    btc_taker_accept::{
        BtcTakeInput, load_btc_taker_actor_from_receipt_for_monitor,
        take_btc_with_authenticated_offer_and_actor_config,
    },
    run_local_delivery::MAXIMUM_LOGOS_OFFER_ANNOUNCEMENT_BASE64_BYTES,
    secure_file::read_private_file_snapshot,
    verify_logos_offer_announcement,
};

/// The role actor that runs one prepared swap, selected by the swap's pair.
///
/// Every lifecycle step below talks to the actor through this enum so the
/// registry, the held lock, the admission fences and the error mapping stay
/// identical across pairs; only the actor crate differs.
#[allow(clippy::large_enum_variant)]
pub(super) enum RouteActor {
    Btc {
        config: btc_reference_actor::ActorConfig,
        swap_id: SwapId,
    },
    #[cfg(feature = "pair-zec")]
    Zec(zec_reference_actor::ActorConfig),
}

/// Typed, pair-neutral view of one actor's durable progress.
/// The Zcash actor's status as the supervisor reads it.
#[cfg(feature = "pair-zec")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ZecActorStatus {
    NotActivated,
    Active {
        phase: Phase,
        revision: u64,
        next_action: ZecLifecycleAction,
        /// The agreement's LEZ refund time, Unix milliseconds.
        lez_refund_at_ms: Option<u64>,
        /// How near that time a held revealing claim needs attention, ms.
        revealing_claim_hold_attention_ms: Option<u64>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LifecycleStatus {
    NotActivated,
    Active {
        phase: Phase,
        revision: u64,
        state: TakerSwapStateV1,
        available_action: Option<TakerTerminalActionV1>,
        privacy_guidance: Option<TakerPrivacyGuidanceV1>,
    },
}

impl RouteActor {
    /// Whether this is the Zcash pair's in-process actor, whose admitted
    /// actions the Taker Node's supervisor retries by itself.
    #[cfg_attr(
        not(feature = "pair-zec"),
        expect(
            clippy::unused_self,
            reason = "without the Zcash pair no route is the Zcash actor"
        )
    )]
    pub(super) const fn is_zec(&self) -> bool {
        #[cfg(feature = "pair-zec")]
        {
            matches!(self, Self::Zec(_))
        }
        #[cfg(not(feature = "pair-zec"))]
        {
            false
        }
    }

    /// Reloads the receipt-bound actor of `pair`, pinning the receipt digest and
    /// identity and confining the bundle to the configured actor root.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn load_for_monitor(
        pair: Pair,
        receipt: &Path,
        actor_root: &Path,
        swap_id: &SwapId,
        receipt_sha256: [u8; 32],
        device: u64,
        inode: u64,
    ) -> anyhow::Result<Self> {
        match pair {
            Pair::Bitcoin => {
                let config = load_btc_taker_actor_from_receipt_for_monitor(
                    receipt,
                    actor_root,
                    swap_id,
                    receipt_sha256,
                    device,
                    inode,
                )?;
                Ok(Self::Btc {
                    config,
                    swap_id: swap_id.clone(),
                })
            }
            #[cfg(feature = "pair-zec")]
            Pair::Zcash => load_taker_actor_from_receipt_for_monitor(
                receipt,
                actor_root,
                swap_id,
                receipt_sha256,
                device,
                inode,
            )
            .map(Self::Zec),
            #[allow(unreachable_patterns)]
            _ => Err(anyhow::anyhow!("pair has no Taker Node lifecycle route")),
        }
    }

    pub(super) fn swap_id(&self) -> &SwapId {
        match self {
            Self::Btc { swap_id, .. } => swap_id,
            #[cfg(feature = "pair-zec")]
            Self::Zec(config) => config.swap_id(),
        }
    }

    /// The bound agreement's public schedule and amounts; ZEC has none yet.
    fn agreement_terms(&self) -> Option<lez_btc_swap_sdk::BtcAgreementTermsV1> {
        match self {
            Self::Btc { config, .. } => config.agreement_terms(),
            #[cfg(feature = "pair-zec")]
            Self::Zec(_) => None,
        }
    }

    /// The swap's on-chain effects so far; empty when they cannot be read.
    fn effects(&self) -> Vec<btc_reference_actor::ActorEffectV1> {
        match self {
            Self::Btc { config, .. } => {
                btc_reference_actor::actor_effects(config).unwrap_or_default()
            }
            #[cfg(feature = "pair-zec")]
            Self::Zec(_) => Vec::new(),
        }
    }

    /// The fee of this role's planned Bitcoin lock, from the funding plan the
    /// take wrote beside the actor's state; absent when the Maker funds Bitcoin
    /// or the plan predates the fee being kept.
    fn bitcoin_lock_fee_sat(&self) -> Option<u64> {
        planned_lock_fee_sat(self.state_db())
    }

    pub(super) fn state_db(&self) -> &Path {
        match self {
            Self::Btc { config, .. } => config.state_db(),
            #[cfg(feature = "pair-zec")]
            Self::Zec(config) => config.role_state_db(),
        }
    }

    /// Everything a custody re-read must reproduce byte for byte.
    pub(super) fn custody_identity(&self) -> (SwapId, PathBuf, PathBuf, [u8; 32]) {
        match self {
            Self::Btc { config, swap_id } => (
                swap_id.clone(),
                config.state_db().to_path_buf(),
                PathBuf::new(),
                config.agreement_sha256().unwrap_or_default(),
            ),
            #[cfg(feature = "pair-zec")]
            Self::Zec(config) => (
                config.swap_id().clone(),
                config.role_state_db().to_path_buf(),
                config.bridge_journal_db().to_path_buf(),
                config.signed_agreement_sha256(),
            ),
        }
    }

    /// Reads durable status without touching a chain.
    pub(super) async fn status(&self) -> Result<LifecycleStatus, ()> {
        match self {
            Self::Btc { config, .. } => {
                use btc_reference_actor::{
                    ActorCommand, ActorCommandOutputV1, ActorStatusProjectionV1,
                    execute_actor_command,
                };
                let output = execute_actor_command(config, ActorCommand::Status)
                    .await
                    .map_err(|error| report_actor_failure("status", &error))?;
                let ActorCommandOutputV1::Status(status) = output else {
                    return Err(());
                };
                Ok(match status.projection() {
                    ActorStatusProjectionV1::NotActivated => LifecycleStatus::NotActivated,
                    ActorStatusProjectionV1::Active {
                        phase,
                        revision,
                        next_action,
                    } => {
                        let (state, available_action) = btc_actor_progress(phase, next_action);
                        LifecycleStatus::Active {
                            phase,
                            revision,
                            state,
                            available_action,
                            privacy_guidance: None,
                        }
                    }
                })
            }
            #[cfg(feature = "pair-zec")]
            Self::Zec(config) => {
                use zec_reference_actor::{
                    ActorCommand, ActorCommandOutputV1, ActorStatusProjectionV1,
                    execute_actor_command,
                };
                let output = execute_actor_command(config, ActorCommand::Status)
                    .await
                    .map_err(|error| report_actor_failure("status", &error))?;
                let ActorCommandOutputV1::Status(status) = output else {
                    return Err(());
                };
                Ok(match status.projection() {
                    ActorStatusProjectionV1::NotActivated => LifecycleStatus::NotActivated,
                    ActorStatusProjectionV1::Active {
                        phase,
                        revision,
                        next_action,
                    } => {
                        let (state, available_action, privacy_guidance) =
                            zec_actor_progress(phase, next_action);
                        LifecycleStatus::Active {
                            phase,
                            revision,
                            state,
                            available_action,
                            privacy_guidance,
                        }
                    }
                })
            }
        }
    }

    /// The Zcash actor's durable status as the supervisor decides on it:
    /// phase, revision and next action, or not activated.
    #[cfg(feature = "pair-zec")]
    pub(super) async fn zec_status(
        &self,
    ) -> Result<ZecActorStatus, zec_reference_actor::ActorCommandError> {
        use zec_reference_actor::{
            ActorCommand, ActorCommandError, ActorCommandOutputV1, ActorStatusProjectionV1,
            execute_actor_command,
        };
        let Self::Zec(config) = self else {
            return Err(ActorCommandError::StatusUnavailable);
        };
        match execute_actor_command(config, ActorCommand::Status).await? {
            ActorCommandOutputV1::Status(status) => Ok(match status.projection() {
                ActorStatusProjectionV1::NotActivated => ZecActorStatus::NotActivated,
                ActorStatusProjectionV1::Active {
                    phase,
                    revision,
                    next_action,
                } => ZecActorStatus::Active {
                    phase,
                    revision,
                    next_action,
                    lez_refund_at_ms: status.lez_refund_at_ms(),
                    revealing_claim_hold_attention_ms: status.revealing_claim_hold_attention_ms(),
                },
            }),
            ActorCommandOutputV1::Effect(_) => Err(ActorCommandError::StatusUnavailable),
        }
    }

    /// One Zcash actor effect (activate, drive, claim, recover) as the
    /// supervisor runs it; the actor's secret-free JSON output comes back for
    /// the effect log line and the progress snapshot.
    #[cfg(feature = "pair-zec")]
    pub(super) async fn zec_effect(
        &self,
        command: zec_reference_actor::ActorCommand,
    ) -> Result<serde_json::Value, zec_reference_actor::ActorCommandError> {
        use zec_reference_actor::{ActorCommandError, ActorCommandOutputV1, execute_actor_command};
        let Self::Zec(config) = self else {
            return Err(ActorCommandError::DriveUnavailable);
        };
        match execute_actor_command(config, command).await? {
            ActorCommandOutputV1::Effect(effect) => {
                serde_json::to_value(effect).map_err(|_| ActorCommandError::DriveUnavailable)
            }
            ActorCommandOutputV1::Status(_) => Err(ActorCommandError::DriveUnavailable),
        }
    }

    /// Runs one terminal effect: the pair decides which actor command it is.
    async fn effect(&self, action: TakerTerminalActionV1) -> Result<(), ()> {
        match self {
            Self::Btc { config, .. } => {
                use btc_reference_actor::{
                    ActorCommand, ActorCommandOutputV1, execute_actor_command,
                };
                // The BTC actor selects the concrete transition from its durable
                // revision: a Taker "claim" is one bounded drive.
                let command = match action {
                    TakerTerminalActionV1::Claim => ActorCommand::Drive,
                    TakerTerminalActionV1::Refund => ActorCommand::Recover,
                };
                match execute_actor_command(config, command).await {
                    Ok(ActorCommandOutputV1::Effect(_)) => Ok(()),
                    Ok(ActorCommandOutputV1::Status(_)) => Err(()),
                    Err(error) => {
                        report_actor_failure("effect", &error);
                        Err(())
                    }
                }
            }
            #[cfg(feature = "pair-zec")]
            Self::Zec(config) => {
                use zec_reference_actor::{
                    ActorCommand, ActorCommandOutputV1, execute_actor_command,
                };
                let command = match action {
                    TakerTerminalActionV1::Claim => ActorCommand::Claim,
                    TakerTerminalActionV1::Refund => ActorCommand::Recover,
                };
                match execute_actor_command(config, command).await {
                    Ok(ActorCommandOutputV1::Effect(_)) => Ok(()),
                    Ok(ActorCommandOutputV1::Status(_)) => Err(()),
                    Err(error) => {
                        report_actor_failure("effect", &error);
                        Err(())
                    }
                }
            }
        }
    }

    /// One bounded observation drive of the BTC actor in a phase where the
    /// Taker has no effect to make (its own lock, the Maker's lock). Never
    /// called at `BothLegsLocked`, where a drive would be the revealing claim.
    async fn observe(&self) -> Result<(), ()> {
        match self {
            Self::Btc { config, .. } => {
                use btc_reference_actor::{ActorCommand, execute_actor_command};
                execute_actor_command(config, ActorCommand::Drive)
                    .await
                    .map(|_| ())
                    .map_err(|error| report_actor_failure("observe", &error))
            }
            #[cfg(feature = "pair-zec")]
            Self::Zec(_) => Ok(()),
        }
    }

    /// Activates a freshly provisioned actor; a replay is not an error.
    async fn activate(&self) -> Result<(), ()> {
        match self {
            Self::Btc { config, .. } => {
                use btc_reference_actor::{ActorCommand, execute_actor_command};
                execute_actor_command(config, ActorCommand::Activate)
                    .await
                    .map(|_| ())
                    .map_err(|error| report_actor_failure("activate", &error))
            }
            #[cfg(feature = "pair-zec")]
            Self::Zec(_) => Ok(()),
        }
    }
}

/// Maps the BTC actor's durable phase to what the Taker desk may do next.
///
/// In `TakerSellsForeign` the Taker locks Bitcoin first, then claims LEZ with
/// the revealing claim once both legs are locked; the Maker's follow-up claim
/// completes the swap. Refund is the Taker-leg recovery the actor exposes.
fn btc_actor_progress(
    phase: Phase,
    next_action: btc_reference_actor::ActorNextActionV1,
) -> (TakerSwapStateV1, Option<TakerTerminalActionV1>) {
    use btc_reference_actor::ActorNextActionV1;
    match phase {
        Phase::Offered | Phase::AwaitingTakerConfirmations => {
            (TakerSwapStateV1::AwaitingFirstLock, None)
        }
        // The refund is offered once the actor routes to recovery, which it
        // does past the Maker's second-lock cutoff; before that the Maker's
        // lock is still awaited and nothing is offered.
        Phase::TakerLockConfirmed | Phase::AwaitingMakerConfirmations
            if next_action == ActorNextActionV1::RecoverTakerLeg =>
        {
            (
                TakerSwapStateV1::AwaitingSecondLock,
                Some(TakerTerminalActionV1::Refund),
            )
        }
        Phase::TakerLockConfirmed | Phase::AwaitingMakerConfirmations => {
            (TakerSwapStateV1::AwaitingSecondLock, None)
        }
        // The claim window has closed: a claim can no longer land, so the Node
        // follows the Maker's refund and offers the Taker's own afterwards.
        Phase::BothLegsLocked if next_action == ActorNextActionV1::RecoverMakerLeg => {
            (TakerSwapStateV1::BothLegsLocked, None)
        }
        Phase::BothLegsLocked => (
            TakerSwapStateV1::ClaimAvailable,
            Some(TakerTerminalActionV1::Claim),
        ),
        Phase::ClaimEvidenceAvailable => (TakerSwapStateV1::ClaimInProgress, None),
        Phase::MakerLegRefunded if next_action == ActorNextActionV1::RecoverTakerLeg => (
            TakerSwapStateV1::RefundAvailable,
            Some(TakerTerminalActionV1::Refund),
        ),
        Phase::TakerLegRefunded => (TakerSwapStateV1::RefundInProgress, None),
        Phase::Completed => (TakerSwapStateV1::Completed, None),
        Phase::Refunded => (TakerSwapStateV1::Refunded, None),
        Phase::TakerLockReorged
        | Phase::MakerLockReorged
        | Phase::MakerLegRefunded
        | Phase::MakerRecoveryAvailable => (TakerSwapStateV1::AttentionRequired, None),
    }
}

const INITIATION_CONFLICT_CODE: i32 = -32_013;
const SWAP_NOT_FOUND_CODE: i32 = -32_014;
const PROGRESS_GENERATION_CONFLICT_CODE: i32 = -32_015;
const ACTION_UNAVAILABLE_CODE: i32 = -32_016;
const ACTION_CONFLICT_CODE: i32 = -32_017;
const FUNDING_REQUIRED_CODE: i32 = -32_018;
const MAXIMUM_MONITORED_SWAPS: usize = 256;
const MAXIMUM_PREPARED_INPUT_BYTES: u64 = 256 * 1024;
const SIGNING_KEY_BYTES: u64 = 32;

/// Registers the prepared-ZEC lifecycle methods when a validated catalog exists.
pub(super) fn register(
    module: &mut RpcModule<()>,
    state: &Arc<TakerServiceState>,
) -> Result<(), RegisterMethodError> {
    if state.initiation.is_none() {
        return Ok(());
    }
    spawn_dynamic_observer(Arc::clone(state));
    #[cfg(feature = "pair-zec")]
    super::zec_supervisor::spawn_zec_supervisor(Arc::clone(state));
    let balances_state = Arc::clone(state);
    module.register_async_method("taker_wallet_balances_v1", move |params, _, _| {
        let state = Arc::clone(&balances_state);
        async move {
            let request: TakerHealthRequestV1 = params
                .one()
                .map_err(|_| rpc_error(INVALID_PARAMS_CODE, "Invalid params", "invalid_params"))?;
            request.validate_schema_version().map_err(|_| {
                rpc_error(
                    INVALID_PARAMS_CODE,
                    "Invalid params",
                    "unsupported_schema_version",
                )
            })?;
            let initiation = state.initiation.clone().ok_or_else(|| {
                rpc_error(
                    DEPENDENCY_UNAVAILABLE_CODE,
                    "Taker dependency unavailable",
                    "initiation_registry_unavailable",
                )
            })?;
            let dynamic = tokio::task::spawn_blocking(move || {
                initiation
                    .lock()
                    .ok()
                    .and_then(|context| context.dynamic_btc())
            })
            .await
            .map_err(|_| {
                rpc_error(
                    INTERNAL_ERROR_CODE,
                    "Internal error",
                    "initiation_registry_unavailable",
                )
            })?
            .ok_or_else(|| {
                rpc_error(
                    DEPENDENCY_UNAVAILABLE_CODE,
                    "Taker dependency unavailable",
                    "balances_unavailable",
                )
            })?;
            Ok::<_, ErrorObjectOwned>(dynamic.wallet_balances().await)
        }
    })?;
    let list_state = Arc::clone(state);
    module.register_async_method("taker_swap_list_v1", move |params, _, _| {
        let state = Arc::clone(&list_state);
        async move {
            let request: TakerSwapListRequestV1 = params
                .one()
                .map_err(|_| rpc_error(INVALID_PARAMS_CODE, "Invalid params", "invalid_params"))?;
            list_swaps(state, request)
                .await
                .map_err(|error| map_monitoring_error(&error))
        }
    })?;

    let monitor_state = Arc::clone(state);
    module.register_async_method("taker_swap_monitor_v1", move |params, _, _| {
        let state = Arc::clone(&monitor_state);
        async move {
            let request: TakerSwapMonitorRequestV1 = params
                .one()
                .map_err(|_| rpc_error(INVALID_PARAMS_CODE, "Invalid params", "invalid_params"))?;
            monitor_swap(state, request)
                .await
                .map_err(|error| map_monitoring_error(&error))
        }
    })?;

    let initiate_state = Arc::clone(state);
    module.register_async_method("taker_swap_initiate_v1", move |params, _, _| {
        let state = Arc::clone(&initiate_state);
        async move {
            let request: TakerSwapInitiateRequestV1 = params
                .one()
                .map_err(|_| rpc_error(INVALID_PARAMS_CODE, "Invalid params", "invalid_params"))?;
            Box::pin(initiate(state, request))
                .await
                .map_err(map_initiation_error)
        }
    })?;

    register_terminal_methods(module, state)?;
    Ok(())
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
            Box::pin(terminal_action(
                state,
                request.request_id,
                request.swap_id,
                request.expected_generation,
                TakerTerminalActionV1::Claim,
            ))
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
            Box::pin(terminal_action(
                state,
                request.request_id,
                request.swap_id,
                request.expected_generation,
                TakerTerminalActionV1::Refund,
            ))
            .await
            .map_err(map_action_error)
        }
    })?;

    let lock_state = Arc::clone(state);
    module.register_async_method("taker_swap_lock_v1", move |params, _, _| {
        let state = Arc::clone(&lock_state);
        async move {
            let request: TakerLockRequestV1 = params
                .one()
                .map_err(|_| rpc_error(INVALID_PARAMS_CODE, "Invalid params", "invalid_params"))?;
            lock_swap(state, request).await
        }
    })?;
    Ok(())
}

#[derive(Clone, Copy, Debug)]
pub(super) enum ActionError {
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
        // The replay re-drives the actor against the generation the request was
        // admitted at, not the caller's: a refund replay is admitted through the
        // leg-refunded phases only relative to that one.
        if replay_actor_effect_is_required(
            &config,
            &held_lock,
            action,
            admission.requested_after_generation(),
        )
        .await?
        {
            let effect_config =
                reload_action_actor_custody(&prepared, receipt_binding, &config, &held_lock)
                    .await?;
            execute_terminal_actor_command(&effect_config, &held_lock, action).await?;
            revalidate_action_custody(&prepared, receipt_binding, &effect_config, &held_lock)
                .await?;
        } else {
            revalidate_action_custody(&prepared, receipt_binding, &config, &held_lock).await?;
        }
        return Ok(action_commit(&admission));
    }
    // One admitted action per swap. An earlier admission for a different
    // action at an earlier generation is stale once the actor offers this one
    // at a later generation: a claim whose window closed before it landed,
    // followed by the refund. It is retired below, only after the actor's
    // status confirms the new action is what it offers now.
    let superseded = match lookup_admitted_action_for_swap(&initiation, &swap_id).await? {
        None => None,
        Some(existing)
            if existing.action() != facade_action(action)
                && existing.requested_after_generation() < expected_generation =>
        {
            Some(existing)
        }
        Some(_) => return Err(ActionError::Conflict),
    };

    let status = config
        .status()
        .await
        .map_err(|()| ActionError::DependencyUnavailable)?;
    held_lock
        .validate_for_state(config.swap_id(), config.state_db())
        .map_err(|_| ActionError::DependencyUnavailable)?;
    let LifecycleStatus::Active {
        phase,
        revision,
        state,
        available_action,
        ..
    } = status
    else {
        return Err(ActionError::Unavailable);
    };
    if revision != expected_generation {
        return Err(ActionError::ProgressChanged);
    }
    if available_action != Some(action)
        && !route_admits_claim_window_refund(&config, phase, state, action)
    {
        return Err(ActionError::Unavailable);
    }
    if let Some(superseded) = superseded {
        retire_superseded_action(&initiation, &superseded).await?;
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
    // Actor status replay can legitimately update SQLite metadata. Refresh the
    // receipt-bound file identities under the still-held swap lock before secrets.
    let effect_config =
        reload_action_actor_custody(&prepared, receipt_binding, &config, &held_lock).await?;
    let gate_open = admission_claim_gate_open(&initiation, &prepared);
    let first_attempt =
        gated_first_attempt(&effect_config, &held_lock, action, phase, gate_open).await;
    first_attempt_reply(effect_config.is_zec(), first_attempt)?;
    revalidate_action_custody(&prepared, receipt_binding, &effect_config, &held_lock).await?;
    Ok(action_commit(&admission))
}

async fn resolve_action_authority(
    initiation: &Arc<Mutex<ConfiguredTakerInitiationContext>>,
    swap_id: &SwapId,
) -> Result<
    (
        PreparedTakerInitiationV1,
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
            .prepared_for_swap(&swap_id)
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

pub(super) async fn load_bound_action_actor(
    prepared: &PreparedTakerInitiationV1,
    receipt_binding: crate::taker_service_config::PreparedReceiptBindingV1,
) -> Result<(RouteActor, ActorHeldLock), ActionError> {
    let pair = prepared.facts().route().pair();
    let receipt = prepared.execution().receipt_output().to_path_buf();
    let actor_root = prepared.execution().actor_root().to_path_buf();
    let swap_id = prepared.swap_id().clone();
    let receipt_sha256 = receipt_binding.sha256();
    let receipt_identity = receipt_binding.identity();
    tokio::task::spawn_blocking(move || {
        let load = || {
            RouteActor::load_for_monitor(
                pair,
                &receipt,
                &actor_root,
                &swap_id,
                receipt_sha256,
                receipt_identity.device(),
                receipt_identity.inode(),
            )
            .map_err(|_| ActionError::DependencyUnavailable)
        };
        let before = load()?;
        let held_lock =
            ActorHeldLock::acquire_for_within(before.swap_id(), before.state_db(), LOCK_WAIT)
                .map_err(|_| ActionError::DependencyUnavailable)?;
        let config = load()?;
        if before.custody_identity() != config.custody_identity() {
            return Err(ActionError::DependencyUnavailable);
        }
        held_lock
            .validate_for_state(config.swap_id(), config.state_db())
            .map_err(|_| ActionError::DependencyUnavailable)?;
        Ok((config, held_lock))
    })
    .await
    .map_err(|_| ActionError::DependencyUnavailable)?
}

pub(super) async fn reload_action_actor_custody(
    prepared: &PreparedTakerInitiationV1,
    receipt_binding: crate::taker_service_config::PreparedReceiptBindingV1,
    config: &RouteActor,
    held_lock: &ActorHeldLock,
) -> Result<RouteActor, ActionError> {
    let pair = prepared.facts().route().pair();
    let receipt = prepared.execution().receipt_output().to_path_buf();
    let actor_root = prepared.execution().actor_root().to_path_buf();
    let swap_id = prepared.swap_id().clone();
    let receipt_sha256 = receipt_binding.sha256();
    let receipt_identity = receipt_binding.identity();
    let after = tokio::task::spawn_blocking(move || {
        RouteActor::load_for_monitor(
            pair,
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
    if after.custody_identity() != config.custody_identity() {
        return Err(ActionError::DependencyUnavailable);
    }
    held_lock
        .validate_for_state(config.swap_id(), config.state_db())
        .map_err(|_| ActionError::DependencyUnavailable)?;
    Ok(after)
}

async fn revalidate_action_custody(
    prepared: &PreparedTakerInitiationV1,
    receipt_binding: crate::taker_service_config::PreparedReceiptBindingV1,
    config: &RouteActor,
    held_lock: &ActorHeldLock,
) -> Result<(), ActionError> {
    reload_action_actor_custody(prepared, receipt_binding, config, held_lock)
        .await
        .map(|_| ())
}

async fn replay_actor_effect_is_required(
    config: &RouteActor,
    held_lock: &ActorHeldLock,
    action: TakerTerminalActionV1,
    expected_generation: u64,
) -> Result<bool, ActionError> {
    let status = config
        .status()
        .await
        .map_err(|()| ActionError::DependencyUnavailable)?;
    held_lock
        .validate_for_state(config.swap_id(), config.state_db())
        .map_err(|_| ActionError::DependencyUnavailable)?;
    if config.is_zec() {
        zec_replay_actor_effect_required(
            status,
            expected_generation,
            action,
            revealing_claim_was_submitted(config.state_db()),
        )
    } else {
        replay_actor_effect_required(status, expected_generation, action)
    }
}

/// Selling ZEC, both legs locked is the Taker's claim window: the Node offers
/// the revealing LEZ claim. A Taker that does not claim must still reach its
/// Zcash refund, and the supervisor never drives there (a drive would be the
/// claim), so a refund request is admitted at this shape too and runs the
/// agreement-ordered recovery: the actor waits for the Maker's LEZ refund and
/// the Zcash refund height before it submits. Zcash route only; in ZEC the
/// pair (`BothLegsLocked`, `ClaimAvailable`) occurs only in `TakerSellsForeign`.
/// Never once a revealing claim of this swap was submitted (`claim_submitted`:
/// its marker exists): the secret may be public and the Maker could take the
/// Zcash, so the claim must be pursued (re-planned if its nonce was taken),
/// never traded for the refund (FINDINGS 24).
pub(super) const fn zec_claim_window_admits_refund(
    phase: Phase,
    state: TakerSwapStateV1,
    action: TakerTerminalActionV1,
    claim_submitted: bool,
) -> bool {
    !claim_submitted
        && matches!(action, TakerTerminalActionV1::Refund)
        && matches!(phase, Phase::BothLegsLocked)
        && matches!(state, TakerSwapStateV1::ClaimAvailable)
}

/// The Zcash route's claim-window refund for this actor (never on Bitcoin,
/// never after a revealing claim was submitted).
fn route_admits_claim_window_refund(
    config: &RouteActor,
    phase: Phase,
    state: TakerSwapStateV1,
    action: TakerTerminalActionV1,
) -> bool {
    config.is_zec()
        && zec_claim_window_admits_refund(
            phase,
            state,
            action,
            revealing_claim_was_submitted(config.state_db()),
        )
}

/// Whether this swap's actor has ever submitted a revealing LEZ claim.
pub(super) fn revealing_claim_was_submitted(state_db: &Path) -> bool {
    revealing_claim_submitted_marker(state_db).is_some_and(|marker| marker.is_file())
}

/// `replay_actor_effect_required` for the Zcash route, which also admits the
/// refund of its claim window ([`zec_claim_window_admits_refund`]).
pub(super) fn zec_replay_actor_effect_required(
    status: LifecycleStatus,
    expected_generation: u64,
    action: TakerTerminalActionV1,
    claim_submitted: bool,
) -> Result<bool, ActionError> {
    if let LifecycleStatus::Active {
        phase,
        revision,
        state,
        ..
    } = status
        && revision == expected_generation
        && zec_claim_window_admits_refund(phase, state, action, claim_submitted)
    {
        return Ok(true);
    }
    replay_actor_effect_required(status, expected_generation, action)
}

pub(super) fn replay_actor_effect_required(
    status: LifecycleStatus,
    expected_generation: u64,
    action: TakerTerminalActionV1,
) -> Result<bool, ActionError> {
    let LifecycleStatus::Active {
        phase,
        revision,
        available_action,
        ..
    } = status
    else {
        return Err(ActionError::DependencyUnavailable);
    };
    if revision > expected_generation {
        return Ok(action == TakerTerminalActionV1::Refund
            && matches!(phase, Phase::MakerLegRefunded | Phase::TakerLegRefunded));
    }
    if revision < expected_generation {
        return Err(ActionError::ProgressChanged);
    }
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

pub(super) async fn lookup_admitted_action_for_swap(
    initiation: &Arc<Mutex<ConfiguredTakerInitiationContext>>,
    swap_id: &SwapId,
) -> Result<Option<TakerActionAdmissionV1>, ActionError> {
    let context = Arc::clone(initiation);
    let swap_id = swap_id.clone();
    tokio::task::spawn_blocking(move || {
        let mut context = context
            .lock()
            .map_err(|_| ActionError::RegistryUnavailable)?;
        context
            .registry_mut()
            .lookup_action_for_swap(&swap_id)
            .map_err(map_action_store_error)
    })
    .await
    .map_err(|_| ActionError::RegistryUnavailable)?
}

async fn retire_superseded_action(
    initiation: &Arc<Mutex<ConfiguredTakerInitiationContext>>,
    stale: &TakerActionAdmissionV1,
) -> Result<(), ActionError> {
    let context = Arc::clone(initiation);
    let stale = stale.clone();
    tokio::task::spawn_blocking(move || {
        let mut context = context
            .lock()
            .map_err(|_| ActionError::RegistryUnavailable)?;
        context
            .registry_mut()
            .retire_superseded_action(&stale)
            .map(|_| ())
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
        let admission = context
            .registry_mut()
            .admit_action(
                &request_id,
                &swap_id,
                facade_action(action),
                expected_generation,
                admitted_at,
            )
            .map_err(map_action_store_error)?;
        // The owner's action runs now, not after the row's backoff; a failed
        // row is requeued by it, as on the Maker.
        #[cfg(feature = "pair-zec")]
        if let Err(error) = context
            .schedule_mut()
            .requeue_for_action(swap_id.as_str(), admitted_at)
        {
            eprintln!(
                "taker actor {}: schedule requeue after the admitted {action:?} failed: {error}",
                swap_id.as_str()
            );
        }
        Ok(admission)
    })
    .await
    .map_err(|_| ActionError::RegistryUnavailable)?
}

/// What the caller hears about the first attempt of an admitted action. A
/// Zcash action is admitted durably and the Node's own supervisor executes it
/// from here on (its row names the class while it backs off): a refusal of
/// this first attempt, right after the status said the action is available,
/// is a transient of the chains (the funding observation reads unstable when
/// a block lands between its two tip samples) and the supervisor's to retry,
/// so the caller gets the admission. A Bitcoin action is executed here and
/// its failure is the caller's; every other error stays an error for both.
fn first_attempt_reply(
    pair_is_zec: bool,
    attempt: Result<(), ActionError>,
) -> Result<(), ActionError> {
    match attempt {
        Err(ActionError::DependencyUnavailable) if pair_is_zec => {
            report_actor_failure(
                "first attempt after admission",
                &"retried by the supervisor",
            );
            Ok(())
        }
        other => other,
    }
}

/// The Zcash claim gate (one revealing LEZ claim in flight per Node,
/// FINDINGS 24) as the admission sees it; always open without the pair.
#[cfg(feature = "pair-zec")]
fn admission_claim_gate_open(
    initiation: &Arc<Mutex<ConfiguredTakerInitiationContext>>,
    prepared: &PreparedTakerInitiationV1,
) -> bool {
    super::zec_supervisor::lez_claim_gate_open(initiation, prepared)
}

#[cfg(not(feature = "pair-zec"))]
const fn admission_claim_gate_open(
    _initiation: &Arc<Mutex<ConfiguredTakerInitiationContext>>,
    _prepared: &PreparedTakerInitiationV1,
) -> bool {
    true
}

/// The admission's first attempt. Selling ZEC the claim at both legs locked
/// is a revealing LEZ claim: while another swap's is in flight it is not
/// attempted here (the admission is answered and the Node's supervisor
/// submits it once the first is final, FINDINGS 24).
async fn gated_first_attempt(
    config: &RouteActor,
    held_lock: &ActorHeldLock,
    action: TakerTerminalActionV1,
    phase: Phase,
    gate_open: bool,
) -> Result<(), ActionError> {
    let lez_claim =
        config.is_zec() && action == TakerTerminalActionV1::Claim && phase == Phase::BothLegsLocked;
    if lez_claim && !gate_open {
        report_actor_failure(
            "first attempt after admission",
            &"another swap's revealing LEZ claim is in flight; the supervisor submits this one next",
        );
        return Ok(());
    }
    execute_terminal_actor_command(config, held_lock, action).await
}

async fn execute_terminal_actor_command(
    config: &RouteActor,
    held_lock: &ActorHeldLock,
    action: TakerTerminalActionV1,
) -> Result<(), ActionError> {
    config
        .effect(action)
        .await
        .inspect(|()| {
            if action == TakerTerminalActionV1::Claim {
                mark_claim_submitted(config.state_db());
            }
        })
        .map_err(|()| ActionError::DependencyUnavailable)?;
    held_lock
        .validate_for_state(config.swap_id(), config.state_db())
        .map_err(|error| {
            report_actor_failure("custody", &error);
            ActionError::DependencyUnavailable
        })
}

/// The marker the ZEC actor leaves beside its state when the LEZ bridge
/// refused the first lock for the depositor's balance
/// (`<swap>/actor/taker/lez-depositor-balance-insufficient`).
#[cfg(feature = "pair-zec")]
use zec_reference_actor::{
    lez_depositor_balance_marker, revealing_claim_holder, revealing_claim_submitted_marker,
};
/// Without the Zcash pair no revealing claim is held behind another.
#[cfg(not(feature = "pair-zec"))]
fn revealing_claim_holder(_state_db: &Path) -> Option<String> {
    None
}

/// A Zcash swap whose revealing claim the supervisor holds behind another
/// swap's as its own LEZ refund time nears: the row needs attention and names
/// the holder (the marker is removed when the claim goes ahead). The admissible
/// action stays: the owner may still refund in the claim window.
fn overlay_held_revealing_claim(view: &mut TakerSwapViewV1, state_db: &Path) {
    if matches!(
        view.state,
        TakerSwapStateV1::Completed | TakerSwapStateV1::Refunded
    ) {
        return;
    }
    if let Some(holder) = revealing_claim_holder(state_db) {
        view.state = TakerSwapStateV1::AttentionRequired;
        view.attention_reason = Some("lez_revealing_claim_held_near_refund".to_owned());
        view.revealing_claim_held_by = Some(holder);
    }
}
/// Without the Zcash pair no actor submits a revealing LEZ claim.
#[cfg(not(feature = "pair-zec"))]
fn revealing_claim_submitted_marker(_state_db: &Path) -> Option<PathBuf> {
    None
}
/// Without the Zcash pair no actor leaves the marker.
#[cfg(not(feature = "pair-zec"))]
fn lez_depositor_balance_marker(_state_db: &Path) -> Option<PathBuf> {
    None
}

/// The marker a Node-owned Bitcoin swap keeps once its revealing claim was
/// submitted, so the observer may drive `BothLegsLocked` afterwards (the
/// drive then only observes the claim) but never before (it would claim).
fn claim_submitted_marker(state_db: &Path) -> Option<PathBuf> {
    Some(state_db.parent()?.parent()?.join("claim-submitted"))
}

pub(super) fn mark_claim_submitted(state_db: &Path) {
    if let Some(marker) = claim_submitted_marker(state_db) {
        let _ = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(marker);
    }
}

/// Names one failed actor step on stderr so `docker compose logs` explains
/// what the RPC caller only sees as `taker_action_execution_unavailable`.
/// Only the error's variant name is printed, never its payload.
pub(super) fn report_actor_failure(step: &str, error: &dyn std::fmt::Debug) {
    let rendered = format!("{error:?}");
    let class = rendered.split(['(', '{', ' ']).next().unwrap_or_default();
    eprintln!(
        "{}",
        json!({"event": "taker_actor_command_failed", "step": step, "class": class})
    );
}

const fn facade_action(action: TakerTerminalActionV1) -> TakerFacadeActionV1 {
    match action {
        TakerTerminalActionV1::Claim => TakerFacadeActionV1::Claim,
        TakerTerminalActionV1::Refund => TakerFacadeActionV1::Refund,
    }
}

pub(super) const fn terminal_action_of(action: TakerFacadeActionV1) -> TakerTerminalActionV1 {
    match action {
        TakerFacadeActionV1::Claim => TakerTerminalActionV1::Claim,
        TakerFacadeActionV1::Refund => TakerTerminalActionV1::Refund,
    }
}

fn action_commit(admission: &TakerActionAdmissionV1) -> TakerActionCommitV1 {
    TakerActionCommitV1 {
        schema_version: 1,
        swap_id: admission.swap_id().clone(),
        action: terminal_action_of(admission.action()),
        requested_after_generation: admission.requested_after_generation(),
        was_replay: admission.was_replay(),
    }
}

fn map_action_store_error(error: TakerFacadeStoreError) -> ActionError {
    match error {
        TakerFacadeStoreError::RequestConflict
        | TakerFacadeStoreError::ActionGenerationConflict => ActionError::Conflict,
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

#[derive(Clone, Debug)]
enum MonitoringError {
    UnsupportedSchemaVersion,
    NotFound,
    /// The swap's own bundle (receipt, actor configuration) no longer passes
    /// the checks it was bound with; the swap needs attention, the Node is fine.
    BundleInvalid,
    /// The swap's actor is held by the supervisor (a drive in flight); the
    /// path is the actor's state database, for the marker read.
    ActorBusy(
        #[cfg_attr(
            not(feature = "pair-zec"),
            expect(
                dead_code,
                reason = "the Zcash supervisor's snapshot path reads the state database"
            )
        )]
        PathBuf,
    ),
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
        Ok::<_, MonitoringError>(facts)
    })
    .await
    .map_err(|_| MonitoringError::RegistryUnavailable)??;

    // One swap whose bundle is gone (an aborted take, a retired swap
    // directory) must not hide every other swap: it is listed as needing
    // attention and the desk can still inspect it. Every other failure (a
    // dependency or state store that cannot answer) still fails the list, so
    // a degraded Node is reported as degraded rather than as a row.
    let mut swaps = Vec::with_capacity(swap_ids.len());
    for facts in swap_ids {
        match project_swap(&initiation, facts.swap_id()).await {
            Ok(view) => swaps.push(view),
            Err(MonitoringError::NotFound) => swaps
                .push(commit_from_facts(&facts, false, TakerSwapStateV1::AttentionRequired).swap),
            Err(error) => return Err(error),
        }
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
    let lookup_id = swap_id.clone();
    let (prepared, facts, receipt_binding) = tokio::task::spawn_blocking(move || {
        let mut context = lookup
            .lock()
            .map_err(|_| MonitoringError::RegistryUnavailable)?;
        let prepared = context
            .prepared_for_swap(&lookup_id)
            .cloned()
            .ok_or(MonitoringError::NotFound)?;
        let facts = context
            .registry_mut()
            .lookup_initiation_for_monitor(&lookup_id, prepared.authority())
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
            match project_receipt_bound_swap(initiation, &prepared, &facts, receipt_binding).await {
                // The bundle the swap was bound with (its receipt, its copied
                // actor configuration) fails the checks it was pinned by: an
                // altered or damaged copy. That swap needs attention and
                // cannot be acted on; it must not read as a Node outage, nor
                // hide every other swap from the list.
                Err(MonitoringError::BundleInvalid) => {
                    Ok(commit_from_facts(&facts, false, TakerSwapStateV1::AttentionRequired).swap)
                }
                // The supervisor holds the actor: the row is what its last
                // attempt observed (the Maker's progress-snapshot pattern).
                #[cfg(feature = "pair-zec")]
                Err(MonitoringError::ActorBusy(state_db))
                    if prepared.facts().route().pair() == Pair::Zcash =>
                {
                    let record = super::zec_supervisor::schedule_record(initiation, &swap_id).await;
                    match record.as_ref().and_then(|record| record.progress()) {
                        // A snapshot older than the bound is not answered: a
                        // stale claim_available would mislead the desk.
                        Some(snapshot) if super::zec_supervisor::snapshot_is_fresh(snapshot) => {
                            let mut view =
                                super::zec_supervisor::view_from_snapshot(&facts, snapshot);
                            if view.state == TakerSwapStateV1::AwaitingFirstLock
                                && lez_depositor_balance_marker(&state_db)
                                    .is_some_and(|marker| marker.is_file())
                            {
                                view.state = TakerSwapStateV1::AttentionRequired;
                                view.available_action = None;
                                view.attention_reason =
                                    Some("lez_depositor_balance_insufficient".to_owned());
                            }
                            overlay_held_revealing_claim(&mut view, &state_db);
                            super::zec_supervisor::overlay_schedule(&mut view, record.as_ref());
                            let admitted = lookup_monitored_action(initiation, &swap_id).await?;
                            overlay_admitted_action(view, admitted.as_ref())
                        }
                        _ => Err(MonitoringError::DependencyUnavailable),
                    }
                }
                Err(MonitoringError::ActorBusy(_)) => Err(MonitoringError::DependencyUnavailable),
                other => other,
            }
        }
    }
}

async fn project_receipt_bound_swap(
    initiation: &Arc<Mutex<ConfiguredTakerInitiationContext>>,
    prepared: &PreparedTakerInitiationV1,
    facts: &TakerInitiationFactsV1,
    receipt_binding: crate::taker_service_config::PreparedReceiptBindingV1,
) -> Result<TakerSwapViewV1, MonitoringError> {
    let pair = prepared.facts().route().pair();
    let receipt = prepared.execution().receipt_output().to_path_buf();
    let actor_root = prepared.execution().actor_root().to_path_buf();
    let swap_id = prepared.swap_id().clone();
    let receipt_sha256 = receipt_binding.sha256();
    let receipt_identity = receipt_binding.identity();
    let (config, held_lock, terms, effects, lock_fee) = tokio::task::spawn_blocking(move || {
        // Loading reads and re-verifies the swap's own files against the
        // binding the take recorded; it fails only on that bundle, never on a
        // dependency, so its failure is the swap's, not the Node's.
        let load = || {
            RouteActor::load_for_monitor(
                pair,
                &receipt,
                &actor_root,
                &swap_id,
                receipt_sha256,
                receipt_identity.device(),
                receipt_identity.inode(),
            )
            .map_err(|error| {
                // Named once per failed load so an operator can tell an altered
                // bundle from a disk that cannot be read (both read as the
                // swap needing attention).
                eprintln!(
                    "taker monitor: swap {} bundle failed its checks: {error:#}",
                    swap_id.as_str()
                );
                MonitoringError::BundleInvalid
            })
        };
        let before = load()?;
        // A Zcash actor is driven by the Node's supervisor, which holds its
        // lock for a whole drive; a read does not wait that out, it answers
        // from the supervisor's last snapshot instead (project_swap).
        let wait = if pair == Pair::Zcash {
            MONITOR_LOCK_WAIT_SUPERVISED
        } else {
            LOCK_WAIT
        };
        let held_lock =
            ActorHeldLock::acquire_for_within(before.swap_id(), before.state_db(), wait)
                .map_err(|_| MonitoringError::ActorBusy(before.state_db().to_path_buf()))?;
        let config = load()?;
        if before.custody_identity() != config.custody_identity() {
            return Err(MonitoringError::DependencyUnavailable);
        }
        held_lock
            .validate_for_state(config.swap_id(), config.state_db())
            .map_err(|_| MonitoringError::DependencyUnavailable)?;
        let terms = config.agreement_terms();
        let effects = config.effects();
        let lock_fee = config.bitcoin_lock_fee_sat();
        Ok::<_, MonitoringError>((config, held_lock, terms, effects, lock_fee))
    })
    .await
    .map_err(|_| MonitoringError::DependencyUnavailable)??;

    let admitted_action = lookup_monitored_action(initiation, config.swap_id()).await?;
    let status = config
        .status()
        .await
        .map_err(|()| MonitoringError::DependencyUnavailable)?;
    held_lock
        .validate_for_state(config.swap_id(), config.state_db())
        .map_err(|_| MonitoringError::DependencyUnavailable)?;
    let mut view = view_from_actor_status(facts, status);
    view.terms = terms;
    view.effects = effects;
    view.bitcoin_lock_fee_sat = lock_fee;
    // A Zcash swap whose LEZ depositor cannot fund the lot: the actor leaves a
    // marker beside its state when the bridge refuses the first lock for the
    // balance, and the row reads as needing attention with that reason until
    // the account is funded (the marker goes when a preparation succeeds).
    if pair == Pair::Zcash
        && view.state == TakerSwapStateV1::AwaitingFirstLock
        && lez_depositor_balance_marker(config.state_db()).is_some_and(|marker| marker.is_file())
    {
        view.state = TakerSwapStateV1::AttentionRequired;
        view.available_action = None;
        view.attention_reason = Some("lez_depositor_balance_insufficient".to_owned());
    }
    if pair == Pair::Zcash {
        overlay_held_revealing_claim(&mut view, config.state_db());
    }
    #[cfg(feature = "pair-zec")]
    if pair == Pair::Zcash {
        let record = super::zec_supervisor::schedule_record(initiation, config.swap_id()).await;
        super::zec_supervisor::overlay_schedule(&mut view, record.as_ref());
    }
    overlay_admitted_action(view, admitted_action.as_ref())
}

/// How long a read waits for a supervised Zcash actor's lock before it
/// answers from the supervisor's snapshot.
const MONITOR_LOCK_WAIT_SUPERVISED: std::time::Duration = std::time::Duration::from_secs(2);

/// Reads `fee_sat` from the funding plan the take wrote beside the actor's
/// state (`<swap>/actor/state.sqlite3` and `<swap>/bitcoin/funding-plan.json`).
fn planned_lock_fee_sat(state_db: &Path) -> Option<u64> {
    let plan = state_db
        .parent()?
        .parent()?
        .join("bitcoin")
        .join("funding-plan.json");
    let plan: serde_json::Value = serde_json::from_slice(&std::fs::read(plan).ok()?).ok()?;
    plan.get("fee_sat")?.as_u64().filter(|fee| *fee > 0)
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
    // An admitted action shows as "in progress" only while the actor still
    // offers it at the generation it was requested after. Once the actor has
    // moved on, or stopped offering it (a claim whose window closed), the
    // admission is history and the actor's own view rules, so the refund the
    // actor offers next is not hidden behind a claim that can never land.
    let still_offered = admission.requested_after_generation() == view.progress_generation
        && view.available_action.map(facade_action) == Some(admission.action());
    if !still_offered
        || matches!(
            view.state,
            TakerSwapStateV1::Completed | TakerSwapStateV1::Refunded
        )
    {
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
    status: LifecycleStatus,
) -> TakerSwapViewV1 {
    let (state, progress_generation, available_action, privacy_guidance) = match status {
        LifecycleStatus::NotActivated => (TakerSwapStateV1::NotActivated, 0, None, None),
        LifecycleStatus::Active {
            revision,
            state,
            available_action,
            privacy_guidance,
            ..
        } => (state, revision, available_action, privacy_guidance),
    };
    // The shielding guidance is for a Taker that received transparent Zcash:
    // selling LEZ. A Taker that sold Zcash received LEZ and has nothing to shield.
    let privacy_guidance =
        privacy_guidance.filter(|_| facts.route().direction() == SwapDirection::TakerSellsLez);
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
        attention_reason: None,
        revealing_claim_held_by: None,
        last_failure_class: None,
        observed_at: None,
        terms: None,
        effects: Vec::new(),
        bitcoin_lock_fee_sat: None,
    }
}

/// Maps the ZEC actor's durable phase to what the Taker desk may do next.
#[cfg(feature = "pair-zec")]
pub(super) fn zec_actor_progress(
    phase: Phase,
    next_action: ZecLifecycleAction,
) -> (
    TakerSwapStateV1,
    Option<TakerTerminalActionV1>,
    Option<TakerPrivacyGuidanceV1>,
) {
    match phase {
        Phase::Offered => (TakerSwapStateV1::AwaitingFirstLock, None, None),
        // TakerSellsForeign: the Taker is the LEZ claimant, and its revealing
        // LEZ claim is the owner's action once both legs are locked.
        Phase::BothLegsLocked if next_action == ZecLifecycleAction::ClaimLez => (
            TakerSwapStateV1::ClaimAvailable,
            Some(TakerTerminalActionV1::Claim),
            None,
        ),
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
        // The Taker's claim, whichever leg it is: the Zcash follow-up claim
        // (TakerSellsLez) or the revealing LEZ claim as first claimant
        // (TakerSellsForeign); the actor's Claim runs the agreement-ordered
        // claim for either role.
        Phase::ClaimEvidenceAvailable
            if matches!(
                next_action,
                ZecLifecycleAction::ClaimZcash | ZecLifecycleAction::ClaimLez
            ) =>
        {
            (
                TakerSwapStateV1::ClaimAvailable,
                Some(TakerTerminalActionV1::Claim),
                None,
            )
        }
        Phase::Completed => (
            TakerSwapStateV1::Completed,
            None,
            Some(TakerPrivacyGuidanceV1::ShieldReceivedTransparentZecSeparately),
        ),
        Phase::Refunded => (TakerSwapStateV1::Refunded, None, None),
        // The Taker's LEZ leg is back; the Maker's Zcash refund is still owed.
        // The refund stays offered: a replay of the admitted request (or a new
        // one at this generation) re-drives the recovery until the Taker has
        // observed that refund, as the BTC projection reads the same phase.
        Phase::TakerLegRefunded => (
            TakerSwapStateV1::RefundInProgress,
            Some(TakerTerminalActionV1::Refund),
            None,
        ),
        // The Maker's leg is back first and the Taker owns the next refund.
        Phase::MakerLegRefunded if next_action == ZecLifecycleAction::RefundZcash => (
            TakerSwapStateV1::RefundAvailable,
            Some(TakerTerminalActionV1::Refund),
            None,
        ),
        // TakerSellsForeign: the Taker's revealing LEZ claim is final and the
        // Maker's Zcash follow-up claim completes the swap; nothing to press.
        Phase::ClaimEvidenceAvailable if next_action == ZecLifecycleAction::Wait => {
            (TakerSwapStateV1::ClaimInProgress, None, None)
        }
        Phase::ClaimEvidenceAvailable | Phase::MakerLegRefunded | Phase::MakerRecoveryAvailable => {
            (TakerSwapStateV1::AttentionRequired, None, None)
        }
    }
}

fn map_monitoring_error(error: &MonitoringError) -> ErrorObjectOwned {
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
        MonitoringError::BundleInvalid => rpc_error(
            DEPENDENCY_UNAVAILABLE_CODE,
            "Taker swap bundle invalid",
            "taker_swap_bundle_invalid",
        ),
        MonitoringError::ActorBusy(_) | MonitoringError::DependencyUnavailable => rpc_error(
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

#[derive(Clone, Debug)]
enum InitiationError {
    UnsupportedSchemaVersion,
    UnsupportedPair,
    SelectionMismatch,
    Conflict,
    Backend(TakerBackendError),
    ExecutionUnavailable,
    FundingRequired(lez_btc_role_lifecycle::FundingRequired),
    Internal,
}

async fn initiate(
    state: Arc<TakerServiceState>,
    request: TakerSwapInitiateRequestV1,
) -> Result<TakerInitiationCommitV1, InitiationError> {
    request
        .validate_schema_version()
        .map_err(|_| InitiationError::UnsupportedSchemaVersion)?;
    if !route_has_node_lifecycle(request.route.pair()) {
        return Err(InitiationError::UnsupportedPair);
    }
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
            execute_prepared(&initiation, &prepared, admitted_at).await?;
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
    let (catalog_entry, dynamic) = tokio::task::spawn_blocking(move || {
        let context = selection_context
            .lock()
            .map_err(|_| InitiationError::Internal)?;
        Ok::<_, InitiationError>((
            context.prepared_for_offer(&offer_id).cloned(),
            context.dynamic_btc(),
        ))
    })
    .await
    .map_err(|_| InitiationError::Internal)??;
    let prepared = match (catalog_entry, dynamic) {
        (Some(prepared), _) => prepared,
        (None, Some(dynamic)) if request.route.pair() == Pair::Bitcoin => {
            prepare_dynamic_btc(&state, &request, &initiation, &dynamic).await?
        }
        (None, _) => return Err(InitiationError::SelectionMismatch),
    };
    if !request_matches_facts(&request, prepared.facts()) {
        return Err(InitiationError::SelectionMismatch);
    }

    let (now, live_match) = selected_offer_is_live(&state, &request).await?;
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
        execute_prepared(&initiation, &prepared, now).await?;
        bind_prepared_receipt(&initiation, prepared.swap_id()).await?;
        TakerSwapStateV1::NotActivated
    } else {
        TakerSwapStateV1::Initiating
    };
    Ok(commit_from_admission(&admission, progress))
}

/// Prepares a Bitcoin swap for any live offer at take time (ADR 0213) and
/// registers it in the catalog, so the rest of the initiation is shared.
async fn prepare_dynamic_btc(
    state: &TakerServiceState,
    request: &TakerSwapInitiateRequestV1,
    initiation: &Arc<Mutex<ConfiguredTakerInitiationContext>>,
    dynamic: &Arc<btc_dynamic::DynamicBtcRole>,
) -> Result<PreparedTakerInitiationV1, InitiationError> {
    let (now, live_match) = selected_offer_is_live(state, request).await?;
    if !live_match {
        return Err(InitiationError::SelectionMismatch);
    }
    let announcement = match request.logos_offer_announcement_base64.as_deref() {
        Some(proof_base64) => {
            let encoded = BASE64_STANDARD
                .decode(proof_base64.as_bytes())
                .map_err(|_| InitiationError::SelectionMismatch)?;
            Some(
                verify_logos_offer_announcement(&encoded, now)
                    .map_err(|_| InitiationError::SelectionMismatch)?
                    .offer()
                    .clone(),
            )
        }
        None => None,
    };
    let take = btc_dynamic::TakeRequest {
        request_id: request.request_id.clone(),
        offer_id: request.offer_id.clone(),
        route: request.route,
        maker_identity: *request.maker_identity.as_bytes(),
        signed_envelope_sha256: request.signed_envelope_sha256,
        foreign_units: request.foreign_units,
        expected_lez_units: request.expected_lez_units,
        announcement,
        funding_transaction_hex: request.funding_transaction_hex.as_deref().map(Into::into),
    };
    let prepared = btc_dynamic::prepare(dynamic, &take, now)
        .await
        .map_err(|error| match error.downcast() {
            Ok(required) => InitiationError::FundingRequired(required),
            Err(error) => {
                eprintln!("taker BTC reservation failed: {error:#}");
                InitiationError::ExecutionUnavailable
            }
        })?;
    let entry = dynamic
        .load_entry(&prepared.configured)
        .map_err(|_| InitiationError::ExecutionUnavailable)?;
    let context = Arc::clone(initiation);
    let inserted = entry.clone();
    tokio::task::spawn_blocking(move || {
        context
            .lock()
            .map_err(|_| InitiationError::Internal)?
            .insert_prepared(inserted)
            .map_err(|()| InitiationError::Conflict)
    })
    .await
    .map_err(|_| InitiationError::Internal)??;
    Ok(entry)
}

/// The Taker's own first lock for a Node-owned Bitcoin swap.
async fn lock_swap(
    state: Arc<TakerServiceState>,
    request: TakerLockRequestV1,
) -> Result<TakerLockCommitV1, ErrorObjectOwned> {
    if request.schema_version != 1 {
        return Err(rpc_error(
            INVALID_PARAMS_CODE,
            "Invalid params",
            "unsupported_schema_version",
        ));
    }
    let initiation = state.initiation.clone().ok_or_else(|| {
        rpc_error(
            INTERNAL_ERROR_CODE,
            "Internal error",
            "initiation_registry_unavailable",
        )
    })?;
    let swap_id = request.swap_id.clone();
    let (reservation_id, dynamic) = tokio::task::spawn_blocking(move || {
        let context = initiation.lock().map_err(|_| {
            rpc_error(
                INTERNAL_ERROR_CODE,
                "Internal error",
                "initiation_registry_unavailable",
            )
        })?;
        let prepared = context
            .prepared_for_swap(&swap_id)
            .filter(|prepared| prepared.execution().dynamic())
            .ok_or_else(|| rpc_error(INVALID_PARAMS_CODE, "Invalid params", "lock_swap_unknown"))?;
        let dynamic = context.dynamic_btc().ok_or_else(|| {
            rpc_error(
                DEPENDENCY_UNAVAILABLE_CODE,
                "Taker dependency unavailable",
                "lock_unavailable",
            )
        })?;
        Ok::<_, ErrorObjectOwned>((prepared.reservation_id().clone(), dynamic))
    })
    .await
    .map_err(|_| {
        rpc_error(
            INTERNAL_ERROR_CODE,
            "Internal error",
            "initiation_registry_unavailable",
        )
    })??;
    if btc_dynamic::first_lock_chain(&dynamic, &reservation_id).is_none() {
        return Err(rpc_error(
            INVALID_PARAMS_CODE,
            "Invalid params",
            "lock_not_this_role",
        ));
    }
    let (chain, transaction_id, was_replay) = btc_dynamic::lock(&dynamic, &reservation_id)
        .await
        .map_err(|error| {
            eprintln!("taker first lock failed: {error:#}");
            rpc_error(
                DEPENDENCY_UNAVAILABLE_CODE,
                "Taker dependency unavailable",
                "lock_unavailable",
            )
        })?;
    Ok(TakerLockCommitV1 {
        schema_version: 1,
        swap_id: request.swap_id,
        chain: chain.into(),
        transaction_id: transaction_id.into(),
        was_replay,
    })
}

/// Phases in which the Taker's next drive only observes chains: before both
/// legs are locked, and after its own revealing claim while the Maker's
/// follow-up claim is awaited.
/// How long an owner's request waits for the observer to release a swap's
/// actor lock before it reports the dependency unavailable.
pub(super) const LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(20);

const fn taker_observation_phase(phase: Phase) -> bool {
    matches!(
        phase,
        Phase::Offered
            | Phase::AwaitingTakerConfirmations
            | Phase::TakerLockConfirmed
            | Phase::AwaitingMakerConfirmations
            | Phase::ClaimEvidenceAvailable
    )
}

/// Drives every Node-owned Bitcoin swap that is waiting on a chain
/// observation, once per pass, under the same held lock the monitor uses.
async fn observe_dynamic_swaps(state: &TakerServiceState) {
    let Some(initiation) = state.initiation.clone() else {
        return;
    };
    let (swaps, dynamic) = {
        let Ok(context) = initiation.lock() else {
            return;
        };
        (context.dynamic_bound_swaps(), context.dynamic_btc())
    };
    let Some(dynamic) = dynamic else {
        return;
    };
    for prepared in swaps {
        let Some(receipt_binding) = prepared.execution().receipt_binding() else {
            continue;
        };
        if let Err(error) = btc_dynamic::ensure_sidecar(&dynamic, prepared.reservation_id()) {
            eprintln!(
                "taker observer: sidecar for {} unavailable: {error:#}",
                prepared.swap_id().as_str()
            );
            continue;
        }
        let receipt = prepared.execution().receipt_output().to_path_buf();
        let actor_root = prepared.execution().actor_root().to_path_buf();
        let swap_id = prepared.swap_id().clone();
        let admitted_refund = matches!(
            lookup_admitted_action_for_swap(&initiation, &swap_id).await,
            Ok(Some(admission)) if admission.action() == TakerFacadeActionV1::Refund
        );
        let loaded = tokio::task::spawn_blocking(move || {
            let config = RouteActor::load_for_monitor(
                Pair::Bitcoin,
                &receipt,
                &actor_root,
                &swap_id,
                receipt_binding.sha256(),
                receipt_binding.identity().device(),
                receipt_binding.identity().inode(),
            )
            .map_err(|_| ())?;
            let held_lock =
                ActorHeldLock::acquire_for(config.swap_id(), config.state_db()).map_err(|_| ())?;
            Ok::<_, ()>((config, held_lock))
        })
        .await;
        let Ok(Ok((config, held_lock))) = loaded else {
            continue;
        };
        let Ok(LifecycleStatus::Active { phase, state, .. }) = config.status().await else {
            continue;
        };
        // An admitted refund is driven here until it lands. Its recover command
        // only observes while the cross-chain cutoff or the Bitcoin maturity is
        // still ahead, submits exactly once when both are met, then follows the
        // confirmations; the owner's request admitted it, the Node finishes it.
        if admitted_refund && !matches!(phase, Phase::Refunded | Phase::Completed) {
            // A maker lock can arrive after the refund was admitted — a late
            // broadcast that is still timely by median time. Recover then only
            // observes ("the lock is canonical, project it") and nothing does,
            // so the swap freezes at TakerLockConfirmed. Project it first in the
            // phases where a drive observes a lock rather than reveals a claim
            // (never BothLegsLocked); recovery then follows the correct leg.
            // Without this a Taker whose refund was admitted before a late-but-
            // timely maker lock stayed at revision one forever (public testnet,
            // swap 54184a0e).
            if taker_observation_phase(phase) {
                let _ = config.observe().await;
            }
            let _ = config.effect(TakerTerminalActionV1::Refund).await;
            drop(held_lock);
            continue;
        }
        // A submitted claim stays observed until it lands or the swap ends,
        // across the claim window's close: a claim included before the
        // deadline may be seen only afterwards, and observing never sends.
        // Once the window has closed the Maker's refund is watched as well,
        // through the recovery command, which for the Taker only observes.
        let claim_submitted =
            claim_submitted_marker(config.state_db()).is_some_and(|marker| marker.is_file());
        let claim_pending = phase == Phase::BothLegsLocked && claim_submitted;
        if taker_observation_phase(phase) || claim_pending {
            let _ = config.observe().await;
        }
        if phase == Phase::BothLegsLocked
            && (!claim_submitted || state != TakerSwapStateV1::ClaimAvailable)
        {
            // Both legs locked and no claim in flight, or the claim window has
            // closed: a drive here would be a revealing claim, so the Maker's
            // leg is watched through the recovery command instead. Once the
            // Maker has refunded, the swap moves to `refund_available`.
            let _ = config.effect(TakerTerminalActionV1::Refund).await;
        }
        drop(held_lock);
    }
}

/// Starts the Taker observer when a runtime is available; the monitor and the
/// terminal actions stay the only user-facing surface.
fn spawn_dynamic_observer(state: Arc<TakerServiceState>) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        eprintln!("taker observer: no async runtime; observation drives run only on demand");
        return;
    };
    handle.spawn(async move {
        loop {
            Box::pin(observe_dynamic_swaps(&state)).await;
            tokio::time::sleep(std::time::Duration::from_secs(15)).await;
        }
    });
}

/// Pairs whose lifecycle this Node runs itself.
const fn route_has_node_lifecycle(pair: Pair) -> bool {
    match pair {
        Pair::Bitcoin => true,
        #[cfg(feature = "pair-zec")]
        Pair::Zcash => true,
        #[allow(unreachable_patterns)]
        _ => false,
    }
}

async fn selected_offer_is_live(
    state: &TakerServiceState,
    request: &TakerSwapInitiateRequestV1,
) -> Result<(u64, bool), InitiationError> {
    let offer_request = TakerOfferListRequestV1 {
        schema_version: 1,
        route: Some(request.route),
    };
    let now = state
        .backend
        .trusted_now_for_offer_list(&offer_request)
        .map_err(InitiationError::Backend)?;
    let live_match = match live_broadcast_offer_matches(request, now)? {
        Some(matches) => matches,
        None => state
            .backend
            .offer_list_at(&offer_request, now)
            .await
            .map_err(InitiationError::Backend)?
            .offers
            .iter()
            .any(|candidate| {
                candidate.offer.id() == &request.offer_id
                    && candidate.offer.route() == request.route
                    && candidate.maker_identity == request.maker_identity
                    && candidate.signed_envelope_sha256 == request.signed_envelope_sha256
                    && candidate
                        .offer
                        .quote_foreign_amount(request.foreign_units)
                        .ok()
                        == Some(request.expected_lez_units)
            }),
    };
    Ok((now, live_match))
}

fn live_broadcast_offer_matches(
    request: &TakerSwapInitiateRequestV1,
    now_unix_seconds: u64,
) -> Result<Option<bool>, InitiationError> {
    let Some(proof_base64) = request.logos_offer_announcement_base64.as_deref() else {
        return Ok(None);
    };
    if proof_base64.is_empty() || proof_base64.len() > MAXIMUM_LOGOS_OFFER_ANNOUNCEMENT_BASE64_BYTES
    {
        return Err(InitiationError::SelectionMismatch);
    }
    let encoded = BASE64_STANDARD
        .decode(proof_base64.as_bytes())
        .map_err(|_| InitiationError::SelectionMismatch)?;
    if BASE64_STANDARD.encode(&encoded) != proof_base64 {
        return Err(InitiationError::SelectionMismatch);
    }
    let announcement = verify_logos_offer_announcement(&encoded, now_unix_seconds)
        .map_err(|_| InitiationError::SelectionMismatch)?;
    let authenticated = announcement.offer();
    let offer = authenticated.offer();
    Ok(Some(
        announcement.status() == MakerOfferStatus::Active
            && offer.id() == &request.offer_id
            && offer.route() == request.route
            && authenticated.maker_identity() == request.maker_identity.as_bytes()
            && authenticated.commitment() == request.signed_envelope_sha256
            && offer.quote_foreign_amount(request.foreign_units).ok()
                == Some(request.expected_lez_units),
    ))
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
    prepared: &PreparedTakerInitiationV1,
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
) -> Result<(PreparedTakerInitiationV1, bool, bool), InitiationError> {
    let context = Arc::clone(initiation);
    let offer_id = offer_id.clone();
    tokio::task::spawn_blocking(move || {
        let context = context.lock().map_err(|_| InitiationError::Internal)?;
        let execution_enabled = context.execution_enabled();
        let prepared = context
            .prepared_for_offer(&offer_id)
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
            .bind_prepared_receipt(&swap_id)
            .map_err(|()| {
                eprintln!(
                    "taker {}: binding the acceptance receipt failed (receipt identity or supervisor schedule)",
                    swap_id.as_str()
                );
                InitiationError::ExecutionUnavailable
            })
    })
    .await
    .map_err(|_| InitiationError::Internal)?
}

async fn execute_prepared(
    initiation: &Arc<Mutex<ConfiguredTakerInitiationContext>>,
    prepared: &PreparedTakerInitiationV1,
    admitted_at: u64,
) -> Result<(), InitiationError> {
    match prepared.facts().route().pair() {
        Pair::Bitcoin if prepared.execution().dynamic() => {
            let dynamic = initiation
                .lock()
                .map_err(|_| InitiationError::Internal)?
                .dynamic_btc()
                .ok_or(InitiationError::ExecutionUnavailable)?;
            btc_dynamic::execute(
                &dynamic,
                prepared.reservation_id(),
                prepared.execution().authenticated_offer(),
                prepared.facts().foreign_units(),
                admitted_at,
            )
            .await
            .map_err(|error| {
                eprintln!("taker BTC take failed: {error:#}");
                InitiationError::ExecutionUnavailable
            })
        }
        Pair::Bitcoin => execute_prepared_btc(prepared, admitted_at).await,
        #[cfg(feature = "pair-zec")]
        Pair::Zcash => execute_prepared_zec(prepared, admitted_at).await,
        #[allow(unreachable_patterns)]
        _ => Err(InitiationError::UnsupportedPair),
    }
}

/// Runs the prepared BTC take through the Maker's Chat methods, provisions the
/// Taker actor bundle, and activates it so monitoring sees durable state.
async fn execute_prepared_btc(
    prepared: &PreparedTakerInitiationV1,
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
    let actor = btc_reference_actor::ActorConfig::load_private_pinned_sha256(
        execution.source_config_path(),
        execution.source_config_sha256(),
    )
    .map_err(|_| InitiationError::ExecutionUnavailable)?;
    if actor.role() != btc_reference_actor::ActorRole::Taker {
        return Err(InitiationError::ExecutionUnavailable);
    }
    let direction: SwapDirection = prepared.facts().route().direction();
    take_btc_with_authenticated_offer_and_actor_config(
        BtcTakeInput {
            direction,
            delivery: None,
            now_unix_seconds: admitted_at,
            offer_id: prepared.offer_id().as_str(),
            chat_socket: execution.chat_socket(),
            reservation_id: prepared.reservation_id().as_str(),
            foreign_units: prepared.facts().foreign_units(),
            unsigned_draft_file: execution.unsigned_draft_path(),
            contribution_files: None,
            role_root: None,
            source_taker_config_file: Some(execution.source_config_path()),
            taker_actor_root: Some(execution.actor_root()),
            acceptance_receipt_file: Some(execution.receipt_output()),
            taker_signing_key_file: execution.signing_key_path(),
            agreement_output_file: execution.agreement_output(),
        },
        execution.authenticated_offer(),
        &actor,
    )
    .await
    .map_err(|error| {
        report_actor_failure("take", &error);
        InitiationError::ExecutionUnavailable
    })?;
    // The BTC actor keeps no durable state until it is activated; activate here
    // so the first monitor call already reports revision 0 instead of "not
    // activated". The ZEC actor is activated by the Node's supervisor on its
    // first attempt after the take (zec_supervisor.rs), not here.
    let receipt = execution.receipt_output().to_path_buf();
    let swap_id = prepared.swap_id().clone();
    let (route_actor, held_lock) = tokio::task::spawn_blocking(move || {
        let config = crate::btc_taker_accept::load_btc_taker_actor_from_receipt(&receipt)
            .map_err(|_| InitiationError::ExecutionUnavailable)?;
        let held_lock = ActorHeldLock::acquire_for_within(&swap_id, config.state_db(), LOCK_WAIT)
            .map_err(|_| InitiationError::ExecutionUnavailable)?;
        Ok::<_, InitiationError>((RouteActor::Btc { config, swap_id }, held_lock))
    })
    .await
    .map_err(|_| InitiationError::Internal)??;
    route_actor
        .activate()
        .await
        .map_err(|()| InitiationError::ExecutionUnavailable)?;
    held_lock
        .validate_for_state(route_actor.swap_id(), route_actor.state_db())
        .map_err(|_| InitiationError::ExecutionUnavailable)?;
    Ok(())
}

#[cfg(feature = "pair-zec")]
async fn execute_prepared_zec(
    prepared: &PreparedTakerInitiationV1,
    admitted_at: u64,
) -> Result<(), InitiationError> {
    use zec_reference_actor::{ActorConfig, ActorRole};
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
            direction: prepared.facts().route().direction(),
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
    .map_err(|error| {
        eprintln!("taker ZEC take failed: {error:#}");
        InitiationError::ExecutionUnavailable
    })?;
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
            attention_reason: None,
            revealing_claim_held_by: None,
            last_failure_class: None,
            observed_at: None,
            terms: None,
            effects: Vec::new(),
            bitcoin_lock_fee_sat: None,
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
        InitiationError::UnsupportedPair => rpc_error(
            INVALID_PARAMS_CODE,
            "Invalid params",
            "initiation_unsupported_pair",
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
            "initiation_execution_unavailable",
        ),
        InitiationError::FundingRequired(required) => ErrorObjectOwned::owned(
            FUNDING_REQUIRED_CODE,
            "Bitcoin funding required",
            Some(json!({
                "category": "bitcoin_funding_required",
                "address": required.address,
                "amount_sat": required.value_sat,
            })),
        ),
        InitiationError::Internal => rpc_error(
            INTERNAL_ERROR_CODE,
            "Internal error",
            "initiation_registry_unavailable",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// REVIEW 16:33Z: a revealing claim held behind another swap's as its own
    /// LEZ refund time nears reads as needing attention and names the holder;
    /// the action the actor offers stays, and a settled swap is never marked.
    #[cfg(feature = "pair-zec")]
    #[test]
    fn a_claim_held_near_its_refund_time_names_its_holder_on_the_row() {
        use lez_swap_core::{Pair, SwapDirection, SwapId};
        use lez_swap_store::{MakerOfferId, MakerRouteV1};
        let swap = tempfile::tempdir().expect("swap directory");
        let state_db = swap.path().join("taker").join("state").join("zec.sqlite3");
        std::fs::create_dir_all(state_db.parent().expect("state directory")).expect("dirs");
        let row = |state| TakerSwapViewV1 {
            schema_version: 1,
            swap_id: SwapId::new("m7-held-swap").unwrap(),
            offer_id: MakerOfferId::new("m7-held-offer").unwrap(),
            route: MakerRouteV1::new(Pair::Zcash, SwapDirection::TakerSellsForeign).unwrap(),
            foreign_units: 10_000,
            lez_units: 25_000,
            progress_generation: 7,
            state,
            available_action: Some(TakerTerminalActionV1::Claim),
            privacy_guidance: None,
            attention_reason: None,
            revealing_claim_held_by: None,
            last_failure_class: None,
            observed_at: None,
            terms: None,
            effects: Vec::new(),
            bitcoin_lock_fee_sat: None,
        };
        let mut view = row(TakerSwapStateV1::ClaimAvailable);
        overlay_held_revealing_claim(&mut view, &state_db);
        assert_eq!(view, row(TakerSwapStateV1::ClaimAvailable));

        zec_reference_actor::record_revealing_claim_holder(&state_db, Some("m7-holder-swap"));
        overlay_held_revealing_claim(&mut view, &state_db);
        assert_eq!(view.state, TakerSwapStateV1::AttentionRequired);
        assert_eq!(
            view.attention_reason.as_deref(),
            Some("lez_revealing_claim_held_near_refund")
        );
        assert_eq!(
            view.revealing_claim_held_by.as_deref(),
            Some("m7-holder-swap")
        );
        assert_eq!(view.available_action, Some(TakerTerminalActionV1::Claim));

        let mut settled = row(TakerSwapStateV1::Completed);
        overlay_held_revealing_claim(&mut settled, &state_db);
        assert_eq!(settled, row(TakerSwapStateV1::Completed));
    }

    /// 9afd25a8: an admitted Zcash action whose first attempt is refused is
    /// the supervisor's to retry, so the caller gets the admission; a Bitcoin
    /// action keeps its reply, and every other error stays an error.
    #[test]
    fn a_refused_first_attempt_is_the_supervisors_for_zcash_and_the_callers_for_bitcoin() {
        assert!(first_attempt_reply(true, Err(ActionError::DependencyUnavailable)).is_ok());
        assert!(matches!(
            first_attempt_reply(false, Err(ActionError::DependencyUnavailable)),
            Err(ActionError::DependencyUnavailable)
        ));
        assert!(matches!(
            first_attempt_reply(true, Err(ActionError::Conflict)),
            Err(ActionError::Conflict)
        ));
        assert!(first_attempt_reply(true, Ok(())).is_ok());
        assert!(first_attempt_reply(false, Ok(())).is_ok());
    }

    #[test]
    fn the_planned_lock_fee_is_read_from_the_swaps_funding_plan() {
        let swap = tempfile::tempdir().expect("swap directory");
        let state_db = swap.path().join("actor").join("state.sqlite3");
        std::fs::create_dir_all(swap.path().join("bitcoin")).expect("bitcoin directory");
        let plan = swap.path().join("bitcoin").join("funding-plan.json");
        // The Maker funds Bitcoin in the other direction: no plan, no fee.
        assert_eq!(planned_lock_fee_sat(&state_db), None);
        std::fs::write(
            &plan,
            br#"{"schema_version":1,"value_sat":10000,"fee_sat":157}"#,
        )
        .unwrap();
        assert_eq!(planned_lock_fee_sat(&state_db), Some(157));
        // A plan written before the fee was kept says nothing rather than zero.
        std::fs::write(&plan, br#"{"schema_version":1,"value_sat":10000}"#).unwrap();
        assert_eq!(planned_lock_fee_sat(&state_db), None);
    }

    fn active(
        phase: Phase,
        revision: u64,
        available_action: Option<TakerTerminalActionV1>,
    ) -> LifecycleStatus {
        LifecycleStatus::Active {
            phase,
            revision,
            state: TakerSwapStateV1::AttentionRequired,
            available_action,
            privacy_guidance: None,
        }
    }

    #[test]
    fn btc_taker_projection_advertises_only_role_valid_terminal_actions() {
        use btc_reference_actor::ActorNextActionV1 as Next;
        assert_eq!(
            btc_actor_progress(Phase::Offered, Next::ObserveTakerFirstLock),
            (TakerSwapStateV1::AwaitingFirstLock, None)
        );
        assert_eq!(
            btc_actor_progress(
                Phase::AwaitingMakerConfirmations,
                Next::ObserveMakerSecondLockOrRecoverTakerLeg,
            ),
            (TakerSwapStateV1::AwaitingSecondLock, None)
        );
        assert_eq!(
            btc_actor_progress(Phase::TakerLockConfirmed, Next::RecoverTakerLeg),
            (
                TakerSwapStateV1::AwaitingSecondLock,
                Some(TakerTerminalActionV1::Refund),
            )
        );
        assert_eq!(
            btc_actor_progress(Phase::BothLegsLocked, Next::ObserveRevealingClaim),
            (
                TakerSwapStateV1::ClaimAvailable,
                Some(TakerTerminalActionV1::Claim),
            )
        );
        assert_eq!(
            btc_actor_progress(Phase::BothLegsLocked, Next::RecoverMakerLeg),
            (TakerSwapStateV1::BothLegsLocked, None)
        );
        assert_eq!(
            btc_actor_progress(Phase::ClaimEvidenceAvailable, Next::ObserveFollowupClaim),
            (TakerSwapStateV1::ClaimInProgress, None)
        );
        assert_eq!(
            btc_actor_progress(Phase::MakerLegRefunded, Next::RecoverTakerLeg),
            (
                TakerSwapStateV1::RefundAvailable,
                Some(TakerTerminalActionV1::Refund),
            )
        );
        assert_eq!(
            btc_actor_progress(Phase::MakerLegRefunded, Next::LaterRevisionNotYetComposed),
            (TakerSwapStateV1::AttentionRequired, None)
        );
        assert_eq!(
            btc_actor_progress(Phase::Completed, Next::Complete),
            (TakerSwapStateV1::Completed, None)
        );
    }

    #[cfg(feature = "pair-zec")]
    #[test]
    fn zec_taker_projection_advertises_only_role_valid_terminal_actions() {
        assert_eq!(
            zec_actor_progress(Phase::Offered, ZecLifecycleAction::CreateAndFundLez),
            (TakerSwapStateV1::AwaitingFirstLock, None, None)
        );
        assert_eq!(
            zec_actor_progress(Phase::BothLegsLocked, ZecLifecycleAction::Wait),
            (
                TakerSwapStateV1::RefundAvailable,
                Some(TakerTerminalActionV1::Refund),
                None,
            )
        );
        assert_eq!(
            zec_actor_progress(Phase::AwaitingMakerConfirmations, ZecLifecycleAction::Wait),
            (
                TakerSwapStateV1::RefundAvailable,
                Some(TakerTerminalActionV1::Refund),
                None,
            )
        );
        assert_eq!(
            zec_actor_progress(
                Phase::ClaimEvidenceAvailable,
                ZecLifecycleAction::ClaimZcash,
            ),
            (
                TakerSwapStateV1::ClaimAvailable,
                Some(TakerTerminalActionV1::Claim),
                None,
            )
        );
        // The reverse direction's Taker is the first claimant: the SDK offers
        // its revealing LEZ claim at BothLegsLocked (next_action claim_lez), and
        // once that claim is final (ClaimEvidenceAvailable, next_action wait)
        // only the Maker's Zcash follow-up claim is left to observe.
        assert_eq!(
            zec_actor_progress(Phase::BothLegsLocked, ZecLifecycleAction::ClaimLez),
            (
                TakerSwapStateV1::ClaimAvailable,
                Some(TakerTerminalActionV1::Claim),
                None,
            )
        );
        assert_eq!(
            zec_actor_progress(Phase::ClaimEvidenceAvailable, ZecLifecycleAction::Wait),
            (TakerSwapStateV1::ClaimInProgress, None, None)
        );
        // A Maker-only pair the Taker never produces reads as needing attention.
        assert_eq!(
            zec_actor_progress(Phase::MakerLegRefunded, ZecLifecycleAction::Wait),
            (TakerSwapStateV1::AttentionRequired, None, None)
        );
        assert_eq!(
            zec_actor_progress(Phase::MakerLegRefunded, ZecLifecycleAction::RefundZcash),
            (
                TakerSwapStateV1::RefundAvailable,
                Some(TakerTerminalActionV1::Refund),
                None,
            )
        );
        assert_eq!(
            zec_actor_progress(Phase::MakerLegRefunded, ZecLifecycleAction::Wait),
            (TakerSwapStateV1::AttentionRequired, None, None)
        );
        // Devnet finding (M2 step 5): after the Taker's own LEZ refund the actor
        // parks at TakerLegRefunded until it observes the Maker's Zcash refund;
        // the desk reads "refund in progress" with Refund still offered, never
        // "attention required".
        assert_eq!(
            zec_actor_progress(Phase::TakerLegRefunded, ZecLifecycleAction::Wait),
            (
                TakerSwapStateV1::RefundInProgress,
                Some(TakerTerminalActionV1::Refund),
                None,
            )
        );
        assert_eq!(
            zec_actor_progress(Phase::Completed, ZecLifecycleAction::Complete),
            (
                TakerSwapStateV1::Completed,
                None,
                Some(TakerPrivacyGuidanceV1::ShieldReceivedTransparentZecSeparately),
            )
        );
    }

    #[test]
    fn exact_action_replay_reenters_only_unadvanced_or_intermediate_refund() {
        let claim_available = active(Phase::BothLegsLocked, 3, Some(TakerTerminalActionV1::Claim));
        assert!(
            replay_actor_effect_required(claim_available, 3, TakerTerminalActionV1::Claim).unwrap()
        );

        let completed = active(Phase::Completed, 4, None);
        assert!(!replay_actor_effect_required(completed, 3, TakerTerminalActionV1::Claim).unwrap());

        for phase in [Phase::MakerLegRefunded, Phase::TakerLegRefunded] {
            assert!(
                replay_actor_effect_required(
                    active(phase, 4, None),
                    3,
                    TakerTerminalActionV1::Refund,
                )
                .unwrap(),
                "an admitted refund must continue through {phase:?}"
            );
            assert!(
                !replay_actor_effect_required(
                    active(phase, 4, None),
                    3,
                    TakerTerminalActionV1::Claim,
                )
                .unwrap(),
                "an advanced claim must not re-enter through {phase:?}"
            );
        }

        let refunded = active(Phase::Refunded, 5, None);
        assert!(!replay_actor_effect_required(refunded, 3, TakerTerminalActionV1::Refund).unwrap());

        assert!(
            replay_actor_effect_required(
                active(Phase::BothLegsLocked, 2, Some(TakerTerminalActionV1::Claim)),
                3,
                TakerTerminalActionV1::Claim,
            )
            .is_err(),
            "a stale actor revision must not run the admitted action"
        );
        assert!(
            replay_actor_effect_required(
                LifecycleStatus::NotActivated,
                3,
                TakerTerminalActionV1::Claim,
            )
            .is_err()
        );
    }

    #[test]
    fn a_refund_replay_is_redriven_against_its_admitted_generation() {
        // Admitted at 4; the actor has since moved through the Taker's leg
        // refund to revision 7 and offers no action there. The desk's Retry
        // re-sends the request with the row's current generation: judged by
        // that one the replay has nothing to do, judged by the admitted one it
        // continues the refund. The replay path uses the admitted one.
        let parked = || active(Phase::TakerLegRefunded, 7, None);
        assert!(matches!(
            replay_actor_effect_required(parked(), 7, TakerTerminalActionV1::Refund),
            Err(ActionError::Unavailable)
        ));
        assert!(replay_actor_effect_required(parked(), 4, TakerTerminalActionV1::Refund).unwrap());
        // A claim admitted at 4 is not re-entered through the refund phases.
        assert!(!replay_actor_effect_required(parked(), 4, TakerTerminalActionV1::Claim).unwrap());
    }

    #[test]
    fn only_the_zcash_route_admits_a_refund_in_the_taker_sells_foreign_claim_window() {
        let claim_window = || LifecycleStatus::Active {
            phase: Phase::BothLegsLocked,
            revision: 4,
            state: TakerSwapStateV1::ClaimAvailable,
            available_action: Some(TakerTerminalActionV1::Claim),
            privacy_guidance: None,
        };
        assert!(
            zec_replay_actor_effect_required(
                claim_window(),
                4,
                TakerTerminalActionV1::Refund,
                false
            )
            .unwrap()
        );
        assert!(
            zec_replay_actor_effect_required(
                claim_window(),
                4,
                TakerTerminalActionV1::Claim,
                false
            )
            .unwrap()
        );
        // The generic rule (the Bitcoin route) is unchanged: one offered action.
        assert!(matches!(
            replay_actor_effect_required(claim_window(), 4, TakerTerminalActionV1::Refund),
            Err(ActionError::Unavailable)
        ));
        // Once a revealing claim was submitted the claim window's refund is refused.
        assert!(matches!(
            zec_replay_actor_effect_required(
                claim_window(),
                4,
                TakerTerminalActionV1::Refund,
                true
            ),
            Err(ActionError::Unavailable)
        ));
        // Admitted at an older generation the refund is not re-run here.
        assert!(!matches!(
            zec_replay_actor_effect_required(
                claim_window(),
                3,
                TakerTerminalActionV1::Refund,
                false
            ),
            Ok(true)
        ));
    }

    #[test]
    fn facade_actions_map_back_to_terminal_actions() {
        for action in [TakerTerminalActionV1::Claim, TakerTerminalActionV1::Refund] {
            assert_eq!(terminal_action_of(facade_action(action)), action);
        }
    }

    #[test]
    fn concurrent_terminal_winner_maps_to_action_conflict() {
        assert!(matches!(
            map_action_store_error(TakerFacadeStoreError::ActionGenerationConflict),
            ActionError::Conflict
        ));
    }
}
