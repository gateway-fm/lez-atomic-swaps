//! Concrete lifecycle errors, secret material, and role-local action projection.

use std::error::Error;

use lez_swap_core::{Participant, Phase, SwapCoordinator, SwapId};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::{
    ClaimError, FirstLockIntentError, FirstLockTransitionError, LezObservationTrackerError,
    MakerLockError, ObservationTrackerError, ObservedMakerLockError,
    ObservedTakerFirstLockTransitionError, ZecAgreementV1Error,
};

/// A SHA-256 claim preimage that is redacted, zeroized, and not serializable.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct ClaimPreimage([u8; 32]);

impl ClaimPreimage {
    /// Wraps locally owned claim material.
    #[must_use]
    pub const fn new(secret: [u8; 32]) -> Self {
        Self(secret)
    }

    /// Exposes the secret only to a concrete LEZ or Zcash signing capability.
    #[must_use]
    pub const fn expose_secret(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for ClaimPreimage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ClaimPreimage([REDACTED])")
    }
}

/// Boxed structured port source retained without reducing it to a string.
pub type BoxPortError = Box<dyn Error + Send + Sync + 'static>;

/// Public lifecycle/facade failure taxonomy.
#[derive(Debug, thiserror::Error)]
pub enum ZecSdkError {
    /// An operation was requested by the wrong fixed local role.
    #[error("operation requires {expected:?}; local participant is {actual:?}")]
    WrongRole {
        /// Participant allowed to invoke the operation.
        expected: Participant,
        /// Participant fixed when this SDK instance was constructed.
        actual: Participant,
    },
    /// Negotiated or durable terms violated an immutable agreement invariant.
    #[error(transparent)]
    InvalidAgreement(#[from] ZecAgreementV1Error),
    /// Prepared first-lock material violates role, direction, shape, or size invariants.
    #[error(transparent)]
    InvalidFirstLock(#[from] FirstLockIntentError),
    /// Confirmed first-lock evidence or its durable transition is invalid.
    #[error(transparent)]
    InvalidFirstLockTransition(#[from] FirstLockTransitionError),
    /// Maker-local taker-lock evidence or durable projection is invalid.
    #[error(transparent)]
    InvalidObservedTakerFirstLockTransition(#[from] ObservedTakerFirstLockTransitionError),
    /// Taker-local maker-lock evidence or durable projection is invalid.
    #[error(transparent)]
    InvalidObservedMakerLockTransition(#[from] ObservedMakerLockError),
    /// Maker second-lock recovery material or confirmed transition is invalid.
    #[error(transparent)]
    InvalidMakerLock(#[from] MakerLockError),
    /// Claim material or a claim transition violates the accepted agreement.
    #[error(transparent)]
    InvalidClaim(#[from] ClaimError),
    /// The durable coordinator cannot identify the exact agreement-bound Zcash funding outpoint.
    #[error(transparent)]
    InvalidZcashClaimContext(#[from] crate::ZcashClaimContextError),
    /// Canonical Zcash observation history is stale, duplicated, or missing replacement proof.
    #[error(transparent)]
    InvalidZcashObservationHistory(#[from] ObservationTrackerError),
    /// Canonical LEZ observation history is stale, regressed, or missing replacement proof.
    #[error(transparent)]
    InvalidLezObservationHistory(#[from] LezObservationTrackerError),
    /// A new activation must begin at durable revision zero.
    #[error("new LEZ/ZEC agreement has invalid initial revision {0}")]
    InvalidActivationRevision(u64),
    /// The accepted or durable role differs from this fixed SDK role.
    #[error("stored local role is {actual:?}; SDK role is {expected:?}")]
    LocalRoleMismatch {
        /// Role fixed on this SDK instance.
        expected: Participant,
        /// Role supplied by the accepted or durable record.
        actual: Participant,
    },
    /// A durable lookup returned a valid record for a different swap.
    #[error("durable agreement identity does not match requested swap")]
    AgreementIdentityMismatch {
        /// ID explicitly requested by the caller.
        requested: SwapId,
        /// ID re-derived from the validated durable wire.
        actual: SwapId,
    },
    /// A different immutable agreement already occupies this role-local key.
    #[error("a conflicting immutable LEZ/ZEC agreement is already durable")]
    AgreementConflict,
    /// A different exact first-lock plan already occupies this role-local swap key.
    #[error("a conflicting immutable first-lock plan is already durable")]
    FirstLockConflict,
    /// A different exact maker-lock plan already occupies this role-local swap key.
    #[error("a conflicting immutable maker-lock plan is already durable")]
    MakerLockConflict,
    /// No durable first-lock plan exists for the active agreement.
    #[error("no durable first-lock plan exists for the active agreement")]
    MissingFirstLockIntent,
    /// No durable maker-lock plan exists for the active agreement.
    #[error("no durable maker-lock plan exists for the active agreement")]
    MissingMakerLockIntent,
    /// No protected claim preimage exists for the agreement-directed local claimant.
    #[error("no protected claim material exists for the active agreement")]
    MissingClaimMaterial,
    /// No durable protected exact submission exists for the active claim step.
    #[error("no durable claim intent exists for the active claim step")]
    MissingClaimIntent,
    /// A different protected exact claim submission occupies this role-local key.
    #[error("a conflicting immutable claim intent is already durable")]
    ClaimIntentConflict,
    /// Claim driving is unavailable before both locks are confirmed.
    #[error("claim driving requires BothLegsLocked or later; active phase is {0:?}")]
    ClaimNotReady(Phase),
    /// First-lock intent may only be staged from the fresh offered phase.
    #[error("first-lock intent requires Offered; active phase is {0:?}")]
    FirstLockNotOffered(Phase),
    /// Taker can observe maker funding only after its own first lock is confirmed.
    #[error("maker-lock observation requires TakerLockConfirmed; active phase is {0:?}")]
    MakerLockObservationNotReady(Phase),
    /// Store reported a revision inconsistent with the exact committed transition.
    #[error("first-lock projection committed revision {actual}; expected {expected}")]
    InvalidProjectionRevision {
        /// Only valid next aggregate revision.
        expected: u64,
        /// Revision returned by the store.
        actual: u64,
    },
    /// Offer discovery/publishing failed in its adapter.
    #[error("offer discovery failed")]
    Discovery(#[source] BoxPortError),
    /// Pre-lock negotiation failed in its adapter.
    #[error("negotiation failed")]
    Negotiation(#[source] BoxPortError),
    /// Agreement persistence failed before activation.
    #[error("recovery persistence failed")]
    Persistence(#[source] BoxPortError),
    /// LEZ first-lock observation or submission failed.
    #[error("LEZ first-lock adapter failed")]
    LezFirstLock(#[source] BoxPortError),
    /// Zcash first-lock observation or submission failed.
    #[error("Zcash first-lock adapter failed")]
    ZcashFirstLock(#[source] BoxPortError),
    /// Maker observation of the taker's LEZ first lock failed.
    #[error("LEZ taker-first-lock observation adapter failed")]
    LezTakerFirstLockObservation(#[source] BoxPortError),
    /// Maker observation of the taker's Zcash first lock failed.
    #[error("Zcash taker-first-lock observation adapter failed")]
    ZcashTakerFirstLockObservation(#[source] BoxPortError),
    /// Taker observation of the maker's LEZ second lock failed.
    #[error("LEZ maker-lock observation adapter failed")]
    LezMakerLockObservation(#[source] BoxPortError),
    /// Taker observation of the maker's Zcash second lock failed.
    #[error("Zcash maker-lock observation adapter failed")]
    ZcashMakerLockObservation(#[source] BoxPortError),
    /// LEZ revealing-claim preparation, observation, or submission failed.
    #[error("LEZ claim adapter failed")]
    LezClaim(#[source] BoxPortError),
    /// Zcash follow-up claim preparation, observation, or submission failed.
    #[error("Zcash claim adapter failed")]
    ZcashClaim(#[source] BoxPortError),
}

/// Next high-level role action derived without accepting peer messages.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ZecLifecycleAction {
    /// No safe local action is currently available.
    Wait,
    /// Create and fund the LEZ escrow through the generated client adapter.
    CreateAndFundLez,
    /// Construct and fund the transparent BIP-199 output.
    FundZcash,
    /// Claim LEZ first and reveal the SHA-256 preimage.
    ClaimLez,
    /// Claim the Zcash HTLC using already persisted claim material.
    ClaimZcash,
    /// The swap has reached its completed phase.
    Complete,
}

pub(crate) fn next_action(coordinator: &SwapCoordinator, local: Participant) -> ZecLifecycleAction {
    match coordinator.phase() {
        Phase::Offered if local == Participant::Taker => match coordinator.funded_chain(local) {
            lez_swap_core::Chain::Lez => ZecLifecycleAction::CreateAndFundLez,
            lez_swap_core::Chain::Zcash => ZecLifecycleAction::FundZcash,
            _ => ZecLifecycleAction::Wait,
        },
        Phase::TakerLockConfirmed if local == Participant::Maker => {
            match coordinator.funded_chain(local) {
                lez_swap_core::Chain::Lez => ZecLifecycleAction::CreateAndFundLez,
                lez_swap_core::Chain::Zcash => ZecLifecycleAction::FundZcash,
                _ => ZecLifecycleAction::Wait,
            }
        }
        Phase::BothLegsLocked if local == coordinator.first_claimant() => {
            ZecLifecycleAction::ClaimLez
        }
        Phase::ClaimEvidenceAvailable if local == coordinator.first_claimant().other() => {
            ZecLifecycleAction::ClaimZcash
        }
        Phase::Completed => ZecLifecycleAction::Complete,
        _ => ZecLifecycleAction::Wait,
    }
}
