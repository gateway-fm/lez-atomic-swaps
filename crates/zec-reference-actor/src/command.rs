use std::{
    convert::Infallible,
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use lez_bridge_adapter::{
    ActorBridgeRequestContextSource, BridgeDiscoveryWindowSource,
    CapabilityFileBridgeClientFactory, ContextOwningLezBridgePorts, ContextOwningLezPortError,
    NativeRevealingClaimAdapterError, PrepareNativeFirstLockError, SqliteCanonicalLezFundingSource,
    SupersededFirstLockPlanV1, validate_runtime_binding,
};
use lez_bridge_client::BridgeClientError;
use lez_bridge_protocol::{DiscoveryWindow, ErrorCode};
use lez_swap_core::{Participant, Phase, SwapDirection, UnixSeconds};
use lez_swap_store::{BridgeOperationKey, SqliteBridgeOperationJournal, SqliteZecRecoveryStore};
use lez_zebra_node_adapter::{
    ExactOutpointZcashFundingPlanner, HttpZebraRpc, HttpZebraRpcConfig, RoleKeyedZcashSigner,
    ZebraChainIdentity, ZebraRpcChain, ZebraRpcZcashPort,
};
use lez_zec_swap_sdk::{
    AcceptedZecAgreementV1, ClaimDriveOutcome, ClaimPreimage, ClaimStepV1, FirstLockDriveOutcome,
    FirstLockPlanV1, FirstLockStepV1, FirstLockSupersessionV1, MAX_FIRST_LOCK_REPLANS,
    MAX_REVEALING_CLAIM_REPLANS, MakerFundingEligibilityOutcome, MakerLockDriveOutcome,
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
enum ActorEffectOutcomeV1 {
    Activated,
    Submitted {
        operation: ActorOperationV1,
        /// The re-plans this drive made before the bytes went out (concurrent
        /// swaps of one signer take each other's nonces); absent when none.
        #[serde(skip_serializing_if = "Vec::is_empty")]
        replans: Vec<ActorReplanV1>,
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
        lez_first_lock_replans: Vec<ActorReplanV1>,
        lez_refund_at_ms: Option<u64>,
        revealing_claim_hold_attention_ms: Option<u64>,
    ) -> Self {
        Self {
            schema_version: STATUS_SCHEMA_VERSION,
            role,
            state: ActorStateV1::Active {
                phase: phase.into(),
                revision,
                next_action: next_action.into(),
                lez_first_lock_replans,
                lez_refund_at_ms,
                revealing_claim_hold_attention_ms,
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
        match &self.state {
            ActorStateV1::NotActivated => ActorStatusProjectionV1::NotActivated,
            ActorStateV1::Active {
                phase,
                revision,
                next_action,
                ..
            } => ActorStatusProjectionV1::Active {
                phase: phase.as_phase(),
                revision: *revision,
                next_action: next_action.as_lifecycle_action(),
            },
        }
    }

    /// The re-plans of the LEZ first lock recorded for this swap, oldest first.
    #[must_use]
    pub fn lez_first_lock_replans(&self) -> &[ActorReplanV1] {
        match &self.state {
            ActorStateV1::NotActivated => &[],
            ActorStateV1::Active {
                lez_first_lock_replans,
                ..
            } => lez_first_lock_replans,
        }
    }

    /// The agreement's LEZ refund time (Unix milliseconds) of an active
    /// actor: a revealing LEZ claim must be final before it. The Nodes read
    /// it to surface a claim held behind another swap's as its window closes.
    #[must_use]
    pub const fn lez_refund_at_ms(&self) -> Option<u64> {
        match &self.state {
            ActorStateV1::NotActivated => None,
            ActorStateV1::Active {
                lez_refund_at_ms, ..
            } => *lez_refund_at_ms,
        }
    }

    /// How near its LEZ refund time a held revealing claim of an active
    /// actor needs attention ([`revealing_claim_hold_attention_ms`]).
    #[must_use]
    pub const fn revealing_claim_hold_attention_ms(&self) -> Option<u64> {
        match &self.state {
            ActorStateV1::NotActivated => None,
            ActorStateV1::Active {
                revealing_claim_hold_attention_ms,
                ..
            } => *revealing_claim_hold_attention_ms,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum ActorStateV1 {
    NotActivated,
    Active {
        phase: ActorPhaseV1,
        revision: u64,
        next_action: ActorNextActionV1,
        /// Every re-plan of the LEZ first lock, oldest first; absent when none.
        #[serde(skip_serializing_if = "Vec::is_empty")]
        lez_first_lock_replans: Vec<ActorReplanV1>,
        /// The agreement's LEZ refund time, Unix milliseconds.
        #[serde(skip_serializing_if = "Option::is_none")]
        lez_refund_at_ms: Option<u64>,
        /// How near that time a held revealing claim needs attention, ms.
        #[serde(skip_serializing_if = "Option::is_none")]
        revealing_claim_hold_attention_ms: Option<u64>,
    },
}

/// One re-plan of the LEZ first lock: both nonces and both transaction ids.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ActorReplanV1 {
    replan_index: u32,
    step: ActorOperationV1,
    superseded_nonce: u128,
    replanned_nonce: u128,
    superseded_transaction_id: String,
    replanned_transaction_id: String,
    /// For a revealing claim: the agreement's LEZ refund time minus the clock
    /// at the re-plan, milliseconds (the claim must land before it).
    #[serde(skip_serializing_if = "Option::is_none")]
    lez_refund_margin_ms: Option<i64>,
}

impl From<&lez_zec_swap_sdk::ClaimSupersessionV1> for ActorReplanV1 {
    fn from(value: &lez_zec_swap_sdk::ClaimSupersessionV1) -> Self {
        Self {
            replan_index: value.replan_index(),
            step: claim_operation(ClaimStepV1::RevealingLez),
            superseded_nonce: value.superseded_nonce(),
            replanned_nonce: value.replanned_nonce(),
            superseded_transaction_id: hex::encode(value.superseded_id()),
            replanned_transaction_id: hex::encode(value.replanned_id()),
            lez_refund_margin_ms: Some(value.lez_refund_margin_ms()),
        }
    }
}

impl From<&FirstLockSupersessionV1> for ActorReplanV1 {
    fn from(value: &FirstLockSupersessionV1) -> Self {
        let (superseded, replanned) = value.step_identities();
        Self {
            replan_index: value.replan_index(),
            step: first_lock_operation(value.step()),
            superseded_nonce: value.superseded_nonce(),
            replanned_nonce: value.replanned_nonce(),
            superseded_transaction_id: hex::encode(superseded),
            replanned_transaction_id: hex::encode(replanned),
            lez_refund_margin_ms: None,
        }
    }
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
    /// The LEZ depositor cannot fund the escrow amount; nothing was submitted.
    #[error("LEZ depositor balance is insufficient for the escrow amount")]
    LezDepositorBalanceInsufficient,
    /// The LEZ first lock was re-planned the bounded number of times and its
    /// nonce was taken again; nothing was submitted, the swap stops by name.
    #[error(
        "LEZ first lock was re-planned {MAX_FIRST_LOCK_REPLANS} times; its nonce keeps being taken"
    )]
    FirstLockReplanExhausted,
    /// The owned revealing LEZ claim was re-planned the bounded number of
    /// times and its nonce was taken again; the claim was never abandoned and
    /// the swap needs attention by name (FINDINGS 24).
    #[error(
        "revealing LEZ claim was re-planned {MAX_REVEALING_CLAIM_REPLANS} times; its nonce keeps being taken"
    )]
    RevealingClaimReplanExhausted,
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
    let material = config.load_drive_material().map_err(|error| {
        eprintln!("actor drive material unavailable: {error:?}");
        ActorCommandError::DriveMaterialUnavailable
    })?;
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
                .map_err(unavailable_drive)?
                .is_none()
            {
                let plan = match active.next_action() {
                    ZecLifecycleAction::CreateAndFundLez => lez_first_lock_or_refusal(
                        lez.prepare_native_first_lock(active.agreement()).await,
                        config,
                    )?,
                    ZecLifecycleAction::FundZcash => planner
                        .plan(active.agreement(), funding_outpoints(config))
                        .await
                        .map_err(unavailable_drive)?,
                    _ => return Err(ActorCommandError::DriveConfigurationUnavailable),
                };
                active
                    .stage_first_lock(plan)
                    .await
                    .map_err(unavailable_drive)?;
            }
            // One drive attempt re-plans and resubmits in place while the
            // sidecar reports the planned nonce taken (D1): the port bounds
            // the re-plans per swap, the swap fails by name past the bound.
            let mut replans = Vec::new();
            loop {
                match active.drive_first_lock().await.map_err(unavailable_drive)? {
                    FirstLockDriveOutcome::Submitted(step) => {
                        break ActorEffectOutcomeV1::Submitted {
                            operation: first_lock_operation(step),
                            replans,
                        };
                    }
                    FirstLockDriveOutcome::AwaitingStableObservation(step) => {
                        break ActorEffectOutcomeV1::AwaitingObservation {
                            operation: first_lock_operation(step),
                        };
                    }
                    FirstLockDriveOutcome::ReadyForFundingProjection(evidence) => {
                        active
                            .project_first_lock(evidence)
                            .await
                            .map_err(unavailable_drive)?;
                        break ActorEffectOutcomeV1::Projected {
                            operation: ActorOperationV1::TakerFirstLock,
                        };
                    }
                    FirstLockDriveOutcome::Superseded {
                        step,
                        planned_nonce,
                        live_nonce,
                    } => {
                        let open = store
                            .load_first_lock_intent(active.agreement().coordinator().id())
                            .await
                            .map_err(unavailable_drive)?
                            .ok_or(ActorCommandError::DriveUnavailable)?;
                        let replanned = replan_lez_first_lock(
                            &lez,
                            active.agreement(),
                            open.plan(),
                            step,
                            planned_nonce,
                            live_nonce,
                            replans.len(),
                        )
                        .await?;
                        active
                            .restage_first_lock(replanned.plan, &replanned.supersession)
                            .await
                            .map_err(unavailable_drive)?;
                        replans.push(ActorReplanV1::from(&replanned.supersession));
                    }
                }
            }
        }
        (Participant::Maker, Phase::Offered | Phase::AwaitingTakerConfirmations) => match active
            .observe_taker_first_lock()
            .await
            .map_err(unavailable_drive)?
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
                .map_err(unavailable_drive)?
            {
                Some(intent) => intent.plan().clone(),
                None => match active.agreement().direction() {
                    SwapDirection::TakerSellsForeign => lez_first_lock_or_refusal(
                        lez.prepare_native_first_lock(active.agreement()).await,
                        config,
                    )?,
                    SwapDirection::TakerSellsLez => planner
                        .plan(active.agreement(), funding_outpoints(config))
                        .await
                        .map_err(unavailable_drive)?,
                },
            };
            // As the Taker's first lock: re-plan and resubmit in place while
            // the sidecar reports the Maker's LEZ nonce taken (D1).
            let mut plan = plan;
            let mut replans = Vec::new();
            loop {
                match active
                    .drive_maker_lock(plan.clone())
                    .await
                    .map_err(unavailable_drive)?
                {
                    MakerLockDriveOutcome::AlreadyLocked { .. } => {
                        break ActorEffectOutcomeV1::Unchanged {
                            operation: ActorOperationV1::MakerLock,
                        };
                    }
                    MakerLockDriveOutcome::AwaitingEligibility(
                        MakerFundingEligibilityOutcome::CanonicalStateChanged(_),
                    ) => {
                        break ActorEffectOutcomeV1::Projected {
                            operation: ActorOperationV1::TakerFirstLock,
                        };
                    }
                    MakerLockDriveOutcome::AwaitingEligibility(_) => {
                        break ActorEffectOutcomeV1::AwaitingObservation {
                            operation: ActorOperationV1::TakerFirstLock,
                        };
                    }
                    MakerLockDriveOutcome::Lock(FirstLockDriveOutcome::Submitted(step)) => {
                        break ActorEffectOutcomeV1::Submitted {
                            operation: first_lock_operation(step),
                            replans,
                        };
                    }
                    MakerLockDriveOutcome::Lock(
                        FirstLockDriveOutcome::AwaitingStableObservation(step),
                    ) => {
                        break ActorEffectOutcomeV1::AwaitingObservation {
                            operation: first_lock_operation(step),
                        };
                    }
                    MakerLockDriveOutcome::Lock(
                        FirstLockDriveOutcome::ReadyForFundingProjection(evidence),
                    ) => {
                        active
                            .project_maker_lock(evidence)
                            .await
                            .map_err(unavailable_drive)?;
                        break ActorEffectOutcomeV1::Projected {
                            operation: ActorOperationV1::MakerLock,
                        };
                    }
                    MakerLockDriveOutcome::Lock(FirstLockDriveOutcome::Superseded {
                        step,
                        planned_nonce,
                        live_nonce,
                    }) => {
                        let replanned = replan_lez_first_lock(
                            &lez,
                            active.agreement(),
                            &plan,
                            step,
                            planned_nonce,
                            live_nonce,
                            replans.len(),
                        )
                        .await?;
                        active
                            .restage_maker_lock(replanned.plan.clone(), &replanned.supersession)
                            .await
                            .map_err(unavailable_drive)?;
                        replans.push(ActorReplanV1::from(&replanned.supersession));
                        plan = replanned.plan;
                    }
                }
            }
        }
        (Participant::Taker, Phase::TakerLockConfirmed) => match active
            .observe_maker_lock()
            .await
            .map_err(unavailable_drive)?
        {
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
            drive_claim_with_replans(
                &mut active,
                &lez,
                config,
                ActorCommandError::DriveUnavailable,
            )
            .await?
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
    let sdk = ZecPairSdk::new(participant, (), (), lez.clone(), zcash, store);
    let mut active = sdk
        .resume_all_capable(config.swap_id())
        .await
        .map_err(|_| ActorCommandError::ClaimReplayUnavailable)?
        .ok_or(ActorCommandError::NotActivated)?;

    ensure_claim_phase(active.status())?;
    let outcome = if active.status() == Phase::Completed {
        ActorEffectOutcomeV1::Completed
    } else {
        drive_claim_with_replans(
            &mut active,
            &lez,
            config,
            ActorCommandError::ClaimUnavailable,
        )
        .await?
    };
    Ok(ActorEffectOutputV1::from_active(
        config.role(),
        ActorEffectCommandV1::Claim,
        outcome,
        &active,
    ))
}

/// One bounded drive attempt failed closed. The lifecycle error's Debug form
/// (variant names, no material) goes to stderr so the operator learns which
/// gate held, as for the claim; the exit class stays `DriveUnavailable`.
/// The marker a Node reads as "needs attention: LEZ balance"; beside the
/// actor's state like the Bitcoin actor's claim-submitted marker
/// (`<swap>/actor/<role>/lez-depositor-balance-insufficient`). Both roles
/// leave it: the Taker selling LEZ and the Maker buying the foreign coin are
/// each the LEZ depositor of their direction.
#[must_use]
pub fn lez_depositor_balance_marker(role_state_db: &Path) -> Option<PathBuf> {
    Some(
        role_state_db
            .parent()?
            .parent()?
            .join("lez-depositor-balance-insufficient"),
    )
}

/// The LEZ first-lock plan for this role, or the named refusal when the
/// bridge found the depositor short of the lot. A refusal leaves the marker
/// so the Node shows the owner why instead of this drive being retried in
/// silence; a later successful preparation removes it, and the drive is
/// retried by the Node (the Taker's re-driver, the Maker's backoff) with no
/// owner action once the account is funded.
fn lez_first_lock_or_refusal<Plan>(
    prepared: Result<Plan, ContextOwningLezPortError>,
    config: &ActorConfig,
) -> Result<Plan, ActorCommandError> {
    let marker = lez_depositor_balance_marker(config.role_state_db());
    match prepared {
        Ok(plan) => {
            if let Some(marker) = marker.as_ref() {
                let _ = fs::remove_file(marker);
            }
            Ok(plan)
        }
        Err(error) if is_depositor_balance_refusal(&error) => {
            if let Some(marker) = marker.as_ref() {
                let _ = fs::write(marker, b"lez_depositor_balance_insufficient\n");
            }
            eprintln!("actor drive refused: {error:?}");
            Err(ActorCommandError::LezDepositorBalanceInsufficient)
        }
        Err(error) => Err(unavailable_drive(error)),
    }
}

/// Re-plans the LEZ first lock whose `step` the sidecar reported superseded,
/// in the same drive attempt. Logs both nonces and both transaction ids. Past
/// the bound (the port's, or the sidecar's `ErrorCode::ReplanExhausted`) the
/// swap fails by name; anything else is an unavailable drive.
async fn replan_lez_first_lock<Factory, Contexts, Store, Funding>(
    lez: &ContextOwningLezBridgePorts<Factory, Contexts, Store, Funding>,
    agreement: &lez_zec_swap_sdk::ZecAgreementV1,
    plan: &FirstLockPlanV1,
    step: FirstLockStepV1,
    planned_nonce: u128,
    live_nonce: u128,
    replans_this_attempt: usize,
) -> Result<SupersededFirstLockPlanV1, ActorCommandError>
where
    Factory: lez_bridge_adapter::FreshLezBridgeTransportFactory,
    Factory::Transport: lez_bridge_adapter::LezBridgeTransport,
    Contexts: lez_bridge_adapter::BridgeRequestContextSource,
    Store: RecoveryStore,
{
    if replans_this_attempt >= MAX_FIRST_LOCK_REPLANS as usize {
        return Err(ActorCommandError::FirstLockReplanExhausted);
    }
    match lez.supersede_native_first_lock(agreement, plan, step).await {
        Ok(replanned) => {
            let (superseded, replanned_id) = replanned.supersession.step_identities();
            eprintln!(
                "actor drive: LEZ {step:?} superseded (planned nonce {planned_nonce}, live {live_nonce}); re-plan {} at nonce {}: tx {} → {}",
                replanned.supersession.replan_index(),
                replanned.supersession.replanned_nonce(),
                hex::encode(superseded),
                hex::encode(replanned_id)
            );
            Ok(replanned)
        }
        Err(error) if is_replan_exhausted(&error) => {
            eprintln!("actor drive refused: {error:?}");
            Err(ActorCommandError::FirstLockReplanExhausted)
        }
        Err(error) => Err(unavailable_drive(error)),
    }
}

/// Whether a re-plan was refused because the bound is spent, at the port or
/// at the sidecar (`ErrorCode::ReplanExhausted`).
fn is_replan_exhausted(error: &ContextOwningLezPortError) -> bool {
    if matches!(error, ContextOwningLezPortError::ReplanExhausted) {
        return true;
    }
    let ContextOwningLezPortError::Adapter(adapter) = error else {
        return false;
    };
    let Some(PrepareNativeFirstLockError::Transport(BridgeClientError::Remote(remote))) =
        adapter.downcast_ref::<PrepareNativeFirstLockError<BridgeClientError>>()
    else {
        return false;
    };
    remote.code() == ErrorCode::ReplanExhausted
}

/// Whether a first-lock preparation failed because the bridge refused the
/// depositor's balance (`ErrorCode::DepositorBalanceInsufficient`), reached
/// through the port, adapter and transport error layers.
fn is_depositor_balance_refusal(error: &ContextOwningLezPortError) -> bool {
    let ContextOwningLezPortError::Adapter(adapter) = error else {
        return false;
    };
    let Some(PrepareNativeFirstLockError::Transport(BridgeClientError::Remote(remote))) =
        adapter.downcast_ref::<PrepareNativeFirstLockError<BridgeClientError>>()
    else {
        return false;
    };
    remote.code() == ErrorCode::DepositorBalanceInsufficient
}

fn unavailable_drive<E: std::fmt::Debug>(error: E) -> ActorCommandError {
    eprintln!("actor drive unavailable: {error:?}");
    ActorCommandError::DriveUnavailable
}

/// As [`unavailable_drive`], for the agreement-ordered recovery attempt.
fn unavailable_recovery<E: std::fmt::Debug>(error: E) -> ActorCommandError {
    eprintln!("actor recovery unavailable: {error:?}");
    ActorCommandError::RecoveryUnavailable
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

/// The marker a revealing LEZ claim leaves the moment it is submitted, kept
/// for the life of the swap: a refund in the claim window is refused once it
/// exists (FINDINGS 24, the secret may be public).
/// (`<swap>/actor/<role>/revealing-claim-submitted`.)
#[must_use]
pub fn revealing_claim_submitted_marker(role_state_db: &Path) -> Option<PathBuf> {
    Some(
        role_state_db
            .parent()?
            .parent()?
            .join("revealing-claim-submitted"),
    )
}

/// The marker of a revealing LEZ claim submitted and not yet observed final:
/// while another swap of the same Node holds it, that Node keeps its own
/// revealing claim back (one per LEZ account in flight; FINDINGS 24). Removed
/// when the claim is projected, never on a timer.
/// (`<swap>/actor/<role>/revealing-claim-in-flight`.)
#[must_use]
pub fn revealing_claim_in_flight_marker(role_state_db: &Path) -> Option<PathBuf> {
    Some(
        role_state_db
            .parent()?
            .parent()?
            .join("revealing-claim-in-flight"),
    )
}

/// How near its own LEZ refund time a revealing LEZ claim held behind another
/// swap's (one in flight per Node) is surfaced as needing attention with the
/// holder named, so a stuck holder cannot make it miss its claim in silence
/// (REVIEW 16:33Z): a third of the agreement's LEZ refund delay (10 min under
/// a 30 min delay, 4 min under the devnet V3's 12; REVIEW 00:52Z), so a gated
/// second claim is not flagged as soon as it waits. The hold itself is kept:
/// two claims at one nonce would supersede each other.
#[must_use]
pub const fn revealing_claim_hold_attention_ms(lez_refund_delay_seconds: u64) -> u64 {
    lez_refund_delay_seconds.saturating_mul(1000) / 3
}

/// Whether a claim held now, `now_ms`, is within `attention_ms` of the swap's
/// LEZ refund time (or past it).
#[must_use]
pub const fn revealing_claim_hold_needs_attention(
    lez_refund_at_ms: u64,
    attention_ms: u64,
    now_ms: u64,
) -> bool {
    lez_refund_at_ms.saturating_sub(now_ms) <= attention_ms
}

/// The marker a Node writes beside a swap whose revealing LEZ claim it holds
/// behind another swap's while the swap's own LEZ refund time is near; it
/// names the holding swap. Removed by the Node when the claim goes ahead and
/// by the actor when the claim is submitted or the swap moves on.
/// (`<swap>/actor/<role>/revealing-claim-held-by`.)
#[must_use]
pub fn revealing_claim_held_by_marker(role_state_db: &Path) -> Option<PathBuf> {
    Some(
        role_state_db
            .parent()?
            .parent()?
            .join("revealing-claim-held-by"),
    )
}

/// Names `holder` in the held-by marker, or removes the marker (`None`).
pub fn record_revealing_claim_holder(role_state_db: &Path, holder: Option<&str>) {
    let Some(marker) = revealing_claim_held_by_marker(role_state_db) else {
        return;
    };
    match holder {
        Some(holder) if valid_swap_id_text(holder) => {
            let _ = fs::write(marker, format!("{holder}\n"));
        }
        _ => {
            let _ = fs::remove_file(marker);
        }
    }
}

/// The holding swap named by the held-by marker, when present and well formed.
#[must_use]
pub fn revealing_claim_holder(role_state_db: &Path) -> Option<String> {
    let text = fs::read_to_string(revealing_claim_held_by_marker(role_state_db)?).ok()?;
    let holder = text.trim();
    valid_swap_id_text(holder).then(|| holder.to_owned())
}

fn valid_swap_id_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':'))
}

