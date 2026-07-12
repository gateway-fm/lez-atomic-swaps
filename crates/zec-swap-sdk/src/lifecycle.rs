//! Immutable agreement and secret types for the LEZ/ZEC SDK lifecycle.

use std::error::Error;

use lez_swap_core::{Pair, Participant, Phase, SwapCoordinator};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::{ZecRefundProfile, ZecSwapBinding};

/// First version of the immutable LEZ/ZEC agreement schema.
pub const ZEC_AGREEMENT_SCHEMA_V1: u16 = 1;

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

/// Mutually authenticated immutable terms returned by a negotiation adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ZecAgreement<LezTerms> {
    schema_version: u16,
    coordinator: SwapCoordinator,
    binding: ZecSwapBinding,
    lez_terms: LezTerms,
    transcript_commitment: [u8; 32],
}

impl<LezTerms> ZecAgreement<LezTerms> {
    /// Validates pair, schema, transcript, profile, and both role policies.
    ///
    /// # Errors
    ///
    /// Returns [`ZecAgreementError`] before persistence or any chain effect.
    pub fn new(
        schema_version: u16,
        coordinator: SwapCoordinator,
        binding: ZecSwapBinding,
        lez_terms: LezTerms,
        transcript_commitment: [u8; 32],
    ) -> Result<Self, ZecAgreementError> {
        if schema_version != ZEC_AGREEMENT_SCHEMA_V1 {
            return Err(ZecAgreementError::UnsupportedSchema(schema_version));
        }
        if coordinator.pair() != Pair::Zcash {
            return Err(ZecAgreementError::WrongPair(coordinator.pair()));
        }
        if coordinator.phase() != Phase::Offered {
            return Err(ZecAgreementError::NonInitialPhase(coordinator.phase()));
        }
        if transcript_commitment == [0; 32] {
            return Err(ZecAgreementError::EmptyTranscriptCommitment);
        }
        let profile = ZecRefundProfile::for_id(binding.profile_id());
        for participant in [Participant::Maker, Participant::Taker] {
            let expected = if coordinator.funded_chain(participant) == lez_swap_core::Chain::Zcash {
                profile.zcash_confirmations()
            } else {
                profile.lez_confirmations()
            };
            let actual = coordinator.required_confirmations(participant);
            if actual != expected {
                return Err(ZecAgreementError::ConfirmationPolicyMismatch {
                    participant,
                    expected,
                    actual,
                });
            }
        }
        Ok(Self {
            schema_version,
            coordinator,
            binding,
            lez_terms,
            transcript_commitment,
        })
    }

    /// Agreement schema version committed by both roles.
    #[must_use]
    pub const fn schema_version(&self) -> u16 {
        self.schema_version
    }

    /// Deterministic coordinator initialized from the immutable agreement.
    #[must_use]
    pub const fn coordinator(&self) -> &SwapCoordinator {
        &self.coordinator
    }

    /// Exact profile and BIP-199 output binding.
    #[must_use]
    pub const fn binding(&self) -> &ZecSwapBinding {
        &self.binding
    }

    /// Typed LEZ terms supplied by the generated escrow client adapter.
    #[must_use]
    pub const fn lez_terms(&self) -> &LezTerms {
        &self.lez_terms
    }

    /// Commitment to the mutually authenticated pre-lock transcript.
    #[must_use]
    pub const fn transcript_commitment(&self) -> &[u8; 32] {
        &self.transcript_commitment
    }
}

/// Invalid immutable agreement returned by a negotiation adapter.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ZecAgreementError {
    /// Only schema version 1 is currently understood.
    #[error("unsupported LEZ/ZEC agreement schema {0}")]
    UnsupportedSchema(u16),
    /// A pair-specific facade never accepts foreign pair terms.
    #[error("LEZ/ZEC agreement uses wrong pair {0:?}")]
    WrongPair(Pair),
    /// Negotiation may only activate a fresh, effect-free aggregate.
    #[error("LEZ/ZEC agreement coordinator is already in phase {0:?}")]
    NonInitialPhase(Phase),
    /// An all-zero value cannot bind a countersigned transcript.
    #[error("LEZ/ZEC agreement transcript commitment is empty")]
    EmptyTranscriptCommitment,
    /// Role policy disagrees with the immutable named profile.
    #[error(
        "{participant:?} confirmation policy is {actual}; immutable profile requires {expected}"
    )]
    ConfirmationPolicyMismatch {
        /// Participant whose funded leg has a mismatched threshold.
        participant: Participant,
        /// Threshold selected by the named profile for that participant's chain.
        expected: u32,
        /// Threshold supplied by negotiated coordinator terms.
        actual: u32,
    },
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
    /// Negotiated terms violated an immutable agreement invariant.
    #[error(transparent)]
    InvalidAgreement(#[from] ZecAgreementError),
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
