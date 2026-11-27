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
    ZebraChainIdentity, ZebraRpc as _, ZebraRpcChain, ZebraRpcZcashPort,
};
use lez_zec_swap_sdk::{
    AcceptedZecAgreementV1, ClaimDriveOutcome, ClaimPreimage, ClaimStepV1, FirstLockDriveOutcome,
    FirstLockPlanV1, FirstLockStepV1, FirstLockSupersessionV1, MAX_FIRST_LOCK_REPLANS,
    MAX_REVEALING_CLAIM_REPLANS, MakerFundingEligibilityOutcome, MakerLockDriveOutcome,
    ObserveMakerLockOutcome, ObserveTakerFirstLockOutcome, REVEALING_CLAIM_REPLAN_ATTENTION,
    RecoveryStore, RefundDriveOutcome, RefundFundingWaitReasonV1, RefundStepV1, ZecLifecycleAction,
    ZecPairSdk,
};
use secp256k1::SecretKey;
use serde::Serialize;
use thiserror::Error;
use zcash_primitives::block::BlockHash;
use zcash_protocol::consensus::{BranchId, NetworkType};
use zcash_transparent::bundle::OutPoint;

use crate::{
    ActorCommand, ActorConfig, ActorRole, ZcashNetworkConfig, ZebraRpcChainConfig,
    first_lock_attempts::{
        ESCROW_REFUSAL_STOP_ATTEMPTS, ESCROW_REFUSAL_STOP_SECONDS, LezFirstLockAttemptOutcomeV1,
        clear_lez_first_lock_attempts, lez_first_lock_attempts, record_lez_first_lock_attempt,
    },
};

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
    /// What the attempt waits for, when it held its step for a reason it can
    /// bound by a chain's own clock (reporting only; never a Node decision).
    #[serde(skip_serializing_if = "Option::is_none")]
    wait: Option<ActorWaitV1>,
}

/// A wait the actor names: `kind` on `chain`, until the chain's clock passes
/// `until_unix_seconds`, read at `chain_unix_seconds`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct ActorWaitV1 {
    kind: &'static str,
    chain: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    until_unix_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    chain_unix_seconds: Option<u64>,
}

impl ActorEffectOutputV1 {
    fn from_active<Lez, Zcash, Store>(
        config: &ActorConfig,
        command: ActorEffectCommandV1,
        outcome: ActorEffectOutcomeV1,
        active: &lez_zec_swap_sdk::ActiveZecSwap<Lez, Zcash, Store>,
    ) -> Self {
        Self {
            schema_version: STATUS_SCHEMA_VERSION,
            role: config.role(),
            command,
            outcome,
            phase: active.status().into(),
            revision: active.revision(),
            next_action: reported_next_action(active, config, wall_clock_ms()).into(),
            wait: None,
        }
    }

    fn waiting_for(mut self, wait: ActorWaitV1) -> Self {
        self.wait = Some(wait);
        self
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
    /// `TakerSellsForeign`: the Taker's revealing LEZ claim.
    LezRevealingClaim,
    /// `TakerSellsForeign`: the Maker's follow-up Zcash claim.
    ZcashFollowupClaim,
    /// `TakerSellsLez`: the Taker's revealing Zcash claim.
    ZcashRevealingClaim,
    /// `TakerSellsLez`: the Maker's follow-up LEZ claim.
    LezFollowupClaim,
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

    #[allow(
        clippy::too_many_arguments,
        reason = "one field per status value; the status is built in one place"
    )]
    fn active(
        role: ActorRole,
        phase: Phase,
        revision: u64,
        next_action: ZecLifecycleAction,
        lez_first_lock_replans: Vec<ActorReplanV1>,
        lez_refund_at_ms: Option<u64>,
        revealing_claim_hold_attention_ms: Option<u64>,
        required_margin_ms: Option<u64>,
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
                required_margin_ms,
                maker_lock: None,
            },
        }
    }

    /// Names the leg the Maker's next drive locks.
    fn with_maker_lock(mut self, leg: Option<ActorMakerLockLegV1>) -> Self {
        if let ActorStateV1::Active { maker_lock, .. } = &mut self.state {
            *maker_lock = leg;
        }
        self
    }

    /// The leg the Maker's next drive locks, while its lock is the next step.
    #[must_use]
    pub const fn maker_lock(&self) -> Option<ActorMakerLockLegV1> {
        match &self.state {
            ActorStateV1::NotActivated => None,
            ActorStateV1::Active { maker_lock, .. } => *maker_lock,
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
        /// The profile's required cross-chain reaction margin `M`, ms: a Maker
        /// Node bounds a LEZ claim admission by half of it.
        #[serde(skip_serializing_if = "Option::is_none")]
        required_margin_ms: Option<u64>,
        /// The leg the Maker's next drive locks, while its lock is the next
        /// step (the Taker's lock confirmed): a Maker Node coordinates a LEZ
        /// lock with its account's other LEZ transactions and admits a Zcash
        /// funding against its open-swap capacity. Absent otherwise.
        #[serde(skip_serializing_if = "Option::is_none")]
        maker_lock: Option<ActorMakerLockLegV1>,
    },
}

/// The leg a Maker locks second.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorMakerLockLegV1 {
    /// `TakerSellsForeign`: the Maker creates and funds the LEZ escrow, two
    /// transactions of its LEZ account.
    Lez,
    /// `TakerSellsLez`: the Maker funds the Zcash HTLC.
    Zcash,
}

/// The leg the Maker's drive locks next, if its lock is the next step.
const fn pending_maker_lock(
    local: Participant,
    phase: Phase,
    direction: SwapDirection,
) -> Option<ActorMakerLockLegV1> {
    match (local, phase, direction) {
        (Participant::Maker, Phase::TakerLockConfirmed, SwapDirection::TakerSellsForeign) => {
            Some(ActorMakerLockLegV1::Lez)
        }
        (Participant::Maker, Phase::TakerLockConfirmed, SwapDirection::TakerSellsLez) => {
            Some(ActorMakerLockLegV1::Zcash)
        }
        _ => None,
    }
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

impl ActorReplanV1 {
    /// One re-plan of the owned LEZ claim `step` (the Taker's revealing LEZ
    /// claim in `TakerSellsForeign`, the Maker's follow-up LEZ claim in
    /// `TakerSellsLez`).
    fn from_claim(value: &lez_zec_swap_sdk::ClaimSupersessionV1, step: ClaimStepV1) -> Self {
        Self {
            replan_index: value.replan_index(),
            step: claim_operation(step),
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
    RefundLez,
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
            ZecLifecycleAction::RefundLez => Self::RefundLez,
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
            Self::RefundLez => ZecLifecycleAction::RefundLez,
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
    /// The LEZ escrow account of this swap already exists, initialized by a
    /// transaction that is not this swap's (another swap with the same swap
    /// id): the escrow initialization can never execute. The bridge refused
    /// it the bounded number of times over the bounded span and it is no
    /// longer submitted; the swap stops by name.
    #[error(
        "LEZ escrow account already exists, initialized by a transaction that is not this swap's; the escrow initialization can never execute and is no longer submitted"
    )]
    LezEscrowAlreadyInitialized,
    /// The owned LEZ claim (revealing in `TakerSellsForeign`, follow-up in
    /// `TakerSellsLez`) was re-planned the bounded number of times and its
    /// nonce was taken again; the claim was never abandoned and the swap needs
    /// attention by name (FINDINGS 24).
    #[error(
        "LEZ claim was re-planned {MAX_REVEALING_CLAIM_REPLANS} times; its nonce keeps being taken"
    )]
    RevealingClaimReplanExhausted,
    /// The revealing claim window closed before the claim left: it is never
    /// prepared or sent after it; the settlement is the refund (FINDINGS 26).
    /// The Taker's revealing LEZ claim (`TakerSellsForeign`) closes at the
    /// LEZ refund time minus the margin; its revealing Zcash claim
    /// (`TakerSellsLez`) also when too few blocks remain before the Maker's
    /// Zcash refund height.
    #[error("the revealing claim window closed; the claim is not sent")]
    ClaimWindowClosed,
    /// The Maker's follow-up LEZ claim (`TakerSellsLez`) can no longer land:
    /// the Taker's LEZ refund is on the chain, or the LEZ refund time passed
    /// (the claim is valid only before it) with no claim of this swap in a
    /// block. Nothing is submitted again; the swap needs attention by name
    /// (the Taker took the Zcash and the LEZ went back to it).
    #[error(
        "the LEZ claim can no longer land; the LEZ escrow was refunded or its refund time passed"
    )]
    LezClaimCannotLand,
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
    /// The revealing claim was refused, nothing sent, because the Zcash
    /// funding it reveals against is not yet `required` deep (`actual`):
    /// the same refusal as [`Self::ClaimUnavailable`], named with its counts
    /// so the Node can say the claim is queued (issue #130).
    #[error(
        "the revealing claim waits for {required} Zcash confirmations of the funding ({actual} so far)"
    )]
    ClaimAwaitingZcashConfirmations {
        /// Confirmations the reveal gate requires.
        required: u32,
        /// Confirmations the actor observed.
        actual: u32,
    },
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
    // The agreement is accepted at the time this role accepted it, as its own
    // provisioner recorded it in this owner-private config, not at the time
    // the activation happens to run: a Node stopped between the take and the
    // activation still activates after the offer expired, as every later step
    // resumes from the recorded acceptance. A recorded time at or after the
    // transcript's expiry is refused here as at the take. A config without
    // one (a template, or one written before the time was recorded) is
    // accepted at the current wall clock.
    let accepted_at = match config.accepted_at() {
        Some(accepted_at) => accepted_at,
        None => trusted_wall_clock()?,
    };
    let accepted = AcceptedZecAgreementV1::accept_wire_at(
        material.signed_agreement_wire(),
        accepted_at,
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
    if !activation_roles_match(
        accepted.agreement().revealing_claimant(),
        accepted.agreement().lez_claimant(),
        participant,
        material.claim_preimage().is_some(),
        config.is_local_zcash_funder(),
    ) {
        eprintln!(
            "actor activation refused: the preimage belongs to the Taker alone and the Zcash \
             candidates to the Zcash funder; this actor's material does not match the agreement"
        );
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
        config,
        ActorEffectCommandV1::Activate,
        ActorEffectOutcomeV1::Activated,
        &active,
    ))
}

/// Whether an actor's material fits its role in the agreement: the preimage is
/// the Taker's alone in both directions (it claims the Maker-funded leg first;
/// the Maker reads the preimage from that claim), and the Zcash funding
/// candidates belong to the Zcash funder (the LEZ claimant).
fn activation_roles_match(
    revealing_claimant: Participant,
    zcash_funder: Participant,
    participant: Participant,
    has_preimage: bool,
    has_funding_candidates: bool,
) -> bool {
    has_preimage == (participant == revealing_claimant)
        && has_funding_candidates == (participant == zcash_funder)
}

/// The wall clock in Unix milliseconds for the claim window (0 if the clock is
/// before the epoch, which keeps the window open only for a broken clock that
/// also breaks every other deadline here).
fn wall_clock_ms() -> lez_zec_swap_sdk::LezUnixMilliseconds {
    lez_zec_swap_sdk::LezUnixMilliseconds::new(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| {
                u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
            }),
    )
}

