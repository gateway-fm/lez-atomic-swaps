use std::{
    convert::Infallible,
    fs, io,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use lez_bridge_adapter::{
    ActorBridgeRequestContextSource, BridgeDiscoveryWindowSource,
    CapabilityFileBridgeClientFactory, ContextOwningLezBridgePorts,
    SqliteCanonicalLezFundingSource, validate_runtime_binding,
};
use lez_bridge_protocol::DiscoveryWindow;
use lez_swap_core::{Participant, Phase, SwapDirection, UnixSeconds};
use lez_swap_store::{BridgeOperationKey, SqliteBridgeOperationJournal, SqliteZecRecoveryStore};
use lez_zebra_node_adapter::{
    ExactOutpointZcashFundingPlanner, HttpZebraRpc, HttpZebraRpcConfig, RoleKeyedZcashSigner,
    ZebraChainIdentity, ZebraRpcChain, ZebraRpcZcashPort,
};
use lez_zec_swap_sdk::{
    AcceptedZecAgreementV1, ClaimDriveOutcome, ClaimPreimage, ClaimStepV1, FirstLockDriveOutcome,
    FirstLockStepV1, MakerFundingEligibilityOutcome, MakerLockDriveOutcome,
    ObserveMakerLockOutcome, ObserveTakerFirstLockOutcome, RecoveryStore, RefundDriveOutcome,
    RefundFundingWaitReasonV1, RefundStepV1, ZecLifecycleAction, ZecPairSdk,
};
use secp256k1::SecretKey;
use serde::Serialize;
use thiserror::Error;
use zcash_primitives::block::BlockHash;
use zcash_protocol::consensus::{BranchId, NetworkType};
use zcash_transparent::bundle::OutPoint;

use crate::{ActorCommand, ActorConfig, ActorRole, ZcashNetworkConfig, ZebraRpcChainConfig};

const STATUS_SCHEMA_VERSION: u16 = 1;

/// Secret-free, versioned output from one actor command.
#[derive(Debug, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ActorCommandOutputV1 {
    /// Durable lifecycle status projected without contacting either chain.
    Status(ActorStatusV1),
    /// Result of one bounded effect-capable actor invocation.
    Effect(ActorEffectOutputV1),
}

/// Secret-free result of activation or one lifecycle-driving attempt.
#[derive(Debug, Eq, PartialEq, Serialize)]
pub struct ActorEffectOutputV1 {
    schema_version: u16,
    role: ActorRole,
    command: ActorEffectCommandV1,
    #[serde(flatten)]
    outcome: ActorEffectOutcomeV1,
    phase: ActorPhaseV1,
    revision: u64,
    next_action: ActorNextActionV1,
}

