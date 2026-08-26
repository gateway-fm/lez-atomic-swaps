//! Durable maker second-lock intent and confirmed transition.

use lez_swap_core::{ChainProof, Participant, SwapCoordinator, SwapDirection, SwapId};

use crate::{
    FirstLockConfirmedEvidenceV1, FirstLockDriveOutcome, FirstLockPlanV1, FirstLockStepV1,
    MakerFundingEligibilityOutcome, ZecAgreementV1,
};

/// One outcome from a maker-lock attempt that always performs a fresh eligibility check.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MakerLockDriveOutcome {
    /// Another instance already committed maker funding; replay caught this instance up.
    AlreadyLocked {
        /// Durable aggregate revision reconstructed without a node effect.
        revision: u64,
    },
    /// No maker effect was staged or submitted because the fresh check was not eligible.
    AwaitingEligibility(MakerFundingEligibilityOutcome),
    /// The exact durable opposite-chain plan was safely driven.
    Lock(FirstLockDriveOutcome),
}

/// Immutable maker second-lock plan retained before the first node call.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MakerLockIntentV1 {
    swap_id: SwapId,
    agreement_commitment: [u8; 32],
    local_participant: Participant,
    staged_revision: u64,
    plan: FirstLockPlanV1,
}

impl MakerLockIntentV1 {
    pub(crate) fn from_active(
        agreement: &ZecAgreementV1,
        local_participant: Participant,
        staged_revision: u64,
        plan: FirstLockPlanV1,
    ) -> Result<Self, MakerLockError> {
        if local_participant != Participant::Maker {
            return Err(MakerLockError::WrongRole(local_participant));
        }
        match (agreement.direction(), &plan) {
            (SwapDirection::TakerSellsForeign, FirstLockPlanV1::Lez { .. })
            | (SwapDirection::TakerSellsLez, FirstLockPlanV1::Zcash { .. }) => {}
            (direction, _) => return Err(MakerLockError::WrongPlanForDirection(direction)),
        }
        Ok(Self {
            swap_id: agreement.coordinator().id().clone(),
            agreement_commitment: *agreement.agreement_commitment(),
            local_participant,
            staged_revision,
            plan,
        })
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

    /// Role fixed by this recovery record.
    #[must_use]
    pub const fn local_participant(&self) -> Participant {
        self.local_participant
    }

    /// Aggregate revision at which the immutable plan first became durable.
    #[must_use]
    pub const fn staged_revision(&self) -> u64 {
        self.staged_revision
    }

    /// Exact opposite-chain recovery plan.
    #[must_use]
    pub const fn plan(&self) -> &FirstLockPlanV1 {
        &self.plan
    }

    pub(crate) fn validate_for_active(
        &self,
        agreement: &ZecAgreementV1,
        current_revision: u64,
    ) -> Result<(), MakerLockError> {
        if self.staged_revision > current_revision {
            return Err(MakerLockError::StagedRevisionAhead {
                staged: self.staged_revision,
                current: current_revision,
            });
        }
        let expected = Self::from_active(
            agreement,
            Participant::Maker,
            self.staged_revision,
            self.plan.clone(),
        )?;
        if &expected == self {
            Ok(())
        } else {
            Err(MakerLockError::DurableIntentMismatch)
        }
    }
}

/// Exact maker funding transition committed at the then-current aggregate head.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MakerLockTransitionV1 {
    schema_version: u16,
    swap_id: SwapId,
    agreement_commitment: [u8; 32],
    local_participant: Participant,
    predecessor_revision: u64,
    intent_staged_revision: u64,
    evidence: FirstLockConfirmedEvidenceV1,
}