/// The next action this role reports: the SDK's, except once the claim window
/// has closed (the agreement's LEZ refund time minus the required margin;
/// FINDINGS 26) with both legs locked:
///
/// - the revealing claimant (the Taker, in both directions) whose claim has
///   not left is told to refund its own leg, never to claim: its Zcash in
///   `TakerSellsForeign`, its LEZ in `TakerSellsLez`. A claim already submitted
///   inside the window is still followed;
/// - the Maker of `TakerSellsLez` is told to refund its Zcash (the earlier
///   refund); before the window closes it waits while its drive observes the
///   Taker's Zcash claim.
///
/// Before the Maker's lock is observed the Taker waits while that window is
/// open: the Maker is still in time to lock, and a refund asked for then
/// would only be admitted to wait for its time while the swap could still
/// complete (issue #182). Once the window has closed no claim of the Maker's
/// leg can land any more (selling LEZ the Maker no longer funds either), so
/// the Taker is told to refund its own leg
/// ([`taker_refund_before_second_lock`]); its recovery sends the refund at
/// its own deadline, the LEZ refund time or the Zcash refund height.
///
/// The Maker of `TakerSellsLez` is also told to refund its Zcash from the
/// agreement's refund height `H_z` on, whatever the clock (ADR 0216: it
/// refunds between `H_z` and `T_lez - M`): its Node then alternates the refund
/// with observing the Taker's claim, so a claim that landed late is still
/// read, and a Taker that never claimed cannot hold the Zcash past `H_z` and
/// shrink the Maker's LEZ claim budget below `M`. Status makes no RPC, so the
/// height is the last Zcash tip a drive of this actor recorded
/// ([`zcash_tip_observed_marker`]); the Maker's drive records it at every
/// observation of the Taker's claim.
fn reported_next_action<Lez, Zcash, Store>(
    active: &lez_zec_swap_sdk::ActiveZecSwap<Lez, Zcash, Store>,
    config: &ActorConfig,
    now_ms: lez_zec_swap_sdk::LezUnixMilliseconds,
) -> ZecLifecycleAction {
    let claim_submitted = revealing_claim_submitted_marker(config.role_state_db())
        .is_some_and(|marker| marker.is_file());
    claimant_next_action(
        active.next_action(),
        active.status(),
        active.agreement().direction(),
        active.local_participant(),
        claim_submitted,
        active.claim_window_closed(now_ms),
        zcash_refund_height_observed(
            config.role_state_db(),
            active.agreement().zcash_refund_at_height(),
        ),
    )
}

/// [`reported_next_action`]'s rule on its own.
const fn claimant_next_action(
    sdk_next: ZecLifecycleAction,
    phase: Phase,
    direction: SwapDirection,
    local: Participant,
    claim_submitted: bool,
    window_closed: bool,
    zcash_refund_height_reached: bool,
) -> ZecLifecycleAction {
    if zcash_refund_height_reached
        && matches!(
            (direction, local, phase, sdk_next),
            (
                SwapDirection::TakerSellsLez,
                Participant::Maker,
                Phase::BothLegsLocked,
                ZecLifecycleAction::Wait
            )
        )
    {
        return ZecLifecycleAction::RefundZcash;
    }
    // TakerSellsLez: after the Maker's earlier Zcash refund the swap waits on the
    // Taker's later LEZ refund; the Maker's recovery observes it (the non-owner
    // of a refund step observes it) and ends Refunded.
    if matches!(
        (direction, local, phase, sdk_next),
        (
            SwapDirection::TakerSellsLez,
            Participant::Maker,
            Phase::MakerLegRefunded,
            ZecLifecycleAction::Wait
        )
    ) {
        return ZecLifecycleAction::RefundLez;
    }
    if !window_closed {
        return sdk_next;
    }
    if matches!(
        (local, sdk_next),
        (Participant::Taker, ZecLifecycleAction::Wait)
    ) && let Some(refund) = taker_refund_before_second_lock(direction, phase)
    {
        return refund;
    }
    if !matches!(phase, Phase::BothLegsLocked) {
        return sdk_next;
    }
    match (direction, local, sdk_next) {
        (SwapDirection::TakerSellsForeign, Participant::Taker, ZecLifecycleAction::ClaimLez)
            if !claim_submitted =>
        {
            ZecLifecycleAction::RefundZcash
        }
        (SwapDirection::TakerSellsLez, Participant::Taker, ZecLifecycleAction::ClaimZcash)
            if !claim_submitted =>
        {
            ZecLifecycleAction::RefundLez
        }
        (SwapDirection::TakerSellsLez, Participant::Maker, ZecLifecycleAction::Wait) => {
            ZecLifecycleAction::RefundZcash
        }
        _ => sdk_next,
    }
}

/// The Taker's refund path in a phase before both legs are locked, where the
/// SDK's recovery admits one (`next_refund_step`): its LEZ escrow selling LEZ,
/// its Zcash HTLC selling ZEC. In the reorg phases selling ZEC the recovery
/// first observes the Maker's LEZ refund, then refunds the Zcash; with the
/// Maker's lock confirming selling ZEC it admits none, and the Taker waits.
const fn taker_refund_before_second_lock(
    direction: SwapDirection,
    phase: Phase,
) -> Option<ZecLifecycleAction> {
    match (direction, phase) {
        (
            SwapDirection::TakerSellsLez,
            Phase::AwaitingTakerConfirmations
            | Phase::TakerLockConfirmed
            | Phase::AwaitingMakerConfirmations
            | Phase::TakerLockReorged
            | Phase::MakerLockReorged,
        ) => Some(ZecLifecycleAction::RefundLez),
        (
            SwapDirection::TakerSellsForeign,
            Phase::AwaitingTakerConfirmations
            | Phase::TakerLockConfirmed
            | Phase::TakerLockReorged
            | Phase::MakerLockReorged,
        ) => Some(ZecLifecycleAction::RefundZcash),
        _ => None,
    }
}

/// The Maker of `TakerSellsLez` funds Zcash only while the Taker's claim
/// window is open, by time and by height (ADR 0216): before the agreement's LEZ
/// refund time minus the required margin (`window_closed`), and while
/// `tip + R + C < H_z` (`height_closed`, [`zcash_funding_window_closed`]), and,
/// for a profile with a slowest pace, while `H_z` still comes before the window
/// closes at that pace (also `height_closed`,
/// [`zcash_refund_height_after_window`]). A funding after either cannot be
/// claimed by an honest Taker and only exposes the Maker to a refund race
/// (or, past the pace bound, leaves its refund to open only with the Taker's
/// LEZ refund). A funding already staged (its exact bytes may
/// be on the chain) is still driven to its observation.
const fn maker_funding_cut_off(
    direction: SwapDirection,
    funding_staged: bool,
    window_closed: bool,
    height_closed: bool,
) -> bool {
    matches!(direction, SwapDirection::TakerSellsLez)
        && !funding_staged
        && (window_closed || height_closed)
}

/// Whether a Zcash funding sent at `tip_height` leaves the Taker no reveal:
/// it needs `confirmations` blocks before the Taker's gate admits it, and the
/// Taker's claim then needs `tip + reveal_margin_blocks < refund_height` (the
/// SDK's `ZcashRevealWindowClosed` rule). Open only while
/// `tip + R + C < H_z`.
const fn zcash_funding_window_closed(
    tip_height: u32,
    reveal_margin_blocks: u32,
    confirmations: u32,
    refund_height: u32,
) -> bool {
    tip_height
        .saturating_add(reveal_margin_blocks)
        .saturating_add(confirmations)
        >= refund_height
}

/// Whether the Zcash refund height `H_z` may come after the Taker's claim
/// window closes (`T_lez - M`, `window_closes_at_ms`) at the profile's slowest
/// Zcash pace: `(H_z - tip) * slowest > T_lez - M - now`. A Maker that funds
/// then can be refunded only after the Taker's LEZ refund is open, so a late
/// Taker claim can take the Zcash while the Taker also refunds the LEZ.
///
/// Only a public profile has a slowest pace (`public-testnet-v3`: 75 s). On
/// the Regtest profiles the harness mines every block, so no wall-clock pace
/// bounds `H_z` and this check does not apply (`slowest_block_millis` is
/// `None`); their timing is the harness's, as their signed earlier-refund
/// bound is.
fn zcash_refund_height_after_window(
    tip_height: u32,
    refund_height: u32,
    slowest_block_millis: Option<u64>,
    now_ms: u64,
    window_closes_at_ms: u64,
) -> bool {
    let Some(slowest_block_millis) = slowest_block_millis else {
        return false;
    };
    let blocks = u64::from(refund_height.saturating_sub(tip_height));
    blocks.saturating_mul(slowest_block_millis) > window_closes_at_ms.saturating_sub(now_ms)
}

/// The marker holding the last Zcash tip height an effect command of this
/// actor observed (decimal text), beside the actor's state
/// (`<swap>/actor/<role>/zcash-tip-observed`). Status stays offline: it reads
/// this marker instead of asking Zebra, so a Maker's refund is offered from
/// the refund height on as soon as a drive has seen the tip there.
#[must_use]
pub fn zcash_tip_observed_marker(role_state_db: &Path) -> Option<PathBuf> {
    Some(role_state_db.parent()?.parent()?.join("zcash-tip-observed"))
}

/// Records `tip_height` as the last observed Zcash tip.
fn record_observed_zcash_tip(role_state_db: &Path, tip_height: u32) {
    if let Some(marker) = zcash_tip_observed_marker(role_state_db) {
        let _ = fs::write(marker, format!("{tip_height}\n"));
    }
}