/// One claim attempt; an owned revealing claim whose nonce was taken is
/// re-planned and resubmitted in the same attempt (FINDINGS 24), each re-plan
/// recorded with both nonces, both ids and the margin to the LEZ refund time.
/// Past the bound the swap stops by name; it is never abandoned.
async fn drive_claim_with_replans<Factory, Contexts, Funding, Zcash>(
    active: &mut lez_zec_swap_sdk::ActiveZecSwap<
        ContextOwningLezBridgePorts<Factory, Contexts, SqliteZecRecoveryStore, Funding>,
        Zcash,
        SqliteZecRecoveryStore,
    >,
    lez: &ContextOwningLezBridgePorts<Factory, Contexts, SqliteZecRecoveryStore, Funding>,
    config: &ActorConfig,
    unavailable: ActorCommandError,
) -> Result<ActorEffectOutcomeV1, ActorCommandError>
where
    Factory: lez_bridge_adapter::FreshLezBridgeTransportFactory,
    Factory::Transport: lez_bridge_adapter::LezBridgeClaimTransport,
    Contexts: lez_bridge_adapter::BridgeRequestContextSource,
    Funding: lez_bridge_adapter::CanonicalLezFundingSource,
    Zcash: lez_zec_swap_sdk::ZcashClaimPort,
{
    let mut replans = Vec::new();
    loop {
        let outcome = match active.drive_claim().await {
            Ok(outcome) => outcome,
            Err(error) => {
                // The lifecycle error names the gate that held the claim back
                // (funding depth, port, persistence); the exit code stays a
                // secret-free class, the reason goes to stderr for the operator.
                eprintln!("actor claim unavailable: {error:?}");
                return Err(unavailable);
            }
        };
        match outcome {
            ClaimDriveOutcome::Superseded {
                planned_nonce,
                live_nonce,
                ..
            } => {
                if replans.len() >= MAX_REVEALING_CLAIM_REPLANS as usize {
                    return Err(ActorCommandError::RevealingClaimReplanExhausted);
                }
                let open = active
                    .open_revealing_claim_submission()
                    .await
                    .map_err(|error| {
                        eprintln!("actor claim unavailable: {error:?}");
                        ActorCommandError::ClaimUnavailable
                    })?;
                let replanned = match lez
                    .supersede_revealing_claim(active.agreement(), &open)
                    .await
                {
                    Ok(replanned) => replanned,
                    Err(error) if is_claim_replan_exhausted(&error) => {
                        eprintln!("actor claim refused: {error:?}");
                        return Err(ActorCommandError::RevealingClaimReplanExhausted);
                    }
                    Err(error) => {
                        eprintln!("actor claim unavailable: {error:?}");
                        return Err(unavailable);
                    }
                };
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |elapsed| {
                        i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
                    });
                let refund_at_ms =
                    i64::try_from(active.agreement().lez_refund_at_ms()).unwrap_or(i64::MAX);
                let supersession = lez_zec_swap_sdk::ClaimSupersessionV1::new(
                    lez_zec_swap_sdk::ClaimSupersessionInputV1 {
                        replan_index: replanned.replan_index,
                        superseded_id: *open.expected_submission_id(),
                        superseded_nonce: replanned.superseded_nonce,
                        replanned_id: *replanned.claim.expected_submission_id(),
                        replanned_nonce: replanned.replanned_nonce,
                        lez_refund_margin_ms: refund_at_ms.saturating_sub(now_ms),
                    },
                )
                .map_err(|_| ActorCommandError::ClaimUnavailable)?;
                eprintln!(
                    "actor claim: revealing LEZ claim superseded (planned nonce {planned_nonce}, live {live_nonce}); re-plan {} at nonce {}: {} -> {}; {} s left before the LEZ refund time",
                    supersession.replan_index(),
                    supersession.replanned_nonce(),
                    hex::encode(supersession.superseded_id()),
                    hex::encode(supersession.replanned_id()),
                    supersession.lez_refund_margin_ms() / 1000
                );
                active
                    .restage_revealing_claim(&replanned.claim, &supersession)
                    .await
                    .map_err(|error| {
                        eprintln!("actor claim unavailable: {error:?}");
                        ActorCommandError::ClaimUnavailable
                    })?;
                replans.push(ActorReplanV1::from(&supersession));
            }
            ClaimDriveOutcome::Submitted(step) => {
                if step == ClaimStepV1::RevealingLez {
                    mark_revealing_claim_submitted(config);
                }
                return Ok(ActorEffectOutcomeV1::Submitted {
                    operation: claim_operation(step),
                    replans,
                });
            }
            other => {
                if !matches!(active.status(), Phase::BothLegsLocked) {
                    clear_revealing_claim_in_flight(config);
                }
                return Ok(claim_outcome(other));
            }
        }
    }
}

