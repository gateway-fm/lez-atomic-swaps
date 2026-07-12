//! Concrete lifecycle errors, secret material, and role-local action projection.

use std::error::Error;

use lez_swap_core::{Participant, Phase, SwapCoordinator, SwapId};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::ZecAgreementV1Error;

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
    /// Offer discovery/publishing failed in its adapter.
    #[error("offer discovery failed")]
    Discovery(#[source] BoxPortError),
    /// Pre-lock negotiation failed in its adapter.
    #[error("negotiation failed")]
    Negotiation(#[source] BoxPortError),
    /// Agreement persistence failed before activation.
    #[error("recovery persistence failed")]
    Persistence(#[source] BoxPortError),
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
