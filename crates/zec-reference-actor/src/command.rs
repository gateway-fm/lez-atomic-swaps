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
    ObserveMakerLockOutcome, ObserveTakerFirstLockOutcome, RecoveryStore, ZecLifecycleAction,
    ZecPairSdk,
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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
enum ActorEffectOutcomeV1 {
    Activated,
    Submitted { operation: ActorOperationV1 },
    AwaitingObservation { operation: ActorOperationV1 },
    AwaitingSafeZcashFunding,
    Unchanged { operation: ActorOperationV1 },
    Projected { operation: ActorOperationV1 },
    Completed,
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
}

/// Versioned role-local lifecycle status.
#[derive(Debug, Eq, PartialEq, Serialize)]
pub struct ActorStatusV1 {
    schema_version: u16,
    role: ActorRole,
    #[serde(flatten)]
    state: ActorStateV1,
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
}

#[derive(Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum ActorStateV1 {
    NotActivated,
    Active {
        phase: ActorPhaseV1,
        revision: u64,
        next_action: ActorNextActionV1,
    },
}

#[derive(Debug, Eq, PartialEq, Serialize)]
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

#[derive(Debug, Eq, PartialEq, Serialize)]
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
        (_, Phase::BothLegsLocked | Phase::ClaimEvidenceAvailable) => match active
            .drive_claim()
            .await
            .map_err(|_| ActorCommandError::DriveUnavailable)?
        {
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
        },
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