impl ActorEffectOutputV1 {
    fn from_active<Lez, Zcash, Store>(
        role: ActorRole,
        command: ActorEffectCommandV1,
        outcome: ActorEffectOutcomeV1,
        active: &lez_zec_swap_sdk::ActiveZecSwap<Lez, Zcash, Store>,
    ) -> Self {
        Self {
            schema_version: STATUS_SCHEMA_VERSION,
            role,
            command,
            outcome,
            phase: active.status().into(),
            revision: active.revision(),
            next_action: active.next_action().into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ActorEffectCommandV1 {
    Activate,
    Drive,
    Claim,
    Recover,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
enum ActorEffectOutcomeV1 {
    Activated,
    Submitted {
        operation: ActorOperationV1,
    },
    AwaitingObservation {
        operation: ActorOperationV1,
    },
    AwaitingSafeZcashFunding,
    Unchanged {
        operation: ActorOperationV1,
    },
    Projected {
        operation: ActorOperationV1,
    },
    Completed,
    AwaitingFunding {
        operation: ActorOperationV1,
        reason: ActorRefundFundingWaitReasonV1,
    },
    AwaitingDeadline {
        operation: ActorOperationV1,
    },
    SubmissionRejected {
        operation: ActorOperationV1,
    },
    SubmissionOutcomeUnknown {
        operation: ActorOperationV1,
    },
    Refunded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ActorRefundFundingWaitReasonV1 {
    Absent,
    Spent,
    Reorged,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ActorOperationV1 {
    LezInitialize,
    LezFund,
    TakerFirstLock,
    ZcashFund,
    MakerLock,
    LezRevealingClaim,
    ZcashFollowupClaim,
    LezRefund,
    ZcashRefund,
}

/// Versioned role-local lifecycle status.
#[derive(Debug, Eq, PartialEq, Serialize)]
pub struct ActorStatusV1 {
    schema_version: u16,
    role: ActorRole,
    #[serde(flatten)]
    state: ActorStateV1,
}

/// Typed, secret-free view of durable actor lifecycle state.
///
/// This projection is deliberately separate from the serialized status schema so
/// in-process callers can inspect status without depending on its JSON encoding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActorStatusProjectionV1 {
    /// No durable activation exists for this actor yet.
    NotActivated,
    /// The actor has durable lifecycle state.
    Active {
        /// Current protocol phase.
        phase: Phase,
        /// Monotonic durable-state revision.
        revision: u64,
        /// Next safe high-level action for this role.
        next_action: ZecLifecycleAction,
    },
}

impl ActorStatusV1 {
    fn not_activated(role: ActorRole) -> Self {
        Self {
            schema_version: STATUS_SCHEMA_VERSION,
            role,
            state: ActorStateV1::NotActivated,
        }
    }

    fn active(
        role: ActorRole,
        phase: Phase,
        revision: u64,
        next_action: ZecLifecycleAction,
    ) -> Self {
        Self {
            schema_version: STATUS_SCHEMA_VERSION,
            role,
            state: ActorStateV1::Active {
                phase: phase.into(),
                revision,
                next_action: next_action.into(),
            },
        }
    }

    /// Return the role permanently bound to this actor status.
    #[must_use]
    pub const fn role(&self) -> ActorRole {
        self.role
    }

    /// Project the wire-oriented status into typed lifecycle values.
    #[must_use]
    pub const fn projection(&self) -> ActorStatusProjectionV1 {
        match self.state {
            ActorStateV1::NotActivated => ActorStatusProjectionV1::NotActivated,
            ActorStateV1::Active {
                phase,
                revision,
                next_action,
            } => ActorStatusProjectionV1::Active {
                phase: phase.as_phase(),
                revision,
                next_action: next_action.as_lifecycle_action(),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum ActorStateV1 {
    NotActivated,
    Active {
        phase: ActorPhaseV1,
        revision: u64,
        next_action: ActorNextActionV1,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ActorPhaseV1 {
    Offered,
    AwaitingTakerConfirmations,
    TakerLockConfirmed,
    AwaitingMakerConfirmations,
    BothLegsLocked,
    TakerLockReorged,
    MakerLockReorged,
    ClaimEvidenceAvailable,
    Completed,
    MakerLegRefunded,
    TakerLegRefunded,
    Refunded,
    MakerRecoveryAvailable,
}

impl From<Phase> for ActorPhaseV1 {
    fn from(value: Phase) -> Self {
        match value {
            Phase::Offered => Self::Offered,
            Phase::AwaitingTakerConfirmations => Self::AwaitingTakerConfirmations,
            Phase::TakerLockConfirmed => Self::TakerLockConfirmed,
            Phase::AwaitingMakerConfirmations => Self::AwaitingMakerConfirmations,
            Phase::BothLegsLocked => Self::BothLegsLocked,
            Phase::TakerLockReorged => Self::TakerLockReorged,
            Phase::MakerLockReorged => Self::MakerLockReorged,
            Phase::ClaimEvidenceAvailable => Self::ClaimEvidenceAvailable,
            Phase::Completed => Self::Completed,
            Phase::MakerLegRefunded => Self::MakerLegRefunded,
            Phase::TakerLegRefunded => Self::TakerLegRefunded,
            Phase::Refunded => Self::Refunded,
            Phase::MakerRecoveryAvailable => Self::MakerRecoveryAvailable,
        }
    }
}

impl ActorPhaseV1 {
    const fn as_phase(self) -> Phase {
        match self {
            Self::Offered => Phase::Offered,
            Self::AwaitingTakerConfirmations => Phase::AwaitingTakerConfirmations,
            Self::TakerLockConfirmed => Phase::TakerLockConfirmed,
            Self::AwaitingMakerConfirmations => Phase::AwaitingMakerConfirmations,
            Self::BothLegsLocked => Phase::BothLegsLocked,
            Self::TakerLockReorged => Phase::TakerLockReorged,
            Self::MakerLockReorged => Phase::MakerLockReorged,
            Self::ClaimEvidenceAvailable => Phase::ClaimEvidenceAvailable,
            Self::Completed => Phase::Completed,
            Self::MakerLegRefunded => Phase::MakerLegRefunded,
            Self::TakerLegRefunded => Phase::TakerLegRefunded,
            Self::Refunded => Phase::Refunded,
            Self::MakerRecoveryAvailable => Phase::MakerRecoveryAvailable,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ActorNextActionV1 {
    Wait,
    CreateAndFundLez,
    FundZcash,
    ClaimLez,
    ClaimZcash,
    RefundZcash,
    Complete,
}

impl From<ZecLifecycleAction> for ActorNextActionV1 {
    fn from(value: ZecLifecycleAction) -> Self {
        match value {
            ZecLifecycleAction::Wait => Self::Wait,
            ZecLifecycleAction::CreateAndFundLez => Self::CreateAndFundLez,
            ZecLifecycleAction::FundZcash => Self::FundZcash,
            ZecLifecycleAction::ClaimLez => Self::ClaimLez,
            ZecLifecycleAction::ClaimZcash => Self::ClaimZcash,
            ZecLifecycleAction::RefundZcash => Self::RefundZcash,
            ZecLifecycleAction::Complete => Self::Complete,
        }
    }
}

impl ActorNextActionV1 {
    const fn as_lifecycle_action(self) -> ZecLifecycleAction {
        match self {
            Self::Wait => ZecLifecycleAction::Wait,
            Self::CreateAndFundLez => ZecLifecycleAction::CreateAndFundLez,
            Self::FundZcash => ZecLifecycleAction::FundZcash,
            Self::ClaimLez => ZecLifecycleAction::ClaimLez,
            Self::ClaimZcash => ZecLifecycleAction::ClaimZcash,
            Self::RefundZcash => ZecLifecycleAction::RefundZcash,
            Self::Complete => ZecLifecycleAction::Complete,
        }
    }
}

/// Stable payload-free failure categories for the one-shot process boundary.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ActorCommandError {
    /// Fresh activation material could not be loaded safely.
    #[error("actor activation material is unavailable")]
    ActivationMaterialUnavailable,
    /// The signed agreement could not be activated durably.
    #[error("actor activation is unavailable")]
    ActivationUnavailable,
    /// Fresh effect material could not be loaded safely.
    #[error("actor drive material is unavailable")]
    DriveMaterialUnavailable,
    /// A role-local chain adapter could not be configured safely.
    #[error("actor drive configuration is unavailable")]
    DriveConfigurationUnavailable,
    /// The role-local durable stores could not be opened.
    #[error("actor drive store is unavailable")]
    DriveStoreUnavailable,
    /// No activated agreement was available for this role.
    #[error("actor is not activated")]
    NotActivated,
    /// Durable lifecycle state could not be resumed safely.
    #[error("actor drive replay is unavailable")]
    DriveReplayUnavailable,
    /// One bounded lifecycle attempt failed closed.
    #[error("actor drive is unavailable")]
    DriveUnavailable,
    /// Fresh claim material could not be loaded safely.
    #[error("actor claim material is unavailable")]
    ClaimMaterialUnavailable,
    /// A role-local claim adapter could not be configured safely.
    #[error("actor claim configuration is unavailable")]
    ClaimConfigurationUnavailable,
    /// The role-local claim stores could not be opened.
    #[error("actor claim store is unavailable")]
    ClaimStoreUnavailable,
    /// Durable lifecycle state could not be resumed for a claim.
    #[error("actor claim replay is unavailable")]
    ClaimReplayUnavailable,
    /// The durable lifecycle is not in a claim-capable phase.
    #[error("actor claim is not currently available")]
    ClaimUnavailable,
    /// Fresh timeout-recovery material could not be loaded safely.
    #[error("actor recovery material is unavailable")]
    RecoveryMaterialUnavailable,
    /// A role-local recovery adapter could not be configured safely.
    #[error("actor recovery configuration is unavailable")]
    RecoveryConfigurationUnavailable,
    /// The role-local recovery stores could not be opened.
    #[error("actor recovery store is unavailable")]
    RecoveryStoreUnavailable,
    /// Durable lifecycle state could not be resumed for recovery.
    #[error("actor recovery replay is unavailable")]
    RecoveryReplayUnavailable,
    /// One bounded agreement-ordered recovery attempt failed closed.
    #[error("actor recovery is unavailable")]
    RecoveryUnavailable,
    /// The role-local state path could not be inspected safely.
    #[error("actor status is unavailable")]
    StatusUnavailable,
    /// Fresh claim-recovery material could not be loaded safely.
    #[error("actor status material is unavailable")]
    StatusMaterialUnavailable,
    /// The claim-capable durable store could not be reopened.
    #[error("actor status store is unavailable")]
    StatusStoreUnavailable,
    /// Durable lifecycle history failed validation or replay.
    #[error("actor status replay is unavailable")]
    StatusReplayUnavailable,
}

/// Executes exactly one role-fixed command.
///
/// Status is deliberately composed with unit LEZ and Zcash ports. Consequently,
/// its durable replay cannot perform an RPC even if a future port implementation
/// changes behavior. Effect-bearing commands fail closed until their complete
/// adapters and preflight sequence are wired.
///
/// # Errors
///
/// Returns a stable payload-free category for unsupported commands, unsafe
/// material, store failures, or invalid durable lifecycle history.
pub async fn execute_actor_command(
    config: &ActorConfig,
    command: ActorCommand,
) -> Result<ActorCommandOutputV1, ActorCommandError> {
    match command {
        ActorCommand::Status => status(config).await.map(ActorCommandOutputV1::Status),
        ActorCommand::Activate => activate(config).await.map(ActorCommandOutputV1::Effect),
        ActorCommand::Drive => drive(config).await.map(ActorCommandOutputV1::Effect),
        ActorCommand::Claim => claim(config).await.map(ActorCommandOutputV1::Effect),
        ActorCommand::Recover => recover(config).await.map(ActorCommandOutputV1::Effect),
    }
}

async fn activate(config: &ActorConfig) -> Result<ActorEffectOutputV1, ActorCommandError> {
    let material = config
        .load_activate_material()
        .map_err(|_| ActorCommandError::ActivationMaterialUnavailable)?;
    let participant = config.role().sdk_participant();
    let accepted = AcceptedZecAgreementV1::accept_wire_at(
        material.signed_agreement_wire(),
        trusted_wall_clock()?,
        participant,
        0,
    )
    .map_err(|_| ActorCommandError::ActivationUnavailable)?;
    validate_runtime_binding(accepted.agreement(), config.bridge_runtime(), participant)
        .map_err(|_| ActorCommandError::ActivationUnavailable)?;
    let zebra = zebra_identity(config).map_err(|_| ActorCommandError::ActivationUnavailable)?;
    let expected_zcash = accepted.agreement().binding().expected_output();
    if zebra.network() != expected_zcash.network()
        || zebra.consensus_branch_id() != expected_zcash.consensus_branch_id()
    {
        return Err(ActorCommandError::ActivationUnavailable);
    }
    let preimage = material
        .claim_preimage()
        .map(|value| ClaimPreimage::new(*value.expose_secret()));
    let store = SqliteZecRecoveryStore::open_claim_capable(
        config.role_state_db(),
        participant,
        material.into_claim_recovery_key(),
    )
    .map_err(|_| ActorCommandError::ActivationUnavailable)?;
    let sdk = ZecPairSdk::new(participant, (), (), (), (), store);
    let active = match preimage {
        Some(preimage) => sdk.activate_with_claim_preimage(accepted, preimage).await,
        None => sdk.activate(accepted).await,
    }
    .map_err(|_| ActorCommandError::ActivationUnavailable)?;
    Ok(ActorEffectOutputV1::from_active(
        config.role(),
        ActorEffectCommandV1::Activate,
        ActorEffectOutcomeV1::Activated,
        &active,
    ))
}

fn trusted_wall_clock() -> Result<UnixSeconds, ActorCommandError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| UnixSeconds::new(elapsed.as_secs()))
        .map_err(|_| ActorCommandError::ActivationUnavailable)
}

// The configured page seeds new operations. Once reserved, the bridge journal
// owns the exact current page and advances fully covered misses contiguously
// across process restarts.
#[derive(Clone, Copy, Debug)]
struct ConfiguredDiscoveryWindow(DiscoveryWindow);

impl BridgeDiscoveryWindowSource for ConfiguredDiscoveryWindow {
    type Error = Infallible;

    fn discovery_window(&self, _key: &BridgeOperationKey) -> Result<DiscoveryWindow, Self::Error> {
        Ok(self.0)
    }
}

#[allow(clippy::too_many_lines)]
async fn drive(config: &ActorConfig) -> Result<ActorEffectOutputV1, ActorCommandError> {
    let material = config
        .load_drive_material()
        .map_err(|_| ActorCommandError::DriveMaterialUnavailable)?;
    let participant = config.role().sdk_participant();
    let identity = zebra_identity(config)?;
    let mut rpc_config = if let Some(api_key) = material.zebra_api_key() {
        HttpZebraRpcConfig::public_https(config.zebra_endpoint().as_str())
            .and_then(|route| route.with_public_api_key(api_key))
            .map_err(|_| ActorCommandError::DriveConfigurationUnavailable)?
    } else {
        HttpZebraRpcConfig::new(config.zebra_endpoint().as_str())
    }
    .with_request_timeout(Duration::from_secs(30))
    .with_max_concurrent_requests(1);
    if let Some(cookie) = material.zebra_cookie() {
        rpc_config = rpc_config
            .with_cookie_credentials(cookie)
            .map_err(|_| ActorCommandError::DriveConfigurationUnavailable)?;
    }
    let rpc = HttpZebraRpc::connect(&rpc_config)
        .map_err(|_| ActorCommandError::DriveConfigurationUnavailable)?;
    let signer = RoleKeyedZcashSigner::new(
        participant,
        SecretKey::from_slice(material.zcash_secret_key())
            .map_err(|_| ActorCommandError::DriveMaterialUnavailable)?,
    );
    let store = SqliteZecRecoveryStore::open_claim_capable(
        config.role_state_db(),
        participant,
        material.into_claim_recovery_key(),
    )
    .map_err(|_| ActorCommandError::DriveStoreUnavailable)?;
    let factory = CapabilityFileBridgeClientFactory::new(
        config.bridge_endpoint().as_str(),
        config.bridge_capability_file(),
        config.run_id().clone(),
        config.bridge_runtime().clone(),
        config.bridge_request_timeout(),
    );
    let contexts = ActorBridgeRequestContextSource::new(ConfiguredDiscoveryWindow(
        config.lez_discovery_window(),
    ));
    let funding = SqliteCanonicalLezFundingSource::new(store.clone(), participant);
    let lez = ContextOwningLezBridgePorts::new(
        config.run_id().clone(),
        config.bridge_runtime().clone(),
        participant,
        factory,
        contexts,
        store.clone(),
        funding,
        SqliteBridgeOperationJournal::open(config.bridge_journal_db())
            .map_err(|_| ActorCommandError::DriveStoreUnavailable)?,
    )
    .map_err(|_| ActorCommandError::DriveConfigurationUnavailable)?;
    let zcash = ZebraRpcZcashPort::new(rpc.clone(), signer.clone(), identity, participant)
        .map_err(|_| ActorCommandError::DriveConfigurationUnavailable)?
        .with_counterparty_scan_blocks(config.counterparty_scan_blocks());
    let planner = ExactOutpointZcashFundingPlanner::new(rpc, identity, participant, signer);
    let sdk = ZecPairSdk::new(participant, (), (), lez.clone(), zcash, store.clone());
    let mut active = sdk
        .resume_all_capable(config.swap_id())
        .await
        .map_err(|_| ActorCommandError::DriveReplayUnavailable)?
        .ok_or(ActorCommandError::NotActivated)?;

    let outcome = match (participant, active.status()) {
        (Participant::Taker, Phase::Offered) => {
            if store
                .load_first_lock_intent(active.agreement().coordinator().id())
                .await
                .map_err(|_| ActorCommandError::DriveUnavailable)?
                .is_none()
            {
                let plan = match active.next_action() {
                    ZecLifecycleAction::CreateAndFundLez => lez
                        .prepare_native_first_lock(active.agreement())
                        .await
                        .map_err(|_| ActorCommandError::DriveUnavailable)?,
                    ZecLifecycleAction::FundZcash => planner
                        .plan(active.agreement(), funding_outpoints(config))
                        .await
                        .map_err(|_| ActorCommandError::DriveUnavailable)?,
                    _ => return Err(ActorCommandError::DriveConfigurationUnavailable),
                };
                active
                    .stage_first_lock(plan)
                    .await
                    .map_err(|_| ActorCommandError::DriveUnavailable)?;
            }
            match active
                .drive_first_lock()
                .await
                .map_err(|_| ActorCommandError::DriveUnavailable)?
            {
                FirstLockDriveOutcome::Submitted(step) => ActorEffectOutcomeV1::Submitted {
                    operation: first_lock_operation(step),
                },
                FirstLockDriveOutcome::AwaitingStableObservation(step) => {
                    ActorEffectOutcomeV1::AwaitingObservation {
                        operation: first_lock_operation(step),
                    }
                }
                FirstLockDriveOutcome::ReadyForFundingProjection(evidence) => {
                    active
                        .project_first_lock(evidence)
                        .await
                        .map_err(|_| ActorCommandError::DriveUnavailable)?;
                    ActorEffectOutcomeV1::Projected {
                        operation: ActorOperationV1::TakerFirstLock,
                    }
                }
            }
        }
        (Participant::Maker, Phase::Offered | Phase::AwaitingTakerConfirmations) => match active
            .observe_taker_first_lock()
            .await
            .map_err(|_| ActorCommandError::DriveUnavailable)?
        {
            ObserveTakerFirstLockOutcome::AwaitingStableObservation(_) => {
                ActorEffectOutcomeV1::AwaitingObservation {
                    operation: ActorOperationV1::TakerFirstLock,
                }
            }
            ObserveTakerFirstLockOutcome::Unchanged(_) => ActorEffectOutcomeV1::Unchanged {
                operation: ActorOperationV1::TakerFirstLock,
            },
            ObserveTakerFirstLockOutcome::Projected(_) => ActorEffectOutcomeV1::Projected {
                operation: ActorOperationV1::TakerFirstLock,
            },
        },
        (Participant::Maker, Phase::TakerLockConfirmed) => {
            let plan = match store
                .load_maker_lock_intent(active.agreement().coordinator().id())
                .await
                .map_err(|_| ActorCommandError::DriveUnavailable)?
            {
                Some(intent) => intent.plan().clone(),
                None => match active.agreement().direction() {
                    SwapDirection::TakerSellsForeign => lez
                        .prepare_native_first_lock(active.agreement())
                        .await
                        .map_err(|_| ActorCommandError::DriveUnavailable)?,
                    SwapDirection::TakerSellsLez => planner
                        .plan(active.agreement(), funding_outpoints(config))
                        .await
                        .map_err(|_| ActorCommandError::DriveUnavailable)?,
                },
            };
            match active
                .drive_maker_lock(plan)
                .await
                .map_err(|_| ActorCommandError::DriveUnavailable)?
            {
                MakerLockDriveOutcome::AlreadyLocked { .. } => ActorEffectOutcomeV1::Unchanged {
                    operation: ActorOperationV1::MakerLock,
                },
                MakerLockDriveOutcome::AwaitingEligibility(
                    MakerFundingEligibilityOutcome::CanonicalStateChanged(_),
                ) => ActorEffectOutcomeV1::Projected {
                    operation: ActorOperationV1::TakerFirstLock,
                },
                MakerLockDriveOutcome::AwaitingEligibility(_) => {
                    ActorEffectOutcomeV1::AwaitingObservation {
                        operation: ActorOperationV1::TakerFirstLock,
                    }
                }
                MakerLockDriveOutcome::Lock(FirstLockDriveOutcome::Submitted(step)) => {
                    ActorEffectOutcomeV1::Submitted {
                        operation: first_lock_operation(step),
                    }
                }
                MakerLockDriveOutcome::Lock(FirstLockDriveOutcome::AwaitingStableObservation(
                    step,
                )) => ActorEffectOutcomeV1::AwaitingObservation {
                    operation: first_lock_operation(step),
                },
                MakerLockDriveOutcome::Lock(FirstLockDriveOutcome::ReadyForFundingProjection(
                    evidence,
                )) => {
                    active
                        .project_maker_lock(evidence)
                        .await
                        .map_err(|_| ActorCommandError::DriveUnavailable)?;
                    ActorEffectOutcomeV1::Projected {
                        operation: ActorOperationV1::MakerLock,
                    }
                }
            }
        }
        (Participant::Taker, Phase::TakerLockConfirmed) => match active
            .observe_maker_lock()
            .await
            .map_err(|_| {
            ActorCommandError::DriveUnavailable
        })? {
            ObserveMakerLockOutcome::AwaitingStableObservation(_) => {
                ActorEffectOutcomeV1::AwaitingObservation {
                    operation: ActorOperationV1::MakerLock,
                }
            }
            ObserveMakerLockOutcome::Projected(_) => ActorEffectOutcomeV1::Projected {
                operation: ActorOperationV1::MakerLock,
            },
            ObserveMakerLockOutcome::AlreadyObserved { .. } => ActorEffectOutcomeV1::Unchanged {
                operation: ActorOperationV1::MakerLock,
            },
        },
        (_, Phase::BothLegsLocked | Phase::ClaimEvidenceAvailable) => {
            let outcome = active
                .drive_claim()
                .await
                .map_err(|_| ActorCommandError::DriveUnavailable)?;
            claim_outcome(outcome)
        }
        (_, Phase::Completed) => ActorEffectOutcomeV1::Completed,
        _ => return Err(ActorCommandError::DriveUnavailable),
    };

    Ok(ActorEffectOutputV1::from_active(
        config.role(),
        ActorEffectCommandV1::Drive,
        outcome,
        &active,
    ))
}

/// Runs only the agreement-derived claim state machine.
///
/// Unlike `drive`, this boundary cannot fund or lock either leg. A premature
/// operator request therefore fails closed before an effect RPC rather than
/// silently advancing a different lifecycle action.
async fn claim(config: &ActorConfig) -> Result<ActorEffectOutputV1, ActorCommandError> {
    let material = config
        .load_drive_material()
        .map_err(|_| ActorCommandError::ClaimMaterialUnavailable)?;
    let participant = config.role().sdk_participant();
    let identity =
        zebra_identity(config).map_err(|_| ActorCommandError::ClaimConfigurationUnavailable)?;
    let mut rpc_config = if let Some(api_key) = material.zebra_api_key() {
        HttpZebraRpcConfig::public_https(config.zebra_endpoint().as_str())
            .and_then(|route| route.with_public_api_key(api_key))
            .map_err(|_| ActorCommandError::ClaimConfigurationUnavailable)?
    } else {
        HttpZebraRpcConfig::new(config.zebra_endpoint().as_str())
    }
    .with_request_timeout(Duration::from_secs(30))
    .with_max_concurrent_requests(1);
    if let Some(cookie) = material.zebra_cookie() {
        rpc_config = rpc_config
            .with_cookie_credentials(cookie)
            .map_err(|_| ActorCommandError::ClaimConfigurationUnavailable)?;
    }
    let rpc = HttpZebraRpc::connect(&rpc_config)
        .map_err(|_| ActorCommandError::ClaimConfigurationUnavailable)?;
    let signer = RoleKeyedZcashSigner::new(
        participant,
        SecretKey::from_slice(material.zcash_secret_key())
            .map_err(|_| ActorCommandError::ClaimMaterialUnavailable)?,
    );
    let store = SqliteZecRecoveryStore::open_claim_capable(
        config.role_state_db(),
        participant,
        material.into_claim_recovery_key(),
    )
    .map_err(|_| ActorCommandError::ClaimStoreUnavailable)?;
    let factory = CapabilityFileBridgeClientFactory::new(
        config.bridge_endpoint().as_str(),
        config.bridge_capability_file(),
        config.run_id().clone(),
        config.bridge_runtime().clone(),
        config.bridge_request_timeout(),
    );
    let contexts = ActorBridgeRequestContextSource::new(ConfiguredDiscoveryWindow(
        config.lez_discovery_window(),
    ));
    let funding = SqliteCanonicalLezFundingSource::new(store.clone(), participant);
    let lez = ContextOwningLezBridgePorts::new(
        config.run_id().clone(),
        config.bridge_runtime().clone(),
        participant,
        factory,
        contexts,
        store.clone(),
        funding,
        SqliteBridgeOperationJournal::open(config.bridge_journal_db())
            .map_err(|_| ActorCommandError::ClaimStoreUnavailable)?,
    )
    .map_err(|_| ActorCommandError::ClaimConfigurationUnavailable)?;
    let zcash = ZebraRpcZcashPort::new(rpc, signer, identity, participant)
        .map_err(|_| ActorCommandError::ClaimConfigurationUnavailable)?
        .with_counterparty_scan_blocks(config.counterparty_scan_blocks());
    let sdk = ZecPairSdk::new(participant, (), (), lez, zcash, store);
    let mut active = sdk
        .resume_all_capable(config.swap_id())
        .await
        .map_err(|_| ActorCommandError::ClaimReplayUnavailable)?
        .ok_or(ActorCommandError::NotActivated)?;

    ensure_claim_phase(active.status())?;
    let outcome = if active.status() == Phase::Completed {
        ActorEffectOutcomeV1::Completed
    } else {
        active
            .drive_claim()
            .await
            .map(claim_outcome)
            .map_err(|_| ActorCommandError::ClaimUnavailable)?
    };
    Ok(ActorEffectOutputV1::from_active(
        config.role(),
        ActorEffectCommandV1::Claim,
        outcome,
        &active,
    ))
}

const fn ensure_claim_phase(phase: Phase) -> Result<(), ActorCommandError> {
    if matches!(
        phase,
        Phase::BothLegsLocked | Phase::ClaimEvidenceAvailable | Phase::Completed
    ) {
        Ok(())
    } else {
        Err(ActorCommandError::ClaimUnavailable)
    }
}

const fn claim_outcome(outcome: ClaimDriveOutcome) -> ActorEffectOutcomeV1 {
    match outcome {
        ClaimDriveOutcome::Submitted(step) => ActorEffectOutcomeV1::Submitted {
            operation: claim_operation(step),
        },
        ClaimDriveOutcome::AwaitingStableObservation(step) => {
            ActorEffectOutcomeV1::AwaitingObservation {
                operation: claim_operation(step),
            }
        }
        ClaimDriveOutcome::AwaitingSafeZcashFunding(_) => {
            ActorEffectOutcomeV1::AwaitingSafeZcashFunding
        }
        ClaimDriveOutcome::Projected { step, .. } => ActorEffectOutcomeV1::Projected {
            operation: claim_operation(step),
        },
        ClaimDriveOutcome::Completed { .. } => ActorEffectOutcomeV1::Completed,
    }
}

/// Runs only the agreement-derived timeout-recovery state machine.
///
/// The configured role is passed unchanged into both chain adapters and the
/// SDK. Consequently, a non-owner can only observe its counterparty's refund;
/// it cannot prepare, sign, or submit that counterparty-owned effect. The SDK
/// retains LEZ-before-Zcash ordering, fresh funding/deadline admission, and its
/// persist-before-send journal.
async fn recover(config: &ActorConfig) -> Result<ActorEffectOutputV1, ActorCommandError> {
    let material = config
        .load_drive_material()
        .map_err(|_| ActorCommandError::RecoveryMaterialUnavailable)?;
    let participant = config.role().sdk_participant();
    let identity =
        zebra_identity(config).map_err(|_| ActorCommandError::RecoveryConfigurationUnavailable)?;
    let mut rpc_config = if let Some(api_key) = material.zebra_api_key() {
        HttpZebraRpcConfig::public_https(config.zebra_endpoint().as_str())
            .and_then(|route| route.with_public_api_key(api_key))
            .map_err(|_| ActorCommandError::RecoveryConfigurationUnavailable)?
    } else {
        HttpZebraRpcConfig::new(config.zebra_endpoint().as_str())
    }
    .with_request_timeout(Duration::from_secs(30))
    .with_max_concurrent_requests(1);
    if let Some(cookie) = material.zebra_cookie() {
        rpc_config = rpc_config
            .with_cookie_credentials(cookie)
            .map_err(|_| ActorCommandError::RecoveryConfigurationUnavailable)?;
    }
    let rpc = HttpZebraRpc::connect(&rpc_config)
        .map_err(|_| ActorCommandError::RecoveryConfigurationUnavailable)?;
    let signer = RoleKeyedZcashSigner::new(
        participant,
        SecretKey::from_slice(material.zcash_secret_key())
            .map_err(|_| ActorCommandError::RecoveryMaterialUnavailable)?,
    );
    let store = SqliteZecRecoveryStore::open_claim_capable(
        config.role_state_db(),
        participant,
        material.into_claim_recovery_key(),
    )
    .map_err(|_| ActorCommandError::RecoveryStoreUnavailable)?;
    let factory = CapabilityFileBridgeClientFactory::new(
        config.bridge_endpoint().as_str(),
        config.bridge_capability_file(),
        config.run_id().clone(),
        config.bridge_runtime().clone(),
        config.bridge_request_timeout(),
    );
    let contexts = ActorBridgeRequestContextSource::new(ConfiguredDiscoveryWindow(
        config.lez_discovery_window(),
    ));
    let funding = SqliteCanonicalLezFundingSource::new(store.clone(), participant);
    let lez = ContextOwningLezBridgePorts::new(
        config.run_id().clone(),
        config.bridge_runtime().clone(),
        participant,
        factory,
        contexts,
        store.clone(),
        funding,
        SqliteBridgeOperationJournal::open(config.bridge_journal_db())
            .map_err(|_| ActorCommandError::RecoveryStoreUnavailable)?,
    )
    .map_err(|_| ActorCommandError::RecoveryConfigurationUnavailable)?;
    let zcash = ZebraRpcZcashPort::new(rpc, signer, identity, participant)
        .map_err(|_| ActorCommandError::RecoveryConfigurationUnavailable)?
        .with_counterparty_scan_blocks(config.counterparty_scan_blocks());
    let sdk = ZecPairSdk::new(participant, (), (), lez, zcash, store);
    let mut active = sdk
        .resume_all_capable(config.swap_id())
        .await
        .map_err(|_| ActorCommandError::RecoveryReplayUnavailable)?
        .ok_or(ActorCommandError::NotActivated)?;

    ensure_recovery_phase(active.status())?;
    let outcome = active
        .drive_refund()
        .await
        .map_err(|_| ActorCommandError::RecoveryUnavailable)?;
    Ok(ActorEffectOutputV1::from_active(
        config.role(),
        ActorEffectCommandV1::Recover,
        recovery_outcome(outcome),
        &active,
    ))
}

const fn ensure_recovery_phase(phase: Phase) -> Result<(), ActorCommandError> {
    if matches!(
        phase,
        Phase::AwaitingTakerConfirmations
            | Phase::TakerLockConfirmed
            | Phase::BothLegsLocked
            | Phase::TakerLockReorged
            | Phase::MakerLockReorged
            | Phase::MakerLegRefunded
            | Phase::TakerLegRefunded
            | Phase::Refunded
    ) {
        Ok(())
    } else {
        Err(ActorCommandError::RecoveryUnavailable)
    }
}

const fn recovery_outcome(outcome: RefundDriveOutcome) -> ActorEffectOutcomeV1 {
    match outcome {
        RefundDriveOutcome::Submitted(step) => ActorEffectOutcomeV1::Submitted {
            operation: refund_operation(step),
        },
        RefundDriveOutcome::AwaitingStableObservation(step) => {
            ActorEffectOutcomeV1::AwaitingObservation {
                operation: refund_operation(step),
            }
        }
        RefundDriveOutcome::AwaitingFunding { step, reason } => {
            ActorEffectOutcomeV1::AwaitingFunding {
                operation: refund_operation(step),
                reason: refund_funding_reason(reason),
            }
        }
        RefundDriveOutcome::AwaitingDeadline(step) => ActorEffectOutcomeV1::AwaitingDeadline {
            operation: refund_operation(step),
        },
        RefundDriveOutcome::SubmissionRejected(step) => ActorEffectOutcomeV1::SubmissionRejected {
            operation: refund_operation(step),
        },
        RefundDriveOutcome::SubmissionOutcomeUnknown(step) => {
            ActorEffectOutcomeV1::SubmissionOutcomeUnknown {
                operation: refund_operation(step),
            }
        }
        RefundDriveOutcome::Projected { step, .. } => ActorEffectOutcomeV1::Projected {
            operation: refund_operation(step),
        },
        RefundDriveOutcome::Refunded { .. } => ActorEffectOutcomeV1::Refunded,
    }
}

const fn refund_operation(step: RefundStepV1) -> ActorOperationV1 {
    match step {
        RefundStepV1::Lez => ActorOperationV1::LezRefund,
        RefundStepV1::Zcash => ActorOperationV1::ZcashRefund,
    }
}

const fn refund_funding_reason(
    reason: RefundFundingWaitReasonV1,
) -> ActorRefundFundingWaitReasonV1 {
    match reason {
        RefundFundingWaitReasonV1::Absent => ActorRefundFundingWaitReasonV1::Absent,
        RefundFundingWaitReasonV1::Spent => ActorRefundFundingWaitReasonV1::Spent,
        RefundFundingWaitReasonV1::Reorged => ActorRefundFundingWaitReasonV1::Reorged,
    }
}

fn zebra_identity(config: &ActorConfig) -> Result<ZebraChainIdentity, ActorCommandError> {
    let network = match config.zcash_network() {
        ZcashNetworkConfig::Main => NetworkType::Main,
        ZcashNetworkConfig::Test => NetworkType::Test,
        ZcashNetworkConfig::Regtest => NetworkType::Regtest,
    };
    let rpc_chain = match config.zebra_rpc_chain() {
        ZebraRpcChainConfig::Main => ZebraRpcChain::Main,
        ZebraRpcChainConfig::Test => ZebraRpcChain::Test,
    };
    let branch = u32::from_str_radix(config.zcash_consensus_branch_id(), 16)
        .ok()
        .and_then(|raw| BranchId::try_from(raw).ok())
        .ok_or(ActorCommandError::DriveConfigurationUnavailable)?;
    let mut genesis = *config.zcash_genesis_hash().as_bytes();
    genesis.reverse();
    ZebraChainIdentity::new(network, rpc_chain, branch, BlockHash(genesis))
        .map_err(|_| ActorCommandError::DriveConfigurationUnavailable)
}

fn funding_outpoints(config: &ActorConfig) -> Vec<OutPoint> {
    config
        .zcash_funding_outpoints()
        .iter()
        .map(|candidate| {
            let mut transaction_id = *candidate.transaction_id().as_bytes();
            transaction_id.reverse();
            OutPoint::new(transaction_id, candidate.output_index())
        })
        .collect()
}

const fn first_lock_operation(step: FirstLockStepV1) -> ActorOperationV1 {
    match step {
        FirstLockStepV1::LezInitialize => ActorOperationV1::LezInitialize,
        FirstLockStepV1::LezFund => ActorOperationV1::LezFund,
        FirstLockStepV1::ZcashFund => ActorOperationV1::ZcashFund,
    }
}

const fn claim_operation(step: ClaimStepV1) -> ActorOperationV1 {
    match step {
        ClaimStepV1::RevealingLez => ActorOperationV1::LezRevealingClaim,
        ClaimStepV1::FollowupZcash => ActorOperationV1::ZcashFollowupClaim,
    }
}

async fn status(config: &ActorConfig) -> Result<ActorStatusV1, ActorCommandError> {
    match fs::symlink_metadata(config.role_state_db()) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(ActorStatusV1::not_activated(config.role()));
        }
        Err(_) => return Err(ActorCommandError::StatusUnavailable),
    }

    let claim_key = config
        .load_status_material()
        .map_err(|_| ActorCommandError::StatusMaterialUnavailable)?
        .into_claim_recovery_key();
    let store = SqliteZecRecoveryStore::open_claim_capable_existing(
        config.role_state_db(),
        config.role().sdk_participant(),
        claim_key,
    )
    .map_err(|_| ActorCommandError::StatusStoreUnavailable)?;
    let sdk: ZecPairSdk<(), (), (), (), SqliteZecRecoveryStore> =
        ZecPairSdk::new(config.role().sdk_participant(), (), (), (), (), store);
    let active = sdk
        .resume_all_capable(config.swap_id())
        .await
        .map_err(|_| ActorCommandError::StatusReplayUnavailable)?;

    Ok(match active {
        Some(active) => ActorStatusV1::active(
            config.role(),
            active.status(),
            active.revision(),
            active.next_action(),
        ),
        None => ActorStatusV1::not_activated(config.role()),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn status_projection_preserves_typed_lifecycle_values_and_wire_schema() {
        let not_activated = ActorStatusV1::not_activated(ActorRole::Taker);
        assert_eq!(not_activated.role(), ActorRole::Taker);
        assert_eq!(
            not_activated.projection(),
            ActorStatusProjectionV1::NotActivated
        );
        assert_eq!(
            serde_json::to_value(&not_activated).expect("status serializes"),
            json!({
                "schema_version": 1,
                "role": "taker",
                "state": "not_activated"
            })
        );

        let phases = [
            Phase::Offered,
            Phase::AwaitingTakerConfirmations,
            Phase::TakerLockConfirmed,
            Phase::AwaitingMakerConfirmations,
            Phase::BothLegsLocked,
            Phase::TakerLockReorged,
            Phase::MakerLockReorged,
            Phase::ClaimEvidenceAvailable,
            Phase::Completed,
            Phase::MakerLegRefunded,
            Phase::TakerLegRefunded,
            Phase::Refunded,
            Phase::MakerRecoveryAvailable,
        ];
        for phase in phases {
            let status =
                ActorStatusV1::active(ActorRole::Maker, phase, 17, ZecLifecycleAction::Wait);
            assert_eq!(status.role(), ActorRole::Maker);
            assert_eq!(
                status.projection(),
                ActorStatusProjectionV1::Active {
                    phase,
                    revision: 17,
                    next_action: ZecLifecycleAction::Wait,
                }
            );
        }

        let actions = [
            ZecLifecycleAction::Wait,
            ZecLifecycleAction::CreateAndFundLez,
            ZecLifecycleAction::FundZcash,
            ZecLifecycleAction::ClaimLez,
            ZecLifecycleAction::ClaimZcash,
            ZecLifecycleAction::RefundZcash,
            ZecLifecycleAction::Complete,
        ];
        for next_action in actions {
            let status =
                ActorStatusV1::active(ActorRole::Maker, Phase::BothLegsLocked, 23, next_action);
            assert_eq!(
                status.projection(),
                ActorStatusProjectionV1::Active {
                    phase: Phase::BothLegsLocked,
                    revision: 23,
                    next_action,
                }
            );
        }

        let active = ActorStatusV1::active(
            ActorRole::Maker,
            Phase::BothLegsLocked,
            23,
            ZecLifecycleAction::ClaimLez,
        );
        assert_eq!(
            serde_json::to_value(&active).expect("status serializes"),
            json!({
                "schema_version": 1,
                "role": "maker",
                "state": "active",
                "phase": "both_legs_locked",
                "revision": 23,
                "next_action": "claim_lez"
            })
        );
    }

    #[test]
    fn claim_admission_rejects_every_non_claim_phase() {
        for phase in [
            Phase::Offered,
            Phase::AwaitingTakerConfirmations,
            Phase::TakerLockConfirmed,
            Phase::TakerLockReorged,
            Phase::MakerLockReorged,
            Phase::MakerLegRefunded,
            Phase::TakerLegRefunded,
            Phase::Refunded,
            Phase::MakerRecoveryAvailable,
        ] {
            assert_eq!(
                ensure_claim_phase(phase),
                Err(ActorCommandError::ClaimUnavailable),
                "phase {phase:?} must fail before a claim RPC"
            );
        }
        for phase in [
            Phase::BothLegsLocked,
            Phase::ClaimEvidenceAvailable,
            Phase::Completed,
        ] {
            assert_eq!(ensure_claim_phase(phase), Ok(()));
        }
    }

    #[test]
    fn claim_outputs_are_action_specific_bounded_json() {
        assert_eq!(
            serde_json::to_value(ActorEffectCommandV1::Claim).unwrap(),
            json!("claim")
        );
        for (outcome, expected) in [
            (
                ClaimDriveOutcome::Submitted(ClaimStepV1::RevealingLez),
                json!({"outcome":"submitted","operation":"lez_revealing_claim"}),
            ),
            (
                ClaimDriveOutcome::AwaitingStableObservation(ClaimStepV1::FollowupZcash),
                json!({"outcome":"awaiting_observation","operation":"zcash_followup_claim"}),
            ),
            (
                ClaimDriveOutcome::AwaitingSafeZcashFunding(
                    lez_zec_swap_sdk::ZcashFundingWaitReasonV1::Absent,
                ),
                json!({"outcome":"awaiting_safe_zcash_funding"}),
            ),
            (
                ClaimDriveOutcome::Projected {
                    step: ClaimStepV1::RevealingLez,
                    revision: 4,
                },
                json!({"outcome":"projected","operation":"lez_revealing_claim"}),
            ),
            (
                ClaimDriveOutcome::Completed { revision: 5 },
                json!({"outcome":"completed"}),
            ),
        ] {
            assert_eq!(
                serde_json::to_value(claim_outcome(outcome)).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn recovery_admission_rejects_every_non_refund_phase() {
        for phase in [
            Phase::Offered,
            Phase::AwaitingMakerConfirmations,
            Phase::ClaimEvidenceAvailable,
            Phase::Completed,
            Phase::MakerRecoveryAvailable,
        ] {
            assert_eq!(
                ensure_recovery_phase(phase),
                Err(ActorCommandError::RecoveryUnavailable),
                "phase {phase:?} must fail before a recovery RPC"
            );
        }
        for phase in [
            Phase::AwaitingTakerConfirmations,
            Phase::TakerLockConfirmed,
            Phase::BothLegsLocked,
            Phase::TakerLockReorged,
            Phase::MakerLockReorged,
            Phase::MakerLegRefunded,
            Phase::TakerLegRefunded,
            Phase::Refunded,
        ] {
            assert_eq!(ensure_recovery_phase(phase), Ok(()));
        }
    }

    #[test]
    fn recovery_outcomes_are_action_specific_bounded_json() {
        for (outcome, expected) in [
            (
                RefundDriveOutcome::Submitted(RefundStepV1::Lez),
                json!({"outcome":"submitted","operation":"lez_refund"}),
            ),
            (
                RefundDriveOutcome::AwaitingStableObservation(RefundStepV1::Zcash),
                json!({"outcome":"awaiting_observation","operation":"zcash_refund"}),
            ),
            (
                RefundDriveOutcome::AwaitingFunding {
                    step: RefundStepV1::Lez,
                    reason: RefundFundingWaitReasonV1::Reorged,
                },
                json!({
                    "outcome":"awaiting_funding",
                    "operation":"lez_refund",
                    "reason":"reorged"
                }),
            ),
            (
                RefundDriveOutcome::AwaitingDeadline(RefundStepV1::Lez),
                json!({"outcome":"awaiting_deadline","operation":"lez_refund"}),
            ),
            (
                RefundDriveOutcome::SubmissionRejected(RefundStepV1::Zcash),
                json!({"outcome":"submission_rejected","operation":"zcash_refund"}),
            ),
            (
                RefundDriveOutcome::SubmissionOutcomeUnknown(RefundStepV1::Lez),
                json!({
                    "outcome":"submission_outcome_unknown",
                    "operation":"lez_refund"
                }),
            ),
            (
                RefundDriveOutcome::Projected {
                    step: RefundStepV1::Zcash,
                    revision: 4,
                },
                json!({"outcome":"projected","operation":"zcash_refund"}),
            ),
            (
                RefundDriveOutcome::Refunded { revision: 4 },
                json!({"outcome":"refunded"}),
            ),
        ] {
            assert_eq!(
                serde_json::to_value(recovery_outcome(outcome)).expect("outcome serializes"),
                expected
            );
        }
    }
}