/// The last observed Zcash tip height, when recorded and well formed.
fn observed_zcash_tip(role_state_db: &Path) -> Option<u32> {
    fs::read_to_string(zcash_tip_observed_marker(role_state_db)?)
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Whether the last observed Zcash tip has reached `refund_height` (the
/// Maker's Zcash refund is valid from a tip at `H_z`, as the SDK's refund
/// deadline reads it). No observation yet reads as not reached.
fn zcash_refund_height_observed(role_state_db: &Path, refund_height: u32) -> bool {
    observed_zcash_tip(role_state_db).is_some_and(|tip| tip >= refund_height)
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
    let planner = ExactOutpointZcashFundingPlanner::new(rpc.clone(), identity, participant, signer);
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
                let driven = match active.drive_first_lock().await {
                    Ok(driven) => driven,
                    Err(error) if is_escrow_already_initialized(&error) => {
                        let open = store
                            .load_first_lock_intent(active.agreement().coordinator().id())
                            .await
                            .map_err(unavailable_drive)?
                            .ok_or(ActorCommandError::DriveUnavailable)?;
                        break escrow_initialization_refused(config, open.plan())?;
                    }
                    Err(error) => return Err(unavailable_drive(error)),
                };
                match driven {
                    FirstLockDriveOutcome::Submitted(step) => {
                        if let Some(open) = store
                            .load_first_lock_intent(active.agreement().coordinator().id())
                            .await
                            .map_err(unavailable_drive)?
                        {
                            count_lez_submission(config, open.plan(), step);
                        }
                        break ActorEffectOutcomeV1::Submitted {
                            operation: first_lock_operation(step),
                            replans,
                        };
                    }
                    FirstLockDriveOutcome::AwaitingStableObservation(step) => {
                        clear_lez_first_lock_attempts(config.role_state_db());
                        break ActorEffectOutcomeV1::AwaitingObservation {
                            operation: first_lock_operation(step),
                        };
                    }
                    FirstLockDriveOutcome::ReadyForFundingProjection(evidence) => {
                        clear_lez_first_lock_attempts(config.role_state_db());
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
        (Participant::Maker, Phase::TakerLockConfirmed) => 'maker_lock: {
            let staged = store
                .load_maker_lock_intent(active.agreement().coordinator().id())
                .await
                .map_err(unavailable_drive)?;
            let now = wall_clock_ms();
            let window_closed = active.claim_window_closed(now);
            // The height half of the cutoff reads the Zebra tip only where it
            // can decide: a fresh TakerSellsLez funding inside the time window.
            let height_closed = if matches!(
                active.agreement().direction(),
                SwapDirection::TakerSellsLez
            ) && staged.is_none()
                && !window_closed
            {
                let tip_height = u32::from(
                    rpc.chain_info()
                        .await
                        .map_err(unavailable_drive)?
                        .tip_height(),
                );
                let profile = lez_zec_swap_sdk::ZecRefundProfile::for_id(
                    active.agreement().binding().profile_id(),
                );
                let refund_height = active.agreement().zcash_refund_at_height();
                let closed = zcash_funding_window_closed(
                    tip_height,
                    profile.zcash_reveal_margin_blocks(),
                    profile.zcash_confirmations(),
                    refund_height,
                );
                if closed {
                    eprintln!(
                        "actor drive: the Zcash funding is not sent: tip {tip_height} + {} reveal-margin blocks + {} confirmations reaches the refund height {refund_height}; the Taker's settlement is its LEZ refund",
                        profile.zcash_reveal_margin_blocks(),
                        profile.zcash_confirmations()
                    );
                }
                // At the profile's slowest pace H_z must still come inside the
                // window, or the Maker's refund opens only with the Taker's.
                let late = zcash_refund_height_after_window(
                    tip_height,
                    refund_height,
                    profile.zcash_slowest_block_millis(),
                    now.value(),
                    active.claim_window_closes_at_ms(),
                );
                if late {
                    eprintln!(
                        "actor drive: the Zcash funding is not sent: from tip {tip_height} the refund height {refund_height} may come after the Taker's claim window closes at {} ms at the profile's slowest pace of {} ms a block; the Taker's settlement is its LEZ refund",
                        active.claim_window_closes_at_ms(),
                        profile.zcash_slowest_block_millis().unwrap_or_default()
                    );
                }
                closed || late
            } else {
                false
            };
            if maker_funding_cut_off(
                active.agreement().direction(),
                staged.is_some(),
                window_closed,
                height_closed,
            ) {
                if window_closed {
                    eprintln!(
                        "actor drive: the Zcash funding is not sent: the Taker's claim window closed at {} ms; the Taker's settlement is its LEZ refund",
                        active.claim_window_closes_at_ms()
                    );
                }
                break 'maker_lock ActorEffectOutcomeV1::AwaitingObservation {
                    operation: ActorOperationV1::TakerFirstLock,
                };
            }
            let plan = match staged {
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
                let driven = match active.drive_maker_lock(plan.clone()).await {
                    Ok(driven) => driven,
                    Err(error) if is_escrow_already_initialized(&error) => {
                        break escrow_initialization_refused(config, &plan)?;
                    }
                    Err(error) => return Err(unavailable_drive(error)),
                };
                if !matches!(
                    driven,
                    MakerLockDriveOutcome::Lock(
                        FirstLockDriveOutcome::Submitted(_)
                            | FirstLockDriveOutcome::Superseded { .. }
                    )
                ) {
                    clear_lez_first_lock_attempts(config.role_state_db());
                }
                match driven {
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
                        count_lez_submission(config, &plan, step);
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
            if (participant, active.status(), active.agreement().direction())
                == (
                    Participant::Maker,
                    Phase::BothLegsLocked,
                    SwapDirection::TakerSellsLez,
                )
            {
                // The Maker's status offers its Zcash refund from H_z on and
                // makes no RPC; it reads the tip this drive records. A failed
                // read leaves the last record and the observation goes on.
                match rpc.chain_info().await {
                    Ok(info) => {
                        record_observed_zcash_tip(config.role_state_db(), info.tip_height().into());
                    }
                    Err(error) => eprintln!("actor drive: the Zcash tip was not read: {error:?}"),
                }
            }
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

    let output =
        ActorEffectOutputV1::from_active(config, ActorEffectCommandV1::Drive, outcome, &active);
    Ok(match claim_contention_wait(&active).await {
        Some(wait) => output.waiting_for(wait),
        None => output,
    })
}

/// Runs only the agreement-derived claim state machine.
///
/// Unlike `drive`, this boundary cannot fund or lock either leg. A premature
/// operator request therefore fails closed before an effect RPC rather than
/// silently advancing a different lifecycle action.
#[allow(
    clippy::too_many_lines,
    reason = "port construction, the lost-claim settlement and the claim drive read as one command"
)]
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
    if let Some(reason) = followup_lez_claim_cannot_land(&active, &lez).await? {
        eprintln!(
            "actor claim refused: the follow-up LEZ claim can no longer land: {reason}; nothing is submitted again (lez_claim_cannot_land)"
        );
        // Nothing of this swap can take the LEZ account's nonce any more.
        clear_revealing_claim_in_flight(config);
        // The Taker's LEZ refund is final: if the Taker's Zcash claim was never
        // mined the HTLC is unspent, and the Maker recovers its Zcash at the
        // refund height (review 3, item 1). Only an HTLC spent by the Taker's
        // claim at the profile's Zcash depth ends the swap by name; a claim in
        // the mempool only can still be evicted, so it is a wait (review 4, M1).
        return match active
            .drive_refund_after_lost_followup(wall_clock_ms())
            .await
        {
            // Reported as every Zcash refund is (`refunded`, phase `refunded`;
            // issue #178): `completed` there was refused by the Node.
            Ok(lez_zec_swap_sdk::RefundDriveOutcome::Refunded { .. }) => {
                Ok(ActorEffectOutputV1::from_active(
                    config,
                    ActorEffectCommandV1::Claim,
                    lost_claim_refunded_outcome(),
                    &active,
                ))
            }
            Ok(lez_zec_swap_sdk::RefundDriveOutcome::AwaitingFunding {
                reason: RefundFundingWaitReasonV1::Spent,
                ..
            }) => Err(ActorCommandError::LezClaimCannotLand),
            Ok(outcome) => {
                eprintln!(
                    "actor claim: the Maker's Zcash refund after the lost LEZ claim: {outcome:?}"
                );
                Err(ActorCommandError::ClaimUnavailable)
            }
            Err(error) => {
                eprintln!(
                    "actor claim: the Maker's Zcash refund after the lost LEZ claim: {error:?}"
                );
                Err(ActorCommandError::ClaimUnavailable)
            }
        };
    }
    let lez_refund_at_ms = active.agreement().lez_refund_at_ms();
    // Past the LEZ refund time by the host's clock, the LEZ chain's own clock
    // decides whether the Maker's follow-up claim can still be included
    // (review 4, M2).
    if is_maker_followup_lez_claim(&active) && wall_clock_ms().value() >= lez_refund_at_ms {
        let finalized_ms = match lez.observe_finalized_lez_clock().await {
            Ok((_, timestamp_ms)) => Some(timestamp_ms),
            Err(error) => {
                eprintln!("actor claim: the finalized LEZ clock could not be read: {error:?}");
                None
            }
        };
        if followup_lez_claim_drive(lez_refund_at_ms, finalized_ms)
            == FollowupLezClaimDrive::ObserveOnly
        {
            // No block can include it and it takes no nonce: it holds no other claim.
            clear_revealing_claim_in_flight(config);
            let outcome = claim_attempt(
                active.observe_followup_lez_claim_past_window().await,
                ActorCommandError::ClaimUnavailable,
            )?;
            eprintln!(
                "actor claim: the Maker's LEZ claim is past its window by LEZ time ({finalized_ms:?} >= {lez_refund_at_ms}); nothing is submitted, the swap waits for the Taker's LEZ refund ({outcome:?})"
            );
            let completed = matches!(outcome, ClaimDriveOutcome::Completed { .. })
                || active.status() == Phase::Completed;
            if !completed {
                // No claim of the escrow can land once it is still open at a
                // finalized LEZ time past the refund time: the Maker refunds its
                // Zcash whether or not the Taker ever refunds its LEZ (review 5,
                // item 2).
                match lez
                    .observe_finalized_lez_escrow_open(active.agreement())
                    .await
                {
                    Ok((finalized_ms, true)) => {
                        if let Ok(open) = lez_zec_swap_sdk::LezEscrowOpenPastRefundTimeV1::new(
                            active.agreement(),
                            finalized_ms,
                        ) {
                            let refund = active
                                .drive_refund_with_open_escrow(wall_clock_ms(), open)
                                .await;
                            eprintln!(
                                "actor claim: the escrow is open at finalized LEZ time {finalized_ms} past the refund time; the Maker's Zcash refund: {refund:?}"
                            );
                            return lost_claim_refund_output(config, &active, &refund);
                        }
                    }
                    Ok((_, false)) => {}
                    Err(error) => {
                        eprintln!(
                            "actor claim: the finalized LEZ escrow could not be read: {error:?}"
                        );
                    }
                }
            }
            let output = ActorEffectOutputV1::from_active(
                config,
                ActorEffectCommandV1::Claim,
                if completed {
                    ActorEffectOutcomeV1::Completed
                } else {
                    claim_outcome(outcome)
                },
                &active,
            );
            return Ok(if completed {
                output
            } else {
                output.waiting_for(lost_lez_claim_wait(lez_refund_at_ms, finalized_ms))
            });
        }
    }
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
    let past_window = is_maker_followup_lez_claim(&active)
        && matches!(outcome, ActorEffectOutcomeV1::AwaitingObservation { .. })
        && wall_clock_ms().value() >= lez_refund_at_ms;
    let output =
        ActorEffectOutputV1::from_active(config, ActorEffectCommandV1::Claim, outcome, &active);
    Ok(if past_window {
        output.waiting_for(lost_lez_claim_wait(lez_refund_at_ms, None))
    } else if let Some(wait) = claim_contention_wait(&active).await {
        output.waiting_for(wait)
    } else {
        output
    })
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

/// Whether a first-lock drive failed because the bridge did not send the
/// escrow initialization: its escrow account already exists, initialized by a
/// transaction that is not this swap's (`ErrorCode::EscrowAlreadyInitialized`),
/// reached through the SDK, port, adapter and transport error layers.
fn is_escrow_already_initialized(error: &lez_zec_swap_sdk::ZecSdkError) -> bool {
    let lez_zec_swap_sdk::ZecSdkError::LezFirstLock(source) = error else {
        return false;
    };
    let Some(ContextOwningLezPortError::Adapter(adapter)) =
        source.downcast_ref::<ContextOwningLezPortError>()
    else {
        return false;
    };
    let Some(PrepareNativeFirstLockError::Transport(BridgeClientError::Remote(remote))) =
        adapter.downcast_ref::<PrepareNativeFirstLockError<BridgeClientError>>()
    else {
        return false;
    };
    remote.code() == ErrorCode::EscrowAlreadyInitialized
}

/// The actor's spelling of a LEZ first-lock step and its transaction id in
/// `plan`, hex; `None` for the Zcash step.
fn lez_step_identity(
    plan: &FirstLockPlanV1,
    step: FirstLockStepV1,
) -> Option<(&'static str, String)> {
    let FirstLockPlanV1::Lez { initialize, fund } = plan else {
        return None;
    };
    match step {
        FirstLockStepV1::LezInitialize => Some((
            "lez_initialize",
            hex::encode(initialize.expected_submission_id()),
        )),
        FirstLockStepV1::LezFund => Some(("lez_fund", hex::encode(fund.expected_submission_id()))),
        FirstLockStepV1::ZcashFund => None,
    }
}

fn wall_clock_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// Counts one submission of a LEZ first-lock step that was not seen on the
/// chain before it; the Node names the step in the swap's status once it was
/// submitted many times over a long time ([`crate::first_lock_attempts`]).
/// Nothing is decided here: the next drive submits again.
fn count_lez_submission(config: &ActorConfig, plan: &FirstLockPlanV1, step: FirstLockStepV1) {
    let Some((name, transaction_id)) = lez_step_identity(plan, step) else {
        return;
    };
    let named_before = lez_first_lock_attempts(config.role_state_db())
        .is_some_and(|record| record.unincluded_named());
    let record = record_lez_first_lock_attempt(
        config.role_state_db(),
        name,
        &transaction_id,
        LezFirstLockAttemptOutcomeV1::Submitted,
        wall_clock_unix_ms(),
    );
    if !named_before && let Some(detail) = record.status_detail() {
        eprintln!("actor drive: {detail}");
    }
}

/// The bridge did not send the escrow initialization because its escrow
/// account already exists without it. Below the bound (attempts and span,
/// [`crate::first_lock_attempts`]) the drive reports the initialization as
/// awaiting observation and the next drive looks again; at the bound the swap
/// stops by name ([`ActorCommandError::LezEscrowAlreadyInitialized`]).
fn escrow_initialization_refused(
    config: &ActorConfig,
    plan: &FirstLockPlanV1,
) -> Result<ActorEffectOutcomeV1, ActorCommandError> {
    let (name, transaction_id) = lez_step_identity(plan, FirstLockStepV1::LezInitialize)
        .ok_or(ActorCommandError::DriveUnavailable)?;
    let record = record_lez_first_lock_attempt(
        config.role_state_db(),
        name,
        &transaction_id,
        LezFirstLockAttemptOutcomeV1::EscrowAlreadyInitialized,
        wall_clock_unix_ms(),
    );
    if record.stops() {
        eprintln!(
            "actor drive refused: {}",
            record.status_detail().unwrap_or_default()
        );
        return Err(ActorCommandError::LezEscrowAlreadyInitialized);
    }
    eprintln!(
        "actor drive: LEZ escrow initialization {transaction_id} not sent: its escrow account already exists without any of this swap's initializations (attempt {}; the swap stops after {ESCROW_REFUSAL_STOP_ATTEMPTS} over at least {ESCROW_REFUSAL_STOP_SECONDS} s)",
        record.attempts
    );
    Ok(ActorEffectOutcomeV1::AwaitingObservation {
        operation: ActorOperationV1::LezInitialize,
    })
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

/// The marker the Taker's revealing claim (LEZ in `TakerSellsForeign`, Zcash
/// in `TakerSellsLez`) leaves the moment it is submitted, kept for the life of
/// the swap: a refund in the claim window is refused once it exists (FINDINGS
/// 24, the secret may be public).
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

/// The marker of an owned LEZ claim submitted and not yet observed final (the
/// Taker's revealing LEZ claim in `TakerSellsForeign`, the Maker's follow-up
/// LEZ claim in `TakerSellsLez`; both are signed at the claimant's LEZ account
/// nonce): while another swap of the same Node holds it, that Node keeps its
/// own LEZ claim back (one per LEZ account in flight; FINDINGS 24). Removed
/// when the claim is projected or can no longer land, never on a timer. It
/// records the swap's LEZ refund time
/// ([`revealing_claim_in_flight_refund_at_ms`]): past it the claim cannot be
/// included any more and holds no other claim.
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

/// The text of a submitted claim's markers: the claim's name, then the swap's
/// LEZ refund time in ms on a line `lez_refund_at_ms=<ms>`.
fn claim_marker_text(step: ClaimStepV1, lez_refund_at_ms: u64) -> String {
    format!(
        "{} submitted\nlez_refund_at_ms={lez_refund_at_ms}\n",
        claim_name(step)
    )
}

/// The LEZ refund time an in-flight marker records, when the marker exists
/// and records one (a marker written before the time was recorded has none).
#[must_use]
pub fn revealing_claim_in_flight_refund_at_ms(role_state_db: &Path) -> Option<u64> {
    fs::read_to_string(revealing_claim_in_flight_marker(role_state_db)?)
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("lez_refund_at_ms="))?
        .trim()
        .parse()
        .ok()
}

/// When the claim bytes an in-flight marker stands for were first submitted
/// (Unix milliseconds; a line `submitted_at_ms=<ms>`), when the marker exists
/// and records it. A re-planned claim is new bytes at a new nonce and records
/// its own time; sending the same bytes again keeps the time. A Maker Node
/// holds its other LEZ claims behind a submitted one for a bounded time from
/// it, not until the claim is final (issue #176).
#[must_use]
pub fn revealing_claim_in_flight_submitted_at_ms(role_state_db: &Path) -> Option<u64> {
    fs::read_to_string(revealing_claim_in_flight_marker(role_state_db)?)
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("submitted_at_ms="))?
        .trim()
        .parse()
        .ok()
}

/// The in-flight marker's text: [`claim_marker_text`] and the time the
/// current claim bytes were first submitted.
fn in_flight_marker_text(step: ClaimStepV1, lez_refund_at_ms: u64, submitted_at_ms: u64) -> String {
    format!(
        "{}submitted_at_ms={submitted_at_ms}\n",
        claim_marker_text(step, lez_refund_at_ms)
    )
}

/// The time an in-flight marker written now records: the one it already
/// records when the same claim bytes are sent again, `now_ms` for a first
/// submission, a re-planned claim (`replanned`), or a marker without one.
fn in_flight_submitted_at_ms(recorded: Option<u64>, replanned: bool, now_ms: u64) -> u64 {
    match recorded {
        Some(recorded) if !replanned => recorded,
        _ => now_ms,
    }
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

/// The marker a Maker Node writes beside a swap whose LEZ claim waits at the
/// one-claim gate, holding that swap's LEZ refund time (ms): when the claim in
/// flight is final the gate admits the waiting claim with the earliest refund
/// time, not the one scheduled first. Removed by the Node when the swap no
/// longer asks for a LEZ claim, and by the actor once the claim is submitted
/// (after its in-flight marker is written) or the swap leaves the claim's
/// phase. (`<swap>/actor/<role>/lez-claim-waiting`.)
#[must_use]
pub fn lez_claim_waiting_marker(role_state_db: &Path) -> Option<PathBuf> {
    Some(role_state_db.parent()?.parent()?.join("lez-claim-waiting"))
}

/// Records the swap's LEZ refund time in its waiting marker, or removes the
/// marker (`None`).
pub fn record_lez_claim_waiting(role_state_db: &Path, lez_refund_at_ms: Option<u64>) {
    let Some(marker) = lez_claim_waiting_marker(role_state_db) else {
        return;
    };
    match lez_refund_at_ms {
        Some(at) => {
            let _ = fs::write(marker, format!("{at}\n"));
        }
        None => {
            let _ = fs::remove_file(marker);
        }
    }
}

/// The LEZ refund time a waiting marker records, when present and well formed.
#[must_use]
pub fn lez_claim_waiting_refund_at_ms(role_state_db: &Path) -> Option<u64> {
    fs::read_to_string(lez_claim_waiting_marker(role_state_db)?)
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// The marker a Maker Node writes when its one-LEZ-claim gate admits this
/// swap's LEZ claim, before the claim is signed: its LEZ refund time, the
/// admission time and how long the admission holds (ms). Another worker's gate
/// treats it as a claim in flight until the claim's own in-flight marker
/// replaces it, the swap's refund time passes, or the hold ends (the Node stops
/// the attempt before that; review 4, L2). Removed when the attempt ends, at
/// this swap's next gate evaluation, and when the supervisor starts.
/// (`<swap>/actor/<role>/lez-claim-admitted`.)
#[must_use]
pub fn lez_claim_admitted_marker(role_state_db: &Path) -> Option<PathBuf> {
    Some(role_state_db.parent()?.parent()?.join("lez-claim-admitted"))
}

/// One gate admission: the swap's LEZ refund time, when it was admitted, and
/// how long it holds the others (ms; `None` in a marker written before the
/// hold was recorded: the reader then applies its own attempt bound).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LezClaimAdmission {
    /// The admitted swap's LEZ refund time, Unix milliseconds.
    pub lez_refund_at_ms: u64,
    /// The admission time, Unix milliseconds.
    pub admitted_at_ms: u64,
    /// How long the admission holds, milliseconds.
    pub hold_ms: Option<u64>,
}

/// Records this swap's LEZ claim as admitted, or removes the record (`None`).
pub fn record_lez_claim_admitted(role_state_db: &Path, admission: Option<LezClaimAdmission>) {
    let Some(marker) = lez_claim_admitted_marker(role_state_db) else {
        return;
    };
    match admission {
        Some(admission) => {
            let hold = admission
                .hold_ms
                .map_or_else(String::new, |hold_ms| format!("hold_ms={hold_ms}\n"));
            let _ = fs::write(
                marker,
                format!(
                    "lez_refund_at_ms={}\nadmitted_at_ms={}\n{hold}",
                    admission.lez_refund_at_ms, admission.admitted_at_ms
                ),
            );
        }
        None => {
            let _ = fs::remove_file(marker);
        }
    }
}

/// The admission a marker records, when present and well formed.
#[must_use]
pub fn lez_claim_admission(role_state_db: &Path) -> Option<LezClaimAdmission> {
    let text = fs::read_to_string(lez_claim_admitted_marker(role_state_db)?).ok()?;
    let field = |key: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(key))
            .and_then(|value| value.trim().parse::<u64>().ok())
    };
    Some(LezClaimAdmission {
        lez_refund_at_ms: field("lez_refund_at_ms=")?,
        admitted_at_ms: field("admitted_at_ms=")?,
        hold_ms: field("hold_ms="),
    })
}

fn valid_swap_id_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':'))
}