impl MakerLockTransitionV1 {
    pub(crate) fn from_active(
        agreement: &ZecAgreementV1,
        intent: &MakerLockIntentV1,
        predecessor_revision: u64,
        evidence: FirstLockConfirmedEvidenceV1,
    ) -> Result<Self, MakerLockError> {
        intent.validate_for_active(agreement, predecessor_revision)?;
        let final_submission = match intent.plan() {
            FirstLockPlanV1::Zcash { funding } => funding,
            FirstLockPlanV1::Lez { fund, .. } => fund,
        };
        if evidence.step() != final_submission.step() {
            return Err(MakerLockError::WrongFinalStep {
                expected: final_submission.step(),
                actual: evidence.step(),
            });
        }
        if evidence.expected_submission_id() != final_submission.expected_submission_id() {
            return Err(MakerLockError::SubmissionIdentityMismatch);
        }
        let required = agreement
            .coordinator()
            .required_confirmations(Participant::Maker);
        if evidence.confirmations() < required {
            return Err(MakerLockError::InsufficientConfirmations {
                required,
                actual: evidence.confirmations(),
            });
        }
        Ok(Self {
            schema_version: 1,
            swap_id: agreement.coordinator().id().clone(),
            agreement_commitment: *agreement.agreement_commitment(),
            local_participant: Participant::Maker,
            predecessor_revision,
            intent_staged_revision: intent.staged_revision(),
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

    /// Exact aggregate head preceding maker funding.
    #[must_use]
    pub const fn predecessor_revision(&self) -> u64 {
        self.predecessor_revision
    }

    /// Revision that owns the retained immutable intent.
    #[must_use]
    pub const fn intent_staged_revision(&self) -> u64 {
        self.intent_staged_revision
    }

    /// Confirmed final-step evidence.
    #[must_use]
    pub const fn evidence(&self) -> &FirstLockConfirmedEvidenceV1 {
        &self.evidence
    }

    pub(crate) const fn agreement_commitment(&self) -> &[u8; 32] {
        &self.agreement_commitment
    }

    pub(crate) const fn local_participant(&self) -> Participant {
        self.local_participant
    }

    /// Revalidates and applies this primitive transition to an exact aggregate head.
    ///
    /// Persistence adapters use this during full-history replay before trusting
    /// the stored active revision.
    ///
    /// # Errors
    ///
    /// Rejects any agreement, role, revision, confirmation, or core-phase mismatch.
    pub fn apply_to(
        &self,
        agreement: &ZecAgreementV1,
        coordinator: &SwapCoordinator,
        revision: u64,
    ) -> Result<SwapCoordinator, MakerLockError> {
        if self.schema_version != 1
            || self.swap_id != *agreement.coordinator().id()
            || self.agreement_commitment != *agreement.agreement_commitment()
            || self.local_participant != Participant::Maker
            || self.predecessor_revision != revision
            || self.intent_staged_revision > self.predecessor_revision
            || coordinator.id() != &self.swap_id
        {
            return Err(MakerLockError::ContextMismatch);
        }
        let required = coordinator.required_confirmations(Participant::Maker);
        if self.evidence.confirmations() < required {
            return Err(MakerLockError::InsufficientConfirmations {
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
            .map_err(MakerLockError::Core)?,
        )
        .map_err(MakerLockError::Core)?;
        Ok(next)
    }
}

/// Invalid maker second-lock intent or transition.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum MakerLockError {
    /// Only the fixed maker may create the second lock.
    #[error("maker lock requires Maker; local participant is {0:?}")]
    WrongRole(Participant),
    /// The second-lock plan must use the chain opposite the taker's signed direction.
    #[error("maker-lock plan does not match signed direction {0:?}")]
    WrongPlanForDirection(SwapDirection),
    /// A durable staged revision cannot be ahead of the active aggregate.
    #[error("maker-lock intent was staged at revision {staged}; active revision is {current}")]
    StagedRevisionAhead {
        /// Durable staging revision.
        staged: u64,
        /// Current aggregate revision.
        current: u64,
    },
    /// Loaded material differs from the accepted agreement context.
    #[error("durable maker-lock intent does not match the active agreement context")]
    DurableIntentMismatch,
    /// Evidence names a different final plan step.
    #[error("confirmed maker step is {actual:?}; durable plan requires {expected:?}")]
    WrongFinalStep {
        /// Required final step.
        expected: FirstLockStepV1,
        /// Supplied step.
        actual: FirstLockStepV1,
    },
    /// Evidence identity differs from the durable exact submission.
    #[error("confirmed maker-lock identity does not match durable submission")]
    SubmissionIdentityMismatch,
    /// Evidence is below the maker's signed threshold.
    #[error("confirmed maker lock has {actual} confirmations; requires {required}")]
    InsufficientConfirmations {
        /// Signed threshold.
        required: u32,
        /// Observed depth.
        actual: u32,
    },
    /// Transition does not match agreement, role, revision, or aggregate.
    #[error("durable maker-lock transition context mismatch")]
    ContextMismatch,
    /// Core rejected the reconstructed proof or transition.
    #[error(transparent)]
    Core(lez_swap_core::Error),
}