/// Both claim markers: submitted (for the life of the swap) and in flight.
fn mark_revealing_claim_submitted(config: &ActorConfig) {
    for marker in [
        revealing_claim_submitted_marker(config.role_state_db()),
        revealing_claim_in_flight_marker(config.role_state_db()),
    ]
    .into_iter()
    .flatten()
    {
        let _ = fs::write(marker, b"revealing LEZ claim submitted\n");
    }
    record_revealing_claim_holder(config.role_state_db(), None);
}

/// The revealing claim is observed final (the swap left both legs locked).
fn clear_revealing_claim_in_flight(config: &ActorConfig) {
    if let Some(marker) = revealing_claim_in_flight_marker(config.role_state_db()) {
        let _ = fs::remove_file(marker);
    }
    record_revealing_claim_holder(config.role_state_db(), None);
}

/// Whether a claim re-plan was refused because the bound is spent, at the
/// port or at the sidecar (`ErrorCode::ReplanExhausted`).
fn is_claim_replan_exhausted(error: &ContextOwningLezPortError) -> bool {
    if matches!(error, ContextOwningLezPortError::ReplanExhausted) {
        return true;
    }
    let ContextOwningLezPortError::Adapter(adapter) = error else {
        return false;
    };
    let Some(NativeRevealingClaimAdapterError::Transport(BridgeClientError::Remote(remote))) =
        adapter.downcast_ref::<NativeRevealingClaimAdapterError<BridgeClientError>>()
    else {
        return false;
    };
    remote.code() == ErrorCode::ReplanExhausted
}