/// One claim drive's result as the actor reports it: the claim-window refusal
/// by name (FINDINGS 26), every other lifecycle error as `unavailable` with
/// its reason on stderr (the exit code stays a secret-free class).
fn claim_attempt(
    result: Result<ClaimDriveOutcome, lez_zec_swap_sdk::ZecSdkError>,
    unavailable: ActorCommandError,
) -> Result<ClaimDriveOutcome, ActorCommandError> {
    match result {
        Ok(outcome) => Ok(outcome),
        Err(lez_zec_swap_sdk::ZecSdkError::ClaimWindowClosed {
            closes_at_ms,
            now_ms,
        }) => {
            eprintln!(
                "actor claim refused: the revealing claim window closed at {closes_at_ms} ms (now {now_ms} ms); nothing was sent, the settlement is the refund"
            );
            Err(ActorCommandError::ClaimWindowClosed)
        }
        Err(lez_zec_swap_sdk::ZecSdkError::ZcashRevealWindowClosed {
            tip_height,
            refund_height,
            margin_blocks,
        }) => {
            eprintln!(
                "actor claim refused: the revealing Zcash claim window closed: tip {tip_height} + {margin_blocks} blocks reaches the refund height {refund_height}; nothing was sent, the settlement is the LEZ refund"
            );
            Err(ActorCommandError::ClaimWindowClosed)
        }
        Err(lez_zec_swap_sdk::ZecSdkError::InvalidZcashFundingObservation(
            lez_zec_swap_sdk::ZcashFundingObservationError::InsufficientConfirmations {
                required,
                actual,
            },
        )) => {
            eprintln!(
                "actor claim queued: the Zcash funding has {actual} of the {required} confirmations the reveal needs; nothing was sent"
            );
            Err(ActorCommandError::ClaimAwaitingZcashConfirmations { required, actual })
        }
        Err(error) => {
            eprintln!("actor claim unavailable: {error:?}");
            Err(unavailable)
        }
    }
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
    Zcash: lez_zec_swap_sdk::ZcashClaimPort + lez_zec_swap_sdk::ZcashRefundPort,
{
    let mut replans = Vec::new();
    loop {
        let outcome = claim_attempt(active.drive_claim(wall_clock_ms()).await, unavailable)?;
        match outcome {
            ClaimDriveOutcome::Superseded {
                step,
                planned_nonce,
                live_nonce,
            } => {
                // One re-plan per attempt: a claim superseded again in the same
                // attempt waits for the next one (the Node's requeue delay), so
                // re-plans are bounded in rate and never exhausted while the
                // claim can land (issue #172). The claim's in-flight marker
                // stays: it is still this swap's claim at the account's nonce.
                if replans.len() >= REVEALING_CLAIM_REPLANS_PER_ATTEMPT {
                    return Ok(claim_outcome(ClaimDriveOutcome::AwaitingStableObservation(
                        step,
                    )));
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
                        // The protocol bound, out of reach inside any LEZ
                        // refund window; the swap is never failed for it.
                        eprintln!(
                            "actor claim: the LEZ claim reached the protocol's re-plan bound; it is not re-planned again ({error:?}); needs attention (lez_claim_nonce_contention)"
                        );
                        clear_revealing_claim_in_flight(config);
                        return Ok(claim_outcome(ClaimDriveOutcome::AwaitingStableObservation(
                            step,
                        )));
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
                    "actor claim: {} superseded (planned nonce {planned_nonce}, live {live_nonce}); re-plan {} at nonce {}: {} -> {}; {} s left before the LEZ refund time",
                    claim_name(step),
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
                replans.push(ActorReplanV1::from_claim(&supersession, step));
            }
            ClaimDriveOutcome::Submitted(step) => {
                mark_claim_submitted(
                    config,
                    step,
                    active.agreement().lez_refund_at_ms(),
                    !replans.is_empty(),
                );
                return Ok(ActorEffectOutcomeV1::Submitted {
                    operation: claim_operation(step),
                    replans,
                });
            }
            other => {
                if active.status() != owned_lez_claim_phase(active.agreement().direction()) {
                    clear_revealing_claim_in_flight(config);
                }
                return Ok(claim_outcome(other));
            }
        }
    }
}

/// Why a Maker's follow-up LEZ claim (`TakerSellsLez`) can no longer land.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LezClaimCannotLand {
    /// The Taker's LEZ refund is on the chain: the escrow is empty.
    EscrowRefunded,
}

impl std::fmt::Display for LezClaimCannotLand {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::EscrowRefunded => "the Taker's LEZ refund is on the chain",
        })
    }
}

