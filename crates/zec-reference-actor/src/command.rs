use std::{fs, io};

use lez_swap_core::Phase;
use lez_swap_store::SqliteZecRecoveryStore;
use lez_zec_swap_sdk::{ZecLifecycleAction, ZecPairSdk};
use serde::Serialize;
use thiserror::Error;

use crate::{ActorCommand, ActorConfig, ActorRole};

const STATUS_SCHEMA_VERSION: u16 = 1;

/// Secret-free, versioned output from one actor command.
#[derive(Debug, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ActorCommandOutputV1 {
    /// Durable lifecycle status projected without contacting either chain.
    Status(ActorStatusV1),
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
    /// Activate and drive are not wired to real chain capabilities yet.
    #[error("actor command is unavailable")]
    CommandUnavailable,
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
        ActorCommand::Activate | ActorCommand::Drive => Err(ActorCommandError::CommandUnavailable),
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