const fn claim_outcome(outcome: ClaimDriveOutcome) -> ActorEffectOutcomeV1 {
    match outcome {
        ClaimDriveOutcome::Submitted(step) => ActorEffectOutcomeV1::Submitted {
            operation: claim_operation(step),
            replans: Vec::new(),
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
        // Re-planned in `drive_claim_with_replans` before it gets here.
        ClaimDriveOutcome::Superseded { step, .. } => ActorEffectOutcomeV1::AwaitingObservation {
            operation: claim_operation(step),
        },
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
    let outcome = active.drive_refund().await.map_err(unavailable_recovery)?;
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
            | Phase::AwaitingMakerConfirmations
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
            replans: Vec::new(),
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
        Some(active) => {
            let replans = active
                .first_lock_supersessions()
                .await
                .map_err(|_| ActorCommandError::StatusReplayUnavailable)?
                .iter()
                .map(ActorReplanV1::from)
                .collect();
            ActorStatusV1::active(
                config.role(),
                active.status(),
                active.revision(),
                active.next_action(),
                replans,
                Some(active.agreement().lez_refund_at_ms()),
                Some(revealing_claim_hold_attention_ms(
                    lez_zec_swap_sdk::ZecRefundProfile::for_id(
                        active.agreement().binding().profile_id(),
                    )
                    .lez_refund_delay()
                    .value(),
                )),
            )
        }
        None => ActorStatusV1::not_activated(config.role()),
    })
}