/// Whether the Maker's follow-up LEZ claim can no longer land, from what was
/// observed at `now_ms` (the host's clock, used only to skip the observation
/// before the refund time): once the Taker's LEZ refund is confirmed. Nothing
/// is decided from the host's clock: the escrow's claim window ends at
/// `refund_at` by the sequencer's block timestamp, which can lag, so a claim
/// the host thinks late may still be included (review 3, item 2). A claim
/// that cannot be included any more and an escrow nobody refunds leave the
/// swap at its claim, holding no other claim (the gate's time rule) and
/// counted by no capacity (its refund time passed).
const fn lez_claim_cannot_land(
    now_ms: u64,
    lez_refund_at_ms: u64,
    taker_refund_confirmed: bool,
) -> Option<LezClaimCannotLand> {
    if now_ms >= lez_refund_at_ms && taker_refund_confirmed {
        Some(LezClaimCannotLand::EscrowRefunded)
    } else {
        None
    }
}

/// What the Maker of `TakerSellsLez` at claim evidence does with its
/// follow-up LEZ claim this attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FollowupLezClaimDrive {
    /// Drive it: prepare, observe, submit, re-plan.
    Drive,
    /// Its window is closed by LEZ time: observe it only (it may have landed
    /// before), submit nothing, and wait for the Taker's LEZ refund.
    ObserveOnly,
}

/// The follow-up LEZ claim's drive from the finalized LEZ clock
/// (`finalized_ms`, its last finalized block's timestamp, when read).
/// The escrow admits a claim only in a block whose timestamp is before
/// `refund_at`, and block timestamps do not go back: once the finalized one is
/// at or past it, no block can include the claim (review 4, M2). Without a
/// reading the claim is driven: the window may still be open.
const fn followup_lez_claim_drive(
    lez_refund_at_ms: u64,
    finalized_ms: Option<u64>,
) -> FollowupLezClaimDrive {
    match finalized_ms {
        Some(finalized_ms) if finalized_ms >= lez_refund_at_ms => {
            FollowupLezClaimDrive::ObserveOnly
        }
        _ => FollowupLezClaimDrive::Drive,
    }
}

/// Re-plans of the owned LEZ claim in one attempt (issue #172).
const REVEALING_CLAIM_REPLANS_PER_ATTEMPT: usize = 1;

/// The wait an owned LEZ claim reports once it was re-planned more than
/// [`REVEALING_CLAIM_REPLAN_ATTENTION`] times (`supersessions`): the account's
/// nonce keeps being taken, by the Maker's own other LEZ transactions or by
/// the operator's. It goes on being re-planned until the LEZ refund time; the
/// operator is told by name (issue #172).
const fn lez_claim_contention_wait(
    supersessions: usize,
    lez_refund_at_ms: u64,
) -> Option<ActorWaitV1> {
    if supersessions > REVEALING_CLAIM_REPLAN_ATTENTION as usize {
        Some(ActorWaitV1 {
            kind: "lez_claim_nonce_contention",
            chain: "lez",
            until_unix_seconds: Some(lez_refund_at_ms / 1000),
            chain_unix_seconds: None,
        })
    } else {
        None
    }
}

/// The contention wait of `active`'s owned LEZ claim, read from its recorded
/// re-plans (none when they cannot be read).
async fn claim_contention_wait<Lez, Zcash, Store>(
    active: &lez_zec_swap_sdk::ActiveZecSwap<Lez, Zcash, Store>,
) -> Option<ActorWaitV1>
where
    Lez: lez_zec_swap_sdk::LezClaimPort,
    Zcash: lez_zec_swap_sdk::ZcashClaimPort + lez_zec_swap_sdk::ZcashRefundPort,
    Store: lez_zec_swap_sdk::ClaimRecoveryStore + lez_zec_swap_sdk::RefundRecoveryStore,
{
    let count = active.claim_supersessions().await.ok()?.len();
    let wait = lez_claim_contention_wait(count, active.agreement().lez_refund_at_ms());
    if wait.is_some() {
        eprintln!(
            "actor claim: the LEZ claim was re-planned {count} times: the account's nonce keeps being taken; it is re-planned until the LEZ refund time; needs attention (lez_claim_nonce_contention)"
        );
    }
    wait
}

/// The wait a Maker's follow-up LEZ claim past its window reports.
const fn lost_lez_claim_wait(lez_refund_at_ms: u64, finalized_ms: Option<u64>) -> ActorWaitV1 {
    ActorWaitV1 {
        kind: "taker_lez_refund_after_lost_claim",
        chain: "lez",
        until_unix_seconds: Some(lez_refund_at_ms / 1000),
        chain_unix_seconds: match finalized_ms {
            Some(finalized_ms) => Some(finalized_ms / 1000),
            None => None,
        },
    }
}

/// What the Maker's Zcash refund after a lost LEZ claim did, as the claim
/// command reports it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LostClaimRefundStep {
    /// The Zcash refund is recorded: the swap is refunded.
    Refunded,
    /// The HTLC is spent by the Taker's claim at depth: nothing is left.
    CannotLand,
    /// The refund went out (or was rebroadcast).
    Submitted,
    /// The refund waits (deadline, depth, an unstable view, a mempool spend).
    Waiting,
}

const fn lost_claim_refund_step(outcome: &RefundDriveOutcome) -> LostClaimRefundStep {
    match outcome {
        RefundDriveOutcome::Refunded { .. } => LostClaimRefundStep::Refunded,
        RefundDriveOutcome::AwaitingFunding {
            step: RefundStepV1::Zcash,
            reason: RefundFundingWaitReasonV1::Spent,
        } => LostClaimRefundStep::CannotLand,
        RefundDriveOutcome::Submitted(_) => LostClaimRefundStep::Submitted,
        _ => LostClaimRefundStep::Waiting,
    }
}

/// The claim command's outcome once the Maker's Zcash refund after a lost LEZ
/// claim is recorded (phase `refunded`): `refunded`, as every refund path
/// reports it, never `completed` (issue #178).
const fn lost_claim_refunded_outcome() -> ActorEffectOutcomeV1 {
    ActorEffectOutcomeV1::Refunded
}

/// The claim command's output for the Maker's Zcash refund after its LEZ claim
/// was lost: refunded once recorded, `lez_claim_cannot_land` once the HTLC
/// is the Taker's, otherwise a named wait (an error is a retry).
fn lost_claim_refund_output<Lez, Zcash, Store>(
    config: &ActorConfig,
    active: &lez_zec_swap_sdk::ActiveZecSwap<Lez, Zcash, Store>,
    refund: &Result<RefundDriveOutcome, lez_zec_swap_sdk::ZecSdkError>,
) -> Result<ActorEffectOutputV1, ActorCommandError> {
    let Ok(refund) = refund else {
        return Err(ActorCommandError::ClaimUnavailable);
    };
    let outcome = match lost_claim_refund_step(refund) {
        LostClaimRefundStep::Refunded => lost_claim_refunded_outcome(),
        LostClaimRefundStep::CannotLand => return Err(ActorCommandError::LezClaimCannotLand),
        LostClaimRefundStep::Submitted => ActorEffectOutcomeV1::Submitted {
            operation: ActorOperationV1::ZcashRefund,
            replans: Vec::new(),
        },
        LostClaimRefundStep::Waiting => ActorEffectOutcomeV1::AwaitingObservation {
            operation: ActorOperationV1::ZcashRefund,
        },
    };
    let refunded = outcome == lost_claim_refunded_outcome();
    let output =
        ActorEffectOutputV1::from_active(config, ActorEffectCommandV1::Claim, outcome, active);
    Ok(if refunded {
        output
    } else {
        output.waiting_for(ActorWaitV1 {
            kind: "zcash_refund_after_lost_lez_claim",
            chain: "zcash",
            until_unix_seconds: None,
            chain_unix_seconds: None,
        })
    })
}

/// Whether `active` is the Maker of `TakerSellsLez` at claim evidence, the
/// one actor whose LEZ claim is the follow-up claim.
fn is_maker_followup_lez_claim<Lez, Zcash, Store>(
    active: &lez_zec_swap_sdk::ActiveZecSwap<Lez, Zcash, Store>,
) -> bool {
    active.agreement().direction() == SwapDirection::TakerSellsLez
        && active.local_participant() == Participant::Maker
        && active.status() == Phase::ClaimEvidenceAvailable
}

/// The Maker of `TakerSellsLez` with claim evidence recorded has no other
/// exit than its LEZ claim, and the bridge does not refuse one that can never
/// land: the sidecar prepares and submits it (the sequencer admits it to its
/// mempool without running the escrow program or checking its validity
/// window), the block builder drops it with no nonce change (ADR 0006), and
/// the exact observation finds nothing, so the claim would be resubmitted
/// forever. Past the LEZ refund time this asks the chain for the Taker's LEZ
/// refund; the claim itself stops being submitted once the finalized LEZ time
/// is past the refund time (`followup_lez_claim_drive`).
async fn followup_lez_claim_cannot_land<Factory, Contexts, Funding, Zcash>(
    active: &lez_zec_swap_sdk::ActiveZecSwap<
        ContextOwningLezBridgePorts<Factory, Contexts, SqliteZecRecoveryStore, Funding>,
        Zcash,
        SqliteZecRecoveryStore,
    >,
    lez: &ContextOwningLezBridgePorts<Factory, Contexts, SqliteZecRecoveryStore, Funding>,
) -> Result<Option<LezClaimCannotLand>, ActorCommandError>
where
    Factory: lez_bridge_adapter::FreshLezBridgeTransportFactory,
    Factory::Transport:
        lez_bridge_adapter::LezBridgeClaimTransport + lez_bridge_adapter::LezBridgeRefundTransport,
    Contexts: lez_bridge_adapter::BridgeRequestContextSource,
    Funding: lez_bridge_adapter::CanonicalLezFundingSource,
    Zcash: lez_zec_swap_sdk::ZcashClaimPort + lez_zec_swap_sdk::ZcashRefundPort,
{
    let agreement = active.agreement();
    if agreement.direction() != SwapDirection::TakerSellsLez
        || active.local_participant() != Participant::Maker
        || active.status() != Phase::ClaimEvidenceAvailable
    {
        return Ok(None);
    }
    let now_ms = wall_clock_ms().value();
    let refund_at_ms = agreement.lez_refund_at_ms();
    if now_ms < refund_at_ms {
        return Ok(None);
    }
    let taker_refund_confirmed =
        match lez_zec_swap_sdk::LezRefundPort::observe_counterparty_refund(lez, agreement).await {
            Ok(lez_zec_swap_sdk::RefundObservationV1::Confirmed(_)) => true,
            Ok(_) => false,
            Err(error) => {
                eprintln!("actor claim: the Taker's LEZ refund could not be observed: {error:?}");
                false
            }
        };
    Ok(lez_claim_cannot_land(
        now_ms,
        refund_at_ms,
        taker_refund_confirmed,
    ))
}

/// The markers one submitted claim leaves: "submitted" (for the life of the
/// swap) for the Taker's revealing claim on either chain, and "in flight" for
/// a claim signed at this actor's LEZ account nonce (the Taker's revealing LEZ
/// claim, the Maker's follow-up LEZ claim). The Maker's follow-up Zcash claim
/// leaves neither. Each records the swap's LEZ refund time
/// ([`revealing_claim_in_flight_refund_at_ms`]).
fn mark_claim_submitted(
    config: &ActorConfig,
    step: ClaimStepV1,
    lez_refund_at_ms: u64,
    replanned: bool,
) {
    let (submitted, in_flight) = claim_markers(step);
    if submitted && let Some(marker) = revealing_claim_submitted_marker(config.role_state_db()) {
        let _ = fs::write(marker, claim_marker_text(step, lez_refund_at_ms));
    }
    if in_flight {
        if let Some(marker) = revealing_claim_in_flight_marker(config.role_state_db()) {
            let submitted_at_ms = in_flight_submitted_at_ms(
                revealing_claim_in_flight_submitted_at_ms(config.role_state_db()),
                replanned,
                wall_clock_unix_ms(),
            );
            let _ = fs::write(
                marker,
                in_flight_marker_text(step, lez_refund_at_ms, submitted_at_ms),
            );
        }
        record_revealing_claim_holder(config.role_state_db(), None);
        // The in-flight marker now holds the gate; the claim no longer waits.
        record_lez_claim_waiting(config.role_state_db(), None);
    }
}

