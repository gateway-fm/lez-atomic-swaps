//! Taker-local durable projection of the maker's confirmed second lock.

use lez_swap_core::{ChainProof, Participant, SwapCoordinator, SwapDirection, SwapId};

use crate::{
    FirstLockConfirmedEvidenceV1, FirstLockProjectionCommit, FirstLockStepV1,
    FirstLockTransitionError, ZecAgreementV1,
};

/// Stable payload version for taker-local maker-lock observation transitions.
pub const OBSERVED_MAKER_LOCK_SCHEMA_V1: u16 = 1;

/// Stable result returned by the taker's observation-only maker-lock adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MakerLockObservationV1 {
    /// A fresh stable query proves the agreement-directed maker lock absent.
    Absent,
    /// The query cannot prove stable presence or stable absence.
    Unstable,
    /// Stable adapter evidence for the maker's confirmed final lock step.
    ///
    /// The expected submission identity is asserted by the chain adapter. It is
    /// durably bound and checked for consistency on replay, but cannot be
    /// independently derived from the taker's agreement or a maker-local intent.
    Confirmed(FirstLockConfirmedEvidenceV1),
}

/// One taker-local maker-lock observation attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObserveMakerLockOutcome {
    /// Stable evidence is not yet available and no transition was committed.
    AwaitingStableObservation(FirstLockStepV1),
    /// Exact confirmed evidence was atomically projected.
    Projected(FirstLockProjectionCommit),
    /// Restart replay found the transition already durable.
    AlreadyObserved {
        /// Durable aggregate revision after replay.
        revision: u64,
    },
}

/// Taker-local transition proving the maker's agreement-directed second lock.
///
/// This transition deliberately has no local effect intent: the taker observes
/// a remote effect and atomically projects its confirmed identity into its own
/// aggregate history.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservedMakerLockTransitionV1 {
    schema_version: u16,
    swap_id: SwapId,
    agreement_commitment: [u8; 32],
    local_participant: Participant,
    predecessor_revision: u64,
    evidence: FirstLockConfirmedEvidenceV1,
}

impl ObservedMakerLockTransitionV1 {
    pub(crate) fn from_active(
        agreement: &ZecAgreementV1,
        local_participant: Participant,
        predecessor_revision: u64,
        evidence: FirstLockConfirmedEvidenceV1,
    ) -> Result<Self, ObservedMakerLockError> {
        if local_participant != Participant::Taker {
            return Err(ObservedMakerLockError::WrongRole(local_participant));
        }
        let expected_step = maker_final_lock_step(agreement.direction());
        if evidence.step() != expected_step {
            return Err(ObservedMakerLockError::WrongFinalStep {
                expected: expected_step,
                actual: evidence.step(),
            });
        }
        let required = agreement
            .coordinator()
            .required_confirmations(Participant::Maker);
        if evidence.confirmations() < required {
            return Err(ObservedMakerLockError::InsufficientConfirmations {
                required,
                actual: evidence.confirmations(),
            });
        }
        Ok(Self {
            schema_version: OBSERVED_MAKER_LOCK_SCHEMA_V1,
            swap_id: agreement.coordinator().id().clone(),
            agreement_commitment: *agreement.agreement_commitment(),
            local_participant,
            predecessor_revision,
            evidence,
        })
    }

    /// Transition payload schema.
    #[must_use]
    pub const fn schema_version(&self) -> u16 {
        self.schema_version
    }

    /// Signed application swap identity.
    #[must_use]
    pub const fn swap_id(&self) -> &SwapId {
        &self.swap_id
    }

    /// Commitment to the exact executable agreement.
    #[must_use]
    pub const fn agreement_commitment(&self) -> &[u8; 32] {
        &self.agreement_commitment
    }

    /// Role whose independent store owns this observation.
    #[must_use]
    pub const fn local_participant(&self) -> Participant {
        self.local_participant
    }

    /// Exact aggregate head preceding maker funding.
    #[must_use]
    pub const fn predecessor_revision(&self) -> u64 {
        self.predecessor_revision
    }