#[cfg(test)]
mod tests {
    use lez_bridge_adapter::{ContextOwningLezPortError, SqliteCanonicalLezFundingSourceError};
    use lez_zec_swap_sdk::ZecSdkError;
    use serde_json::json;

    use super::*;

    /// The reason the actor writes to stderr before exiting `ClaimUnavailable`
    /// is the lifecycle error's Debug form: variant names only, no material.
    /// This is the line the devnet showed when the Maker's Zcash lock sat one
    /// revision past the slot the funding source looked at.
    #[test]
    fn claim_unavailable_reason_is_exact_and_secret_free() {
        let error = ZecSdkError::LezClaim(Box::new(ContextOwningLezPortError::Recovery(Box::new(
            SqliteCanonicalLezFundingSourceError::SecondLockUnavailable,
        ))));
        assert_eq!(
            format!("actor claim unavailable: {error:?}"),
            "actor claim unavailable: LezClaim(Recovery(SecondLockUnavailable))"
        );
        let preimage = ClaimPreimage::new([0x44; 32]);
        assert!(!format!("{preimage:?}").contains("44"), "{preimage:?}");
    }

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
            let status = ActorStatusV1::active(
                ActorRole::Maker,
                phase,
                17,
                ZecLifecycleAction::Wait,
                Vec::new(),
                None,
                None,
            );
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
            let status = ActorStatusV1::active(
                ActorRole::Maker,
                Phase::BothLegsLocked,
                23,
                next_action,
                Vec::new(),
                None,
                None,
            );
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
            Vec::new(),
            None,
            None,
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
    fn an_active_status_carries_the_lez_refund_time() {
        let status = ActorStatusV1::active(
            ActorRole::Taker,
            Phase::BothLegsLocked,
            9,
            ZecLifecycleAction::ClaimLez,
            Vec::new(),
            Some(1_800_000),
            Some(600_000),
        );
        assert_eq!(status.lez_refund_at_ms(), Some(1_800_000));
        assert_eq!(status.revealing_claim_hold_attention_ms(), Some(600_000));
        assert_eq!(
            serde_json::to_value(&status).expect("status serializes")["lez_refund_at_ms"],
            json!(1_800_000)
        );
        assert_eq!(
            ActorStatusV1::not_activated(ActorRole::Taker).lez_refund_at_ms(),
            None
        );
    }