/// Which markers a submitted claim `step` leaves: (submitted, in flight).
const fn claim_markers(step: ClaimStepV1) -> (bool, bool) {
    match step {
        ClaimStepV1::RevealingLez => (true, true),
        ClaimStepV1::RevealingZcash => (true, false),
        ClaimStepV1::FollowupLez => (false, true),
        ClaimStepV1::FollowupZcash => (false, false),
    }
}

/// The phase in which this actor's own LEZ claim is driven: the Taker's
/// revealing LEZ claim with both legs locked (`TakerSellsForeign`), the
/// Maker's follow-up LEZ claim once the Taker's Zcash claim is recorded
/// (`TakerSellsLez`). Its in-flight marker is removed once the swap leaves it.
const fn owned_lez_claim_phase(direction: SwapDirection) -> Phase {
    match direction {
        SwapDirection::TakerSellsForeign => Phase::BothLegsLocked,
        SwapDirection::TakerSellsLez => Phase::ClaimEvidenceAvailable,
    }
}

/// The text name of a claim step for the operator's stderr line.
const fn claim_name(step: ClaimStepV1) -> &'static str {
    match step {
        ClaimStepV1::RevealingLez => "revealing LEZ claim",
        ClaimStepV1::FollowupZcash => "follow-up Zcash claim",
        ClaimStepV1::RevealingZcash => "revealing Zcash claim",
        ClaimStepV1::FollowupLez => "follow-up LEZ claim",
    }
}

/// The owned LEZ claim is observed final (the swap left that claim's phase).
fn clear_revealing_claim_in_flight(config: &ActorConfig) {
    if let Some(marker) = revealing_claim_in_flight_marker(config.role_state_db()) {
        let _ = fs::remove_file(marker);
    }
    record_revealing_claim_holder(config.role_state_db(), None);
    record_lez_claim_waiting(config.role_state_db(), None);
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
/// retains the agreement's refund order (the Maker-funded leg first: LEZ in
/// `TakerSellsForeign`, Zcash in `TakerSellsLez`, where each funder drives its
/// own refund once both legs are locked), fresh funding/deadline admission,
/// and its persist-before-send journal.
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
    let sdk = ZecPairSdk::new(participant, (), (), lez.clone(), zcash, store);
    let mut active = sdk
        .resume_all_capable(config.swap_id())
        .await
        .map_err(|_| ActorCommandError::RecoveryReplayUnavailable)?
        .ok_or(ActorCommandError::NotActivated)?;

    ensure_recovery_phase(active.status())?;
    let outcome = active
        .drive_refund(wall_clock_ms())
        .await
        .map_err(unavailable_recovery)?;
    let outcome = if refund_found_the_revealing_claim(
        active.agreement().direction(),
        participant,
        active.status(),
        &outcome,
    ) {
        // The Maker's Zcash HTLC was spent, and not by its refund: the
        // Taker's revealing Zcash claim landed after all. Its preimage pays the
        // Maker's LEZ claim, so the claim is observed here rather than the
        // refund waited on (the next status names the LEZ claim).
        eprintln!(
            "actor recovery: the Zcash HTLC is spent; observing the Taker's revealing Zcash claim"
        );
        drive_claim_with_replans(
            &mut active,
            &lez,
            config,
            ActorCommandError::RecoveryUnavailable,
        )
        .await?
    } else {
        recovery_outcome(outcome)
    };
    Ok(ActorEffectOutputV1::from_active(
        config,
        ActorEffectCommandV1::Recover,
        outcome,
        &active,
    ))
}