    /// Confirmed final-step evidence, including expected submission and chain transaction IDs.
    #[must_use]
    pub const fn evidence(&self) -> &FirstLockConfirmedEvidenceV1 {
        &self.evidence
    }

    /// Revalidates and applies this transition to an exact taker-local aggregate head.
    ///
    /// # Errors
    ///
    /// Rejects an agreement, role, revision, direction, confirmation, identity,
    /// or core-phase mismatch.
    pub fn apply_to(
        &self,
        agreement: &ZecAgreementV1,
        coordinator: &SwapCoordinator,
        revision: u64,
    ) -> Result<SwapCoordinator, ObservedMakerLockError> {
        if self.schema_version != OBSERVED_MAKER_LOCK_SCHEMA_V1
            || self.swap_id != *agreement.coordinator().id()
            || self.agreement_commitment != *agreement.agreement_commitment()
            || self.local_participant != Participant::Taker
            || self.predecessor_revision != revision
            || coordinator.id() != &self.swap_id
            || self.evidence.schema_version() != 1
            || self.evidence.step() != maker_final_lock_step(agreement.direction())
        {
            return Err(ObservedMakerLockError::ContextMismatch);
        }
        let required = coordinator.required_confirmations(Participant::Maker);
        if self.evidence.confirmations() < required {
            return Err(ObservedMakerLockError::InsufficientConfirmations {
                required,
                actual: self.evidence.confirmations(),
            });
        }
        let mut next = coordinator.clone();
        next.observe_funding(
            Participant::Maker,
            ChainProof::new(
                self.evidence.transaction_id().to_owned(),
                self.evidence.confirmations(),
            )
            .map_err(ObservedMakerLockError::Core)?,
        )
        .map_err(ObservedMakerLockError::Core)?;
        Ok(next)
    }
}

/// Invalid taker-local maker-lock observation or durable transition.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ObservedMakerLockError {
    /// Only the taker owns this remote observation transition.
    #[error("observed maker-lock transition requires Taker; actual role is {0:?}")]
    WrongRole(Participant),
    /// Evidence names the wrong chain's final maker-lock step.
    #[error("observed maker-lock step is {actual:?}; expected {expected:?}")]
    WrongFinalStep {
        /// Agreement-derived maker final step.
        expected: FirstLockStepV1,
        /// Adapter-supplied final step.
        actual: FirstLockStepV1,
    },
    /// Stable evidence is below the signed maker threshold.
    #[error("observed maker lock has {actual} confirmations; {required} required")]
    InsufficientConfirmations {
        /// Signed maker threshold.
        required: u32,
        /// Observed stable depth.
        actual: u32,
    },
    /// Durable transition fields do not match the active agreement and aggregate head.
    #[error("observed maker-lock transition context mismatch")]
    ContextMismatch,
    /// Primitive evidence was malformed.
    #[error(transparent)]
    Evidence(#[from] FirstLockTransitionError),
    /// Core rejected the reconstructed proof or transition.
    #[error(transparent)]
    Core(lez_swap_core::Error),
}

pub(crate) const fn maker_final_lock_step(direction: SwapDirection) -> FirstLockStepV1 {
    match direction {
        SwapDirection::TakerSellsForeign => FirstLockStepV1::LezFund,
        SwapDirection::TakerSellsLez => FirstLockStepV1::ZcashFund,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maker_final_step_is_opposite_the_taker_first_lock() {
        assert_eq!(
            maker_final_lock_step(SwapDirection::TakerSellsForeign),
            FirstLockStepV1::LezFund
        );
        assert_eq!(
            maker_final_lock_step(SwapDirection::TakerSellsLez),
            FirstLockStepV1::ZcashFund
        );
    }

    #[test]
    fn error_retains_direction_derived_step_mismatch() {
        let error = ObservedMakerLockError::WrongFinalStep {
            expected: maker_final_lock_step(SwapDirection::TakerSellsForeign),
            actual: FirstLockStepV1::ZcashFund,
        };
        assert_eq!(
            error,
            ObservedMakerLockError::WrongFinalStep {
                expected: FirstLockStepV1::LezFund,
                actual: FirstLockStepV1::ZcashFund,
            }
        );
    }
}