    #[test]
    fn a_held_claim_names_its_holder_only_near_its_refund_time() {
        // A third of the agreement's LEZ refund delay: 10 min under the
        // devnet V2's 30 min, 4 min under V3's 12 (REVIEW 00:52Z).
        let v2 = revealing_claim_hold_attention_ms(
            lez_zec_swap_sdk::ZecRefundProfile::for_id(
                lez_zec_swap_sdk::ZecProfileId::DeterministicLocalDevnetV2,
            )
            .lez_refund_delay()
            .value(),
        );
        let v3 = revealing_claim_hold_attention_ms(
            lez_zec_swap_sdk::ZecRefundProfile::for_id(
                lez_zec_swap_sdk::ZecProfileId::DeterministicLocalDevnetV3,
            )
            .lez_refund_delay()
            .value(),
        );
        assert_eq!(v2, 600_000);
        assert_eq!(v3, 240_000);
        for margin in [v2, v3] {
            assert!(!revealing_claim_hold_needs_attention(
                10_000_000,
                margin,
                10_000_000 - margin - 1
            ));
            assert!(revealing_claim_hold_needs_attention(
                10_000_000,
                margin,
                10_000_000 - margin
            ));
            assert!(revealing_claim_hold_needs_attention(
                10_000_000, margin, 20_000_000
            ));
        }
        // A V3 claim gated for two minutes of its twelve is not flagged.
        assert!(!revealing_claim_hold_needs_attention(720_000, v3, 120_000));

        let root = tempfile::tempdir().expect("tempdir");
        let state_db = root.path().join("taker").join("state").join("actor.sqlite");
        std::fs::create_dir_all(state_db.parent().expect("parent")).expect("dirs");
        assert_eq!(revealing_claim_holder(&state_db), None);
        record_revealing_claim_holder(&state_db, Some("swap-0a1b"));
        assert_eq!(
            revealing_claim_holder(&state_db).as_deref(),
            Some("swap-0a1b")
        );
        record_revealing_claim_holder(&state_db, Some("not a swap id\n"));
        assert_eq!(revealing_claim_holder(&state_db), None);
        record_revealing_claim_holder(&state_db, Some("swap-0a1b"));
        record_revealing_claim_holder(&state_db, None);
        assert!(
            !revealing_claim_held_by_marker(&state_db)
                .expect("marker path")
                .exists()
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
            Phase::AwaitingMakerConfirmations,
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