/// Whether the Maker's Zcash refund of `TakerSellsLez`, with both legs
/// locked, found its HTLC spent: only the Taker's revealing claim spends it
/// then, and the Maker must take that claim's preimage to its LEZ claim before
/// the Taker's LEZ refund opens.
const fn refund_found_the_revealing_claim(
    direction: SwapDirection,
    local: Participant,
    phase: Phase,
    outcome: &RefundDriveOutcome,
) -> bool {
    matches!(
        (direction, local, phase, outcome),
        (
            SwapDirection::TakerSellsLez,
            Participant::Maker,
            Phase::BothLegsLocked,
            RefundDriveOutcome::AwaitingFunding {
                step: RefundStepV1::Zcash,
                reason: RefundFundingWaitReasonV1::Spent,
            },
        )
    )
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
        // The party's own submitted lock is neither confirmed nor lapsed: the
        // recovery waits on it rather than end with it outstanding (FINDINGS 27).
        RefundDriveOutcome::AwaitingOwnLockSettlement(step) => {
            ActorEffectOutcomeV1::AwaitingObservation {
                operation: first_lock_operation(step),
            }
        }
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
        ClaimStepV1::RevealingZcash => ActorOperationV1::ZcashRevealingClaim,
        ClaimStepV1::FollowupLez => ActorOperationV1::LezFollowupClaim,
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
                reported_next_action(&active, config, wall_clock_ms()),
                replans,
                Some(active.agreement().lez_refund_at_ms()),
                Some(revealing_claim_hold_attention_ms(
                    lez_zec_swap_sdk::ZecRefundProfile::for_id(
                        active.agreement().binding().profile_id(),
                    )
                    .lez_refund_delay()
                    .value(),
                )),
                Some(
                    lez_zec_swap_sdk::ZecRefundProfile::for_id(
                        active.agreement().binding().profile_id(),
                    )
                    .required_margin()
                    .value()
                    .saturating_mul(1000),
                ),
            )
            .with_maker_lock(pending_maker_lock(
                active.local_participant(),
                active.status(),
                active.agreement().direction(),
            ))
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
    #[allow(
        clippy::too_many_lines,
        reason = "every phase, every action, then the wire form"
    )]
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
            ZecLifecycleAction::RefundLez,
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
    fn the_status_names_the_makers_pending_lock_leg_only_while_it_is_next() {
        // Issues #176 and #178: at `taker_lock_confirmed` the Maker's next
        // action reads `wait` in both directions; the Node must know whether
        // the next drive signs LEZ transactions or funds Zcash.
        assert_eq!(
            pending_maker_lock(
                Participant::Maker,
                Phase::TakerLockConfirmed,
                SwapDirection::TakerSellsForeign
            ),
            Some(ActorMakerLockLegV1::Lez)
        );
        assert_eq!(
            pending_maker_lock(
                Participant::Maker,
                Phase::TakerLockConfirmed,
                SwapDirection::TakerSellsLez
            ),
            Some(ActorMakerLockLegV1::Zcash)
        );
        for (local, phase) in [
            (Participant::Taker, Phase::TakerLockConfirmed),
            (Participant::Maker, Phase::Offered),
            (Participant::Maker, Phase::BothLegsLocked),
            (Participant::Maker, Phase::ClaimEvidenceAvailable),
        ] {
            assert_eq!(
                pending_maker_lock(local, phase, SwapDirection::TakerSellsLez),
                None
            );
        }
        let status = |leg| {
            serde_json::to_value(
                ActorStatusV1::active(
                    ActorRole::Maker,
                    Phase::TakerLockConfirmed,
                    1,
                    ZecLifecycleAction::Wait,
                    Vec::new(),
                    None,
                    None,
                    None,
                )
                .with_maker_lock(leg),
            )
            .unwrap()
        };
        assert_eq!(status(Some(ActorMakerLockLegV1::Lez))["maker_lock"], "lez");
        assert_eq!(
            status(Some(ActorMakerLockLegV1::Zcash))["maker_lock"],
            "zcash"
        );
        assert!(status(None).get("maker_lock").is_none());
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
            Some(120_000),
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
    fn a_taker_sells_lez_maker_whose_zcash_is_refunded_observes_the_takers_lez_refund() {
        // After its earlier Zcash refund the Maker's swap waits on the Taker's
        // later LEZ refund. Reported as refund_lez, the Node runs the recovery,
        // which for the non-owner observes that refund and ends Refunded; as
        // wait, the Node drove a phase the drive does not serve, forever.
        for window_closed in [false, true] {
            assert_eq!(
                claimant_next_action(
                    ZecLifecycleAction::Wait,
                    Phase::MakerLegRefunded,
                    SwapDirection::TakerSellsLez,
                    Participant::Maker,
                    false,
                    window_closed,
                    false,
                ),
                ZecLifecycleAction::RefundLez
            );
        }
        // TakerSellsForeign is unchanged.
        assert_eq!(
            claimant_next_action(
                ZecLifecycleAction::Wait,
                Phase::MakerLegRefunded,
                SwapDirection::TakerSellsForeign,
                Participant::Maker,
                false,
                true,
                false,
            ),
            ZecLifecycleAction::Wait
        );
    }

    #[test]
    fn a_claimant_whose_claim_window_closed_is_told_to_refund_unless_its_claim_left() {
        use Participant::{Maker, Taker};
        use SwapDirection::{TakerSellsForeign, TakerSellsLez};
        use ZecLifecycleAction::{ClaimLez, ClaimZcash, RefundLez, RefundZcash, Wait};
        let both = Phase::BothLegsLocked;
        // TakerSellsForeign (FINDINGS 26): closed window, nothing sent → the
        // Taker's Zcash refund; open window → the claim; a claim that left
        // inside the window is still followed.
        assert_eq!(
            claimant_next_action(ClaimLez, both, TakerSellsForeign, Taker, false, true, false),
            RefundZcash
        );
        assert_eq!(
            claimant_next_action(
                ClaimLez,
                both,
                TakerSellsForeign,
                Taker,
                false,
                false,
                false
            ),
            ClaimLez
        );
        assert_eq!(
            claimant_next_action(ClaimLez, both, TakerSellsForeign, Taker, true, true, false),
            ClaimLez
        );
        // Its Maker and its follow-up Zcash claim are untouched.
        assert_eq!(
            claimant_next_action(Wait, both, TakerSellsForeign, Maker, false, true, false),
            Wait
        );
        assert_eq!(
            claimant_next_action(
                ClaimZcash,
                Phase::ClaimEvidenceAvailable,
                TakerSellsForeign,
                Maker,
                false,
                true,
                false
            ),
            ClaimZcash
        );
        // TakerSellsLez: the Taker's revealing Zcash claim, unsent when the
        // window closed → the Taker's LEZ refund; sent → still followed.
        assert_eq!(
            claimant_next_action(ClaimZcash, both, TakerSellsLez, Taker, false, true, false),
            RefundLez
        );
        assert_eq!(
            claimant_next_action(ClaimZcash, both, TakerSellsLez, Taker, false, false, false),
            ClaimZcash
        );
        assert_eq!(
            claimant_next_action(ClaimZcash, both, TakerSellsLez, Taker, true, true, false),
            ClaimZcash
        );
        // Its Maker waits (its drive observes the Taker's Zcash claim) until
        // the window closes, then refunds its Zcash (the earlier refund).
        assert_eq!(
            claimant_next_action(Wait, both, TakerSellsLez, Maker, false, false, false),
            Wait
        );
        assert_eq!(
            claimant_next_action(Wait, both, TakerSellsLez, Maker, false, true, false),
            RefundZcash
        );
        // Once the Taker's claim is recorded the Maker's LEZ claim is never
        // replaced by a refund, window or not.
        assert_eq!(
            claimant_next_action(
                ClaimLez,
                Phase::ClaimEvidenceAvailable,
                TakerSellsLez,
                Maker,
                false,
                true,
                false
            ),
            ClaimLez
        );
        // The refund phases keep the SDK's action.
        assert_eq!(
            claimant_next_action(
                RefundLez,
                Phase::MakerLegRefunded,
                TakerSellsLez,
                Taker,
                false,
                true,
                false
            ),
            RefundLez
        );
    }

    #[test]
    fn the_next_action_names_are_the_wire_contract() {
        for (action, name) in [
            (ZecLifecycleAction::Wait, "wait"),
            (ZecLifecycleAction::CreateAndFundLez, "create_and_fund_lez"),
            (ZecLifecycleAction::FundZcash, "fund_zcash"),
            (ZecLifecycleAction::ClaimLez, "claim_lez"),
            (ZecLifecycleAction::ClaimZcash, "claim_zcash"),
            (ZecLifecycleAction::RefundZcash, "refund_zcash"),
            (ZecLifecycleAction::RefundLez, "refund_lez"),
            (ZecLifecycleAction::Complete, "complete"),
        ] {
            assert_eq!(
                serde_json::to_value(ActorNextActionV1::from(action)).unwrap(),
                json!(name)
            );
            assert_eq!(
                ActorNextActionV1::from(action).as_lifecycle_action(),
                action
            );
        }
    }

    #[test]
    fn the_taker_sells_lez_maker_funds_zcash_only_inside_the_claim_window() {
        use SwapDirection::{TakerSellsForeign, TakerSellsLez};
        assert!(!maker_funding_cut_off(TakerSellsLez, false, false, false));
        assert!(maker_funding_cut_off(TakerSellsLez, false, true, false));
        // A funding staged before the cutoff may already be on the chain:
        // it is still driven to its observation.
        assert!(!maker_funding_cut_off(TakerSellsLez, true, true, false));
        // The Maker of TakerSellsForeign funds LEZ second, as before.
        assert!(!maker_funding_cut_off(
            TakerSellsForeign,
            false,
            true,
            false
        ));
    }

    #[test]
    fn the_taker_sells_lez_maker_funds_zcash_only_while_the_taker_can_still_reveal_by_height() {
        use SwapDirection::{TakerSellsForeign, TakerSellsLez};
        // devnet-v7: R = 3, C = 1, H_z = 417. The funding needs C blocks and
        // the Taker's claim then needs tip + R < H_z: tip + R + C < H_z.
        let (margin, confirmations, refund_height) = (3, 1, 417);
        assert!(!zcash_funding_window_closed(
            412,
            margin,
            confirmations,
            refund_height
        ));
        assert!(zcash_funding_window_closed(
            413,
            margin,
            confirmations,
            refund_height
        ));
        assert!(zcash_funding_window_closed(
            u32::MAX,
            margin,
            confirmations,
            refund_height
        ));
        // The height cutoff alone stops a fresh funding, as the time cutoff does.
        assert!(maker_funding_cut_off(TakerSellsLez, false, false, true));
        assert!(!maker_funding_cut_off(TakerSellsLez, true, false, true));
        assert!(!maker_funding_cut_off(
            TakerSellsForeign,
            false,
            false,
            true
        ));
    }

    #[test]
    fn the_taker_sells_lez_maker_funds_zcash_only_while_the_refund_height_comes_inside_the_window()
    {
        // public-testnet-v3: 75 s a block at the slowest. H_z must come before
        // T_lez - M at that pace: (H_z - tip) * 75 s <= T_lez - M - now.
        let slowest = lez_zec_swap_sdk::ZecRefundProfile::for_id(
            lez_zec_swap_sdk::ZecProfileId::PublicTestnetV3,
        )
        .zcash_slowest_block_millis();
        assert_eq!(slowest, Some(75_000));
        let (tip, refund_height, now, closes) = (1_000, 1_096, 10_000_000, 10_000_000 + 7_200_000);
        // At the agreement's anchor: 96 blocks * 75 s = 7 200 s, exactly the window.
        assert!(!zcash_refund_height_after_window(
            tip,
            refund_height,
            slowest,
            now,
            closes
        ));
        // One second later with no new block, H_z may come after the window.
        assert!(zcash_refund_height_after_window(
            tip,
            refund_height,
            slowest,
            now + 1_000,
            closes
        ));
        // Blocks that came in the meantime keep it inside.
        assert!(!zcash_refund_height_after_window(
            tip + 10,
            refund_height,
            slowest,
            now + 600_000,
            closes
        ));
        // A closed window, or a tip past H_z with the window closed.
        assert!(zcash_refund_height_after_window(
            tip,
            refund_height,
            slowest,
            closes + 1,
            closes
        ));
        assert!(!zcash_refund_height_after_window(
            refund_height,
            refund_height,
            slowest,
            closes,
            closes
        ));
        // Regtest profiles have no pace bound (the harness mines every block):
        // never refused on this ground.
        for profile in [
            lez_zec_swap_sdk::ZecProfileId::DeterministicLocalV3,
            lez_zec_swap_sdk::ZecProfileId::DeterministicLocalDevnetV7,
            lez_zec_swap_sdk::ZecProfileId::DeterministicLocalDevnetV8,
        ] {
            let pace =
                lez_zec_swap_sdk::ZecRefundProfile::for_id(profile).zcash_slowest_block_millis();
            assert_eq!(pace, None, "{profile:?}");
            assert!(!zcash_refund_height_after_window(
                tip,
                refund_height,
                pace,
                now + 1_000,
                closes
            ));
        }
    }

    /// Issue #182: before the Maker's lock the Taker's refund is reported
    /// only once its claim window has closed; while it is open the Maker is
    /// still in time to lock and the Taker waits.
    #[test]
    fn before_the_makers_lock_the_taker_is_told_to_refund_only_once_its_claim_window_closed() {
        use Participant::{Maker, Taker};
        use SwapDirection::{TakerSellsForeign, TakerSellsLez};
        use ZecLifecycleAction::{RefundLez, RefundZcash, Wait};
        let next = |direction, local, phase, window_closed, height_reached| {
            claimant_next_action(
                Wait,
                phase,
                direction,
                local,
                false,
                window_closed,
                height_reached,
            )
        };
        // Selling LEZ: every phase before both legs in which the recovery
        // admits the Taker's LEZ refund.
        for phase in [
            Phase::AwaitingTakerConfirmations,
            Phase::TakerLockConfirmed,
            Phase::AwaitingMakerConfirmations,
            Phase::TakerLockReorged,
            Phase::MakerLockReorged,
        ] {
            assert_eq!(
                next(TakerSellsLez, Taker, phase, false, false),
                Wait,
                "{phase:?}"
            );
            // The Maker's Zcash refund height does not open the Taker's refund.
            assert_eq!(
                next(TakerSellsLez, Taker, phase, false, true),
                Wait,
                "{phase:?}"
            );
            assert_eq!(
                next(TakerSellsLez, Taker, phase, true, false),
                RefundLez,
                "{phase:?}"
            );
        }
        // Selling ZEC: the Taker's Zcash refund, except while the Maker's
        // lock is confirming, where the recovery admits none.
        for phase in [
            Phase::AwaitingTakerConfirmations,
            Phase::TakerLockConfirmed,
            Phase::TakerLockReorged,
            Phase::MakerLockReorged,
        ] {
            assert_eq!(
                next(TakerSellsForeign, Taker, phase, false, false),
                Wait,
                "{phase:?}"
            );
            assert_eq!(
                next(TakerSellsForeign, Taker, phase, false, true),
                Wait,
                "{phase:?}"
            );
            assert_eq!(
                next(TakerSellsForeign, Taker, phase, true, false),
                RefundZcash,
                "{phase:?}"
            );
        }
        assert_eq!(
            next(
                TakerSellsForeign,
                Taker,
                Phase::AwaitingMakerConfirmations,
                true,
                false
            ),
            Wait
        );
        // Before its own lock there is nothing to refund, window or not.
        for direction in [TakerSellsLez, TakerSellsForeign] {
            assert_eq!(next(direction, Taker, Phase::Offered, true, false), Wait);
        }
        // The Maker's actions in these phases are not the Taker's rule.
        for direction in [TakerSellsLez, TakerSellsForeign] {
            for phase in [
                Phase::AwaitingTakerConfirmations,
                Phase::TakerLockConfirmed,
                Phase::AwaitingMakerConfirmations,
                Phase::TakerLockReorged,
                Phase::MakerLockReorged,
            ] {
                assert_eq!(
                    next(direction, Maker, phase, true, false),
                    Wait,
                    "{phase:?}"
                );
            }
        }
        // A first lock the Taker still has to send is never replaced.
        assert_eq!(
            claimant_next_action(
                ZecLifecycleAction::CreateAndFundLez,
                Phase::Offered,
                TakerSellsLez,
                Taker,
                false,
                true,
                false
            ),
            ZecLifecycleAction::CreateAndFundLez
        );
    }

    #[test]
    fn a_taker_sells_lez_maker_is_offered_its_zcash_refund_from_the_refund_height() {
        use Participant::{Maker, Taker};
        use SwapDirection::{TakerSellsForeign, TakerSellsLez};
        use ZecLifecycleAction::{ClaimLez, ClaimZcash, RefundZcash, Wait};
        let both = Phase::BothLegsLocked;
        // ADR 0216, TM-0030: the Maker refunds between H_z and T_lez - M. From
        // the refund height on its refund is offered whatever the clock (its
        // Node alternates the refund with observing a late Taker claim), so a
        // Taker cannot reveal past H_z and shrink the Maker's LEZ budget.
        assert_eq!(
            claimant_next_action(Wait, both, TakerSellsLez, Maker, false, false, true),
            RefundZcash
        );
        // Before H_z and inside the window it still waits.
        assert_eq!(
            claimant_next_action(Wait, both, TakerSellsLez, Maker, false, false, false),
            Wait
        );
        // The Taker's own action does not follow the Maker's height.
        assert_eq!(
            claimant_next_action(ClaimZcash, both, TakerSellsLez, Taker, false, false, true),
            ClaimZcash
        );
        // The Maker of TakerSellsForeign is unchanged.
        assert_eq!(
            claimant_next_action(Wait, both, TakerSellsForeign, Maker, false, false, true),
            Wait
        );
        // Once the reveal is recorded the Maker's LEZ claim stays.
        assert_eq!(
            claimant_next_action(
                ClaimLez,
                Phase::ClaimEvidenceAvailable,
                TakerSellsLez,
                Maker,
                false,
                false,
                true
            ),
            ClaimLez
        );
    }

    #[test]
    fn the_observed_zcash_tip_is_kept_beside_the_actor_state() {
        let root = tempfile::tempdir().expect("tempdir");
        let state_db = root.path().join("maker").join("state").join("actor.sqlite");
        std::fs::create_dir_all(state_db.parent().expect("parent")).expect("dirs");
        assert_eq!(observed_zcash_tip(&state_db), None);
        assert!(!zcash_refund_height_observed(&state_db, 417));
        record_observed_zcash_tip(&state_db, 416);
        assert_eq!(observed_zcash_tip(&state_db), Some(416));
        assert!(!zcash_refund_height_observed(&state_db, 417));
        record_observed_zcash_tip(&state_db, 417);
        assert!(zcash_refund_height_observed(&state_db, 417));
        assert_eq!(
            zcash_tip_observed_marker(&state_db),
            Some(root.path().join("maker").join("zcash-tip-observed"))
        );
        std::fs::write(
            zcash_tip_observed_marker(&state_db).expect("marker"),
            "not a height\n",
        )
        .expect("write");
        assert!(!zcash_refund_height_observed(&state_db, 0));
    }

    #[test]
    fn the_preimage_is_the_takers_and_the_candidates_the_zcash_funders() {
        use Participant::{Maker, Taker};
        // TakerSellsForeign: the Taker funds Zcash and holds the preimage.
        assert!(activation_roles_match(Taker, Taker, Taker, true, true));
        assert!(activation_roles_match(Taker, Taker, Maker, false, false));
        // TakerSellsLez: the Maker funds Zcash, the Taker holds the preimage.
        assert!(activation_roles_match(Taker, Maker, Taker, true, false));
        assert!(activation_roles_match(Taker, Maker, Maker, false, true));
        // A Maker with a preimage, or a Taker without one, is refused.
        assert!(!activation_roles_match(Taker, Maker, Maker, true, true));
        assert!(!activation_roles_match(Taker, Taker, Maker, true, false));
        assert!(!activation_roles_match(Taker, Maker, Taker, false, false));
        assert!(!activation_roles_match(Taker, Taker, Taker, false, true));
        // Candidates belong to the funder alone.
        assert!(!activation_roles_match(Taker, Maker, Taker, true, true));
        assert!(!activation_roles_match(Taker, Maker, Maker, false, false));
    }

    #[test]
    fn claim_markers_follow_the_revealing_claim_and_the_lez_nonce() {
        // The Taker's revealing claim on either chain marks "submitted"; a
        // claim at the claimant's LEZ nonce marks "in flight".
        assert_eq!(claim_markers(ClaimStepV1::RevealingLez), (true, true));
        assert_eq!(claim_markers(ClaimStepV1::RevealingZcash), (true, false));
        assert_eq!(claim_markers(ClaimStepV1::FollowupLez), (false, true));
        assert_eq!(claim_markers(ClaimStepV1::FollowupZcash), (false, false));
        assert_eq!(
            owned_lez_claim_phase(SwapDirection::TakerSellsForeign),
            Phase::BothLegsLocked
        );
        assert_eq!(
            owned_lez_claim_phase(SwapDirection::TakerSellsLez),
            Phase::ClaimEvidenceAvailable
        );
    }

    #[test]
    fn the_in_flight_marker_records_the_lez_refund_time() {
        let root = tempfile::tempdir().expect("tempdir");
        let state = root
            .path()
            .join("actor")
            .join("maker")
            .join("state.sqlite3");
        fs::create_dir_all(state.parent().unwrap()).unwrap();
        let marker = revealing_claim_in_flight_marker(&state).unwrap();
        assert_eq!(revealing_claim_in_flight_refund_at_ms(&state), None);
        fs::write(
            &marker,
            claim_marker_text(ClaimStepV1::FollowupLez, 1_800_000),
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(&marker).unwrap(),
            "follow-up LEZ claim submitted\nlez_refund_at_ms=1800000\n"
        );
        assert_eq!(
            revealing_claim_in_flight_refund_at_ms(&state),
            Some(1_800_000)
        );
        // A marker written before the time was recorded names none.
        fs::write(&marker, "follow-up LEZ claim submitted\n").unwrap();
        assert_eq!(revealing_claim_in_flight_refund_at_ms(&state), None);
    }

    #[test]
    fn the_in_flight_marker_records_when_the_current_claim_bytes_were_first_submitted() {
        // Issue #176: a Maker Node holds its other LEZ claims behind a
        // submitted one for a bounded time from its submission. The same
        // bytes sent again keep the time; a re-planned claim starts its own.
        let root = tempfile::tempdir().expect("tempdir");
        let state = root
            .path()
            .join("actor")
            .join("maker")
            .join("state.sqlite3");
        fs::create_dir_all(state.parent().unwrap()).unwrap();
        let marker = revealing_claim_in_flight_marker(&state).unwrap();
        assert_eq!(revealing_claim_in_flight_submitted_at_ms(&state), None);
        fs::write(
            &marker,
            in_flight_marker_text(ClaimStepV1::FollowupLez, 1_800_000, 1_000),
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(&marker).unwrap(),
            "follow-up LEZ claim submitted\nlez_refund_at_ms=1800000\nsubmitted_at_ms=1000\n"
        );
        assert_eq!(
            revealing_claim_in_flight_refund_at_ms(&state),
            Some(1_800_000)
        );
        assert_eq!(
            revealing_claim_in_flight_submitted_at_ms(&state),
            Some(1_000)
        );
        assert_eq!(in_flight_submitted_at_ms(Some(1_000), false, 9_000), 1_000);
        assert_eq!(in_flight_submitted_at_ms(Some(1_000), true, 9_000), 9_000);
        assert_eq!(in_flight_submitted_at_ms(None, false, 9_000), 9_000);
        // A marker written before the time was recorded names none.
        fs::write(
            &marker,
            claim_marker_text(ClaimStepV1::FollowupLez, 1_800_000),
        )
        .unwrap();
        assert_eq!(revealing_claim_in_flight_submitted_at_ms(&state), None);
    }

    #[test]
    fn a_maker_lez_claim_cannot_land_only_once_the_escrow_is_refunded() {
        // Review 3, item 2: the escrow's claim window is `..refund_at` against the
        // sequencer's block timestamp, not the Maker host's clock; a lagging
        // sequencer can still include a claim the host thinks is late. The Maker
        // gives up only on the escrow actually being refunded.
        let at = 1_800_000;
        for (now, refunded) in [
            (at - 1, false),
            (at - 1, true),
            (at + 60_000, false),
            (at + 600_000, false),
        ] {
            assert_eq!(
                lez_claim_cannot_land(now, at, refunded),
                None,
                "{now} {refunded}"
            );
        }
        assert_eq!(
            lez_claim_cannot_land(at, at, true),
            Some(LezClaimCannotLand::EscrowRefunded)
        );
    }

    #[test]
    fn a_maker_lez_claim_past_its_window_by_lez_time_is_observed_and_never_resubmitted() {
        // Review 4, M2: past `refund_at` with the Taker's LEZ refund not yet
        // final, the Maker's follow-up claim was resubmitted on every requeue.
        // Once the finalized LEZ block time is at or past `refund_at` no block
        // can include it: it is only observed (it may have landed earlier).
        let at = 1_800_000;
        for finalized in [None, Some(at - 1)] {
            assert_eq!(
                followup_lez_claim_drive(at, finalized),
                FollowupLezClaimDrive::Drive,
                "{finalized:?}"
            );
        }
        for finalized in [at, at + 1, at + 600_000] {
            assert_eq!(
                followup_lez_claim_drive(at, Some(finalized)),
                FollowupLezClaimDrive::ObserveOnly,
                "{finalized}"
            );
        }
    }

    #[test]
    fn a_lez_claim_past_the_old_replan_limit_is_reported_as_needing_attention() {
        // Issue #172: the claim goes on being re-planned past five re-plans;
        // the operator learns of it by name from the actor's wait.
        let at = 1_800_000;
        for count in [0, 1, 5] {
            assert_eq!(lez_claim_contention_wait(count, at), None, "{count}");
        }
        for count in [6, 7, 100] {
            let wait = lez_claim_contention_wait(count, at).expect("named");
            assert_eq!(wait.kind, "lez_claim_nonce_contention");
            assert_eq!(wait.chain, "lez");
            assert_eq!(wait.until_unix_seconds, Some(at / 1000));
        }
    }

    #[test]
    fn a_lost_claim_refund_is_reported_as_refunded() {
        // Issue #178: the Node accepts a claim's `completed` only at phase
        // `completed`; the recorded Zcash refund is phase `refunded`.
        assert_eq!(
            serde_json::to_value(lost_claim_refunded_outcome()).expect("outcome")["outcome"],
            json!("refunded")
        );
    }

    #[test]
    fn the_lost_claim_refund_output_is_the_one_the_maker_node_is_tested_with() {
        // Issue #178, item 1: the claim command's whole output for the
        // Maker's recorded Zcash refund after a lost LEZ claim, byte for
        // byte. The Maker Node's supervisor test
        // (`a_maker_zcash_refund_after_a_lost_lez_claim_ends_the_swap_refunded`)
        // runs an actor that prints this file, so the two sides cannot drift
        // apart again.
        let output = ActorEffectOutputV1 {
            schema_version: STATUS_SCHEMA_VERSION,
            role: ActorRole::Maker,
            command: ActorEffectCommandV1::Claim,
            outcome: lost_claim_refunded_outcome(),
            phase: Phase::Refunded.into(),
            revision: 5,
            next_action: ZecLifecycleAction::Complete.into(),
            wait: None,
        };
        assert_eq!(
            serde_json::to_string(&output).expect("output"),
            include_str!("../tests/fixtures/lost-claim-refund-claim-output.json").trim_end()
        );
    }

    #[test]
    fn the_makers_zcash_refund_after_a_lost_lez_claim_reports_by_name() {
        // Review 5, items 2 and 5: the observe-only branch's refund result.
        assert_eq!(
            lost_claim_refund_step(&RefundDriveOutcome::Refunded { revision: 7 }),
            LostClaimRefundStep::Refunded
        );
        assert_eq!(
            lost_claim_refund_step(&RefundDriveOutcome::AwaitingFunding {
                step: RefundStepV1::Zcash,
                reason: RefundFundingWaitReasonV1::Spent,
            }),
            LostClaimRefundStep::CannotLand
        );
        assert_eq!(
            lost_claim_refund_step(&RefundDriveOutcome::Submitted(RefundStepV1::Zcash)),
            LostClaimRefundStep::Submitted
        );
        for waiting in [
            RefundDriveOutcome::AwaitingStableObservation(RefundStepV1::Zcash),
            RefundDriveOutcome::AwaitingDeadline(RefundStepV1::Zcash),
            RefundDriveOutcome::AwaitingFunding {
                step: RefundStepV1::Zcash,
                reason: RefundFundingWaitReasonV1::Reorged,
            },
        ] {
            assert_eq!(
                lost_claim_refund_step(&waiting),
                LostClaimRefundStep::Waiting
            );
        }
    }

    #[test]
    fn a_maker_refund_that_finds_its_htlc_spent_observes_the_takers_claim() {
        let spent = RefundDriveOutcome::AwaitingFunding {
            step: RefundStepV1::Zcash,
            reason: RefundFundingWaitReasonV1::Spent,
        };
        assert!(refund_found_the_revealing_claim(
            SwapDirection::TakerSellsLez,
            Participant::Maker,
            Phase::BothLegsLocked,
            &spent
        ));
        for (direction, local, phase, outcome) in [
            (
                SwapDirection::TakerSellsForeign,
                Participant::Maker,
                Phase::BothLegsLocked,
                spent,
            ),
            (
                SwapDirection::TakerSellsLez,
                Participant::Taker,
                Phase::BothLegsLocked,
                spent,
            ),
            (
                SwapDirection::TakerSellsLez,
                Participant::Maker,
                Phase::TakerLegRefunded,
                spent,
            ),
            (
                SwapDirection::TakerSellsLez,
                Participant::Maker,
                Phase::BothLegsLocked,
                RefundDriveOutcome::AwaitingFunding {
                    step: RefundStepV1::Zcash,
                    reason: RefundFundingWaitReasonV1::Absent,
                },
            ),
            (
                SwapDirection::TakerSellsLez,
                Participant::Maker,
                Phase::BothLegsLocked,
                RefundDriveOutcome::AwaitingDeadline(RefundStepV1::Zcash),
            ),
        ] {
            assert!(!refund_found_the_revealing_claim(
                direction, local, phase, &outcome
            ));
        }
    }

    #[test]
    fn the_zcash_reveal_window_is_the_claim_window_exit_class() {
        assert_eq!(
            claim_attempt(
                Err(ZecSdkError::ZcashRevealWindowClosed {
                    tip_height: 120,
                    refund_height: 122,
                    margin_blocks: 3,
                }),
                ActorCommandError::ClaimUnavailable,
            ),
            Err(ActorCommandError::ClaimWindowClosed)
        );
        assert_eq!(
            claim_attempt(
                Err(ZecSdkError::ClaimWindowClosed {
                    closes_at_ms: 10,
                    now_ms: 11,
                }),
                ActorCommandError::ClaimUnavailable,
            ),
            Err(ActorCommandError::ClaimWindowClosed)
        );
    }

    /// Issue #130: the reveal gate's depth shortfall is a refusal named with
    /// its counts; nothing is sent either way.
    #[test]
    fn a_shallow_zcash_funding_names_the_queued_claims_confirmations() {
        assert_eq!(
            claim_attempt(
                Err(ZecSdkError::InvalidZcashFundingObservation(
                    lez_zec_swap_sdk::ZcashFundingObservationError::InsufficientConfirmations {
                        required: 2,
                        actual: 1,
                    }
                )),
                ActorCommandError::ClaimUnavailable,
            ),
            Err(ActorCommandError::ClaimAwaitingZcashConfirmations {
                required: 2,
                actual: 1
            })
        );
        assert_eq!(
            claim_attempt(
                Err(ZecSdkError::InvalidZcashFundingObservation(
                    lez_zec_swap_sdk::ZcashFundingObservationError::CanonicalValueMismatch
                )),
                ActorCommandError::ClaimUnavailable,
            ),
            Err(ActorCommandError::ClaimUnavailable)
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
            (
                ClaimDriveOutcome::Submitted(ClaimStepV1::RevealingZcash),
                json!({"outcome":"submitted","operation":"zcash_revealing_claim"}),
            ),
            (
                ClaimDriveOutcome::AwaitingStableObservation(ClaimStepV1::RevealingZcash),
                json!({"outcome":"awaiting_observation","operation":"zcash_revealing_claim"}),
            ),
            (
                ClaimDriveOutcome::Projected {
                    step: ClaimStepV1::FollowupLez,
                    revision: 6,
                },
                json!({"outcome":"projected","operation":"lez_followup_claim"}),
            ),
            (
                ClaimDriveOutcome::Submitted(ClaimStepV1::FollowupLez),
                json!({"outcome":"submitted","operation":"lez_followup_claim"}),
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
