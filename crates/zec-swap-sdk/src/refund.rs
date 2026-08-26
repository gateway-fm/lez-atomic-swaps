//! Agreement-derived, peer-independent timeout recovery contracts.

use lez_swap_core::{
    Chain, ChainPosition, Participant, Phase, SwapCoordinator, SwapDirection, SwapId,
};

use crate::ZecAgreementV1;

/// Maximum exact signed refund submission retained by one durable intent.
pub const MAX_REFUND_SUBMISSION_BYTES: usize = 2_000_000;

/// The fixed cross-chain refund order for transparent Zcash swaps.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RefundStepV1 {
    /// Recover the LEZ-funded leg at the earlier timestamp deadline.
    Lez,
    /// Recover the Zcash-funded leg only after the later CLTV deadline.
    Zcash,
}

impl RefundStepV1 {
    /// Returns the next refund step, preserving LEZ-before-Zcash order.
    #[must_use]
    pub const fn next(self) -> Option<Self> {
        match self {
            Self::Lez => Some(Self::Zcash),
            Self::Zcash => None,
        }
    }

    /// Chain on which this refund is executed.
    #[must_use]
    pub const fn chain(self) -> Chain {
        match self {
            Self::Lez => Chain::Lez,
            Self::Zcash => Chain::Zcash,
        }
    }

    /// Agreement-fixed participant that funded and may sign this refund.
    #[must_use]
    pub const fn owner(self, agreement: &ZecAgreementV1) -> Participant {
        match self {
            Self::Lez => agreement.lez_depositor(),
            Self::Zcash => agreement.lez_claimant(),
        }
    }

    // The Taker may recover its earlier LEZ-funded leg while the Maker second lock is
    // still confirming. The forward direction is intentionally excluded because its
    // Taker-funded Zcash leg is later and must remain fenced behind a projected LEZ refund.
    fn one_leg_lez_refund_is_valid(direction: SwapDirection, phase: Phase) -> bool {
        matches!(
            phase,
            Phase::AwaitingTakerConfirmations
                | Phase::TakerLockConfirmed
                | Phase::AwaitingMakerConfirmations
        ) && direction == SwapDirection::TakerSellsLez
    }

    pub(crate) fn validate_active_phase(
        self,
        agreement: &ZecAgreementV1,
        phase: Phase,
    ) -> Result<(), RefundError> {
        let ready = match self {
            Self::Lez => {
                matches!(
                    phase,
                    Phase::BothLegsLocked | Phase::TakerLockReorged | Phase::MakerLockReorged
                ) || Self::one_leg_lez_refund_is_valid(agreement.direction(), phase)
            }
            Self::Zcash => {
                phase == lez_refunded_phase(agreement)
                    || (matches!(
                        phase,
                        Phase::AwaitingTakerConfirmations | Phase::TakerLockConfirmed
                    ) && agreement.direction() == SwapDirection::TakerSellsForeign)
            }
        };
        if ready {
            Ok(())
        } else {
            Err(RefundError::WrongPhase { step: self, phase })
        }
    }

    pub(crate) fn validate_deadline(
        self,
        agreement: &ZecAgreementV1,
        position: ChainPosition,
    ) -> Result<(), RefundError> {
        if position.chain() != self.chain() {
            return Err(RefundError::WrongChain {
                step: self,
                actual: position.chain(),
            });
        }
        let schedule = agreement.coordinator().recovery_schedule();
        let reached = if self.owner(agreement) == Participant::Maker {
            schedule.maker_deadline_reached(position)
        } else {
            schedule.taker_refund_reached(position)
        }
        .map_err(RefundError::Coordinator)?;
        if reached {
            Ok(())
        } else {
            Err(RefundError::DeadlineNotReached(self))
        }
    }
}

/// Phase reached after the agreement's earlier LEZ refund.
#[must_use]
pub(crate) fn lez_refunded_phase(agreement: &ZecAgreementV1) -> Phase {
    if agreement.lez_depositor() == Participant::Maker {
        Phase::MakerLegRefunded
    } else {
        Phase::TakerLegRefunded
    }
}

/// Why an agreement-derived refund input cannot safely be used yet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefundFundingWaitReasonV1 {
    /// The exact funding transaction/output is absent.
    Absent,
    /// The exact funding output is already spent.
    Spent,
    /// A previously canonical funding transaction was reorganized away.
    Reorged,
}

/// Fresh, stable refund eligibility observed immediately before signing/submission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefundEligibilityObservationV1 {
    /// The adapter could not obtain one stable canonical view.
    Unstable,
    /// The exact agreement-derived funding is unavailable.
    FundingUnavailable(RefundFundingWaitReasonV1),
    /// The exact funding is canonical and this is the governing chain position.
    Canonical(ChainPosition),
}

impl RefundEligibilityObservationV1 {
    /// Constructs a canonical eligibility observation.
    #[must_use]
    pub const fn canonical(position: ChainPosition) -> Self {
        Self::Canonical(position)
    }
}

/// Submission result that does not collapse unknown RPC outcomes into rejection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefundSubmitOutcomeV1 {
    /// The node explicitly accepted the exact submission.
    Accepted,
    /// The node definitively rejected the exact submission.
    DefinitivelyRejected,
    /// Transport failed after submission and acceptance is unknown.
    Unknown,
}

/// One public outcome from advancing peer-independent timeout recovery.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefundDriveOutcome {
    /// Exact durable bytes were submitted or byte-identically rebroadcast.
    Submitted(RefundStepV1),
    /// No effect occurred because the chain view was unstable.
    AwaitingStableObservation(RefundStepV1),
    /// No signing/submission occurred because the exact input was unavailable.
    AwaitingFunding {
        /// Refund step being driven.
        step: RefundStepV1,
        /// Fail-closed funding reason.
        reason: RefundFundingWaitReasonV1,
    },
    /// No signing/submission occurred because the typed deadline has not arrived.
    AwaitingDeadline(RefundStepV1),
    /// The node definitively rejected the exact durable submission.
    SubmissionRejected(RefundStepV1),
    /// The submission may have succeeded; restart/retry must observe before rebroadcast.
    SubmissionOutcomeUnknown(RefundStepV1),
    /// Canonical evidence was durably projected.
    Projected {
        /// Projected refund step.
        step: RefundStepV1,
        /// New durable role-local revision.
        revision: u64,
    },
    /// Both refunds were already durably replayed or projected.
    Refunded {
        /// Terminal durable role-local revision.
        revision: u64,
    },
}

/// Exact signed refund bytes and their chain-derived expected identity.
#[derive(Clone, Eq, PartialEq)]
pub struct PreparedRefundSubmissionV1 {
    step: RefundStepV1,
    expected_submission_id: [u8; 32],
    exact_submission: Vec<u8>,
}

impl PreparedRefundSubmissionV1 {
    /// Validates bounded nonempty bytes and a nonzero identity.
    ///
    /// # Errors
    ///
    /// Rejects empty/oversized bytes or an empty expected identity.
    pub fn new(
        step: RefundStepV1,
        expected_submission_id: [u8; 32],
        exact_submission: Vec<u8>,
    ) -> Result<Self, RefundError> {
        if exact_submission.is_empty() {
            return Err(RefundError::EmptySubmission(step));
        }
        if exact_submission.len() > MAX_REFUND_SUBMISSION_BYTES {
            return Err(RefundError::OversizedSubmission {
                step,
                actual: exact_submission.len(),
                maximum: MAX_REFUND_SUBMISSION_BYTES,
            });
        }
        if expected_submission_id == [0; 32] {
            return Err(RefundError::EmptyExpectedIdentity(step));
        }
        Ok(Self {
            step,
            expected_submission_id,
            exact_submission,
        })
    }

    /// Refund step represented by these bytes.
    #[must_use]
    pub const fn step(&self) -> RefundStepV1 {
        self.step
    }

    /// Chain-derived expected submission identity.
    #[must_use]
    pub const fn expected_submission_id(&self) -> &[u8; 32] {
        &self.expected_submission_id
    }

    /// Exact signed bytes to persist before broadcast and replay byte-for-byte.
    #[must_use]
    pub fn exact_submission(&self) -> &[u8] {
        &self.exact_submission
    }
}

impl std::fmt::Debug for PreparedRefundSubmissionV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedRefundSubmissionV1")
            .field("step", &self.step)
            .field("expected_submission_id", &"[REDACTED]")
            .field("exact_submission", &"[REDACTED]")
            .finish()
    }
}

/// Durable owner-only intent containing the exact prepared refund.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefundIntentV1 {
    swap_id: SwapId,
    agreement_commitment: [u8; 32],
    local_participant: Participant,
    staged_revision: u64,
    prepared: PreparedRefundSubmissionV1,
}

impl RefundIntentV1 {
    /// Binds exact prepared bytes to the active agreement, role, step, and revision.
    ///
    /// # Errors
    ///
    /// Rejects wrong role, ordering, phase, or agreement context.
    pub fn from_active(
        agreement: &ZecAgreementV1,
        coordinator: &SwapCoordinator,
        local_participant: Participant,
        staged_revision: u64,
        prepared: PreparedRefundSubmissionV1,
    ) -> Result<Self, RefundError> {
        prepared
            .step
            .validate_active_phase(agreement, coordinator.phase())?;
        let owner = prepared.step.owner(agreement);
        if local_participant != owner {
            return Err(RefundError::WrongRole {
                step: prepared.step,
                expected: owner,
                actual: local_participant,
            });
        }
        Ok(Self {
            swap_id: agreement.coordinator().id().clone(),
            agreement_commitment: *agreement.agreement_commitment(),
            local_participant,
            staged_revision,
            prepared,
        })
    }

    /// Application swap identity.
    #[must_use]
    pub const fn swap_id(&self) -> &SwapId {
        &self.swap_id
    }

    /// Exact agreement commitment.
    #[must_use]
    pub const fn agreement_commitment(&self) -> &[u8; 32] {
        &self.agreement_commitment
    }

    /// Only role permitted to sign and submit this intent.
    #[must_use]
    pub const fn local_participant(&self) -> Participant {
        self.local_participant
    }

    /// Revision at which exact bytes became durable.
    #[must_use]
    pub const fn staged_revision(&self) -> u64 {
        self.staged_revision
    }

    /// Exact durable prepared refund.
    #[must_use]
    pub const fn prepared(&self) -> &PreparedRefundSubmissionV1 {
        &self.prepared
    }

    pub(crate) fn validate_for_active(
        &self,
        agreement: &ZecAgreementV1,
        coordinator: &SwapCoordinator,
        current_revision: u64,
    ) -> Result<(), RefundError> {
        if self.staged_revision > current_revision {
            return Err(RefundError::StagedRevisionAhead {
                staged: self.staged_revision,
                current: current_revision,
            });
        }
        let expected = Self::from_active(
            agreement,
            coordinator,
            self.local_participant,
            self.staged_revision,
            self.prepared.clone(),
        )?;
        if expected == *self {
            Ok(())
        } else {
            Err(RefundError::ContextMismatch)
        }
    }
}

/// Stable canonical refund evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefundEvidenceV1 {
    step: RefundStepV1,
    observed_submission_id: [u8; 32],
    transaction_id: Box<str>,
    position: ChainPosition,
    confirmations: u32,
}

impl RefundEvidenceV1 {
    /// Validates primitive identity, chain position, deadline, and confirmation evidence.
    ///
    /// # Errors
    ///
    /// Rejects empty identity/transaction data, wrong chain/clock, an early deadline, or zero
    /// confirmations.
    pub fn new(
        agreement: &ZecAgreementV1,
        step: RefundStepV1,
        observed_submission_id: [u8; 32],
        transaction_id: impl Into<Box<str>>,
        position: ChainPosition,
        confirmations: u32,
    ) -> Result<Self, RefundError> {
        if observed_submission_id == [0; 32] {
            return Err(RefundError::EmptyExpectedIdentity(step));
        }
        let transaction_id = transaction_id.into();
        if transaction_id.is_empty() || transaction_id.len() > 256 {
            return Err(RefundError::InvalidTransactionId);
        }
        let required_confirmations = agreement
            .coordinator()
            .required_confirmations(step.owner(agreement));
        if confirmations < required_confirmations {
            return Err(RefundError::InsufficientConfirmations {
                step,
                required: required_confirmations,
                actual: confirmations,
            });
        }
        step.validate_deadline(agreement, position)?;
        Ok(Self {
            step,
            observed_submission_id,
            transaction_id,
            position,
            confirmations,
        })
    }

    /// Refund step proven by this observation.
    #[must_use]
    pub const fn step(&self) -> RefundStepV1 {
        self.step
    }

    /// Exact observed submission identity.
    #[must_use]
    pub const fn observed_submission_id(&self) -> &[u8; 32] {
        &self.observed_submission_id
    }

    /// Canonical transaction identifier.
    #[must_use]
    pub const fn transaction_id(&self) -> &str {
        &self.transaction_id
    }

    /// Governing canonical position at observation.
    #[must_use]
    pub const fn position(&self) -> ChainPosition {
        self.position
    }

    /// Canonical confirmation count.
    #[must_use]
    pub const fn confirmations(&self) -> u32 {
        self.confirmations
    }
}

/// Observation of an exact owner-prepared or agreement-derived counterparty refund.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RefundObservationV1 {
    /// Stable absence.
    Absent,
    /// Presence/absence is not stable enough to act on.
    Unstable,
    /// Stable canonical evidence.
    Confirmed(RefundEvidenceV1),
}

/// Atomic durable refund projection, owned or independently observed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefundTransitionV1 {
    swap_id: SwapId,
    agreement_commitment: [u8; 32],
    local_participant: Participant,
    predecessor_revision: u64,
    intent_staged_revision: Option<u64>,
    expected_submission_id: Option<[u8; 32]>,
    evidence: RefundEvidenceV1,
}

impl RefundTransitionV1 {
    /// Builds an owner transition bound to the exact durable intent.
    ///
    /// # Errors
    ///
    /// Rejects substituted identity, wrong role/context, phase, or revision.
    pub fn from_owned(
        agreement: &ZecAgreementV1,
        coordinator: &SwapCoordinator,
        intent: &RefundIntentV1,
        predecessor_revision: u64,
        evidence: RefundEvidenceV1,
    ) -> Result<Self, RefundError> {
        intent.validate_for_active(agreement, coordinator, predecessor_revision)?;
        if intent.prepared.step != evidence.step {
            return Err(RefundError::StepMismatch);
        }
        if intent.prepared.expected_submission_id != evidence.observed_submission_id {
            return Err(RefundError::SubmissionIdentityMismatch(evidence.step));
        }
        Ok(Self {
            swap_id: intent.swap_id.clone(),
            agreement_commitment: intent.agreement_commitment,
            local_participant: intent.local_participant,
            predecessor_revision,
            intent_staged_revision: Some(intent.staged_revision),
            expected_submission_id: Some(intent.prepared.expected_submission_id),
            evidence,
        })
    }

    /// Builds an observation-only transition; the local observer never signs or submits.
    ///
    /// # Errors
    ///
    /// Rejects the owner pretending to be an observer, wrong phase/order, or invalid context.
    pub fn from_observed(
        agreement: &ZecAgreementV1,
        coordinator: &SwapCoordinator,
        local_participant: Participant,
        predecessor_revision: u64,
        evidence: RefundEvidenceV1,
    ) -> Result<Self, RefundError> {
        evidence
            .step
            .validate_active_phase(agreement, coordinator.phase())?;
        let owner = evidence.step.owner(agreement);
        if local_participant == owner {
            return Err(RefundError::WrongObserverRole {
                step: evidence.step,
                owner,
            });
        }
        Ok(Self {
            swap_id: agreement.coordinator().id().clone(),
            agreement_commitment: *agreement.agreement_commitment(),
            local_participant,
            predecessor_revision,
            intent_staged_revision: None,
            expected_submission_id: None,
            evidence,
        })
    }

    /// Application swap identity.
    #[must_use]
    pub const fn swap_id(&self) -> &SwapId {
        &self.swap_id
    }

    pub(crate) const fn agreement_commitment(&self) -> &[u8; 32] {
        &self.agreement_commitment
    }

    /// Role-local predecessor revision occupied by this transition.
    #[must_use]
    pub const fn predecessor_revision(&self) -> u64 {
        self.predecessor_revision
    }

    /// Fixed local role whose journal owns this transition.
    #[must_use]
    pub const fn local_participant(&self) -> Participant {
        self.local_participant
    }

    /// Evidence projected by this transition.
    #[must_use]
    pub const fn evidence(&self) -> &RefundEvidenceV1 {
        &self.evidence
    }

    /// Whether committing this transition must atomically close a local intent.
    #[must_use]
    pub const fn is_owned(&self) -> bool {
        self.intent_staged_revision.is_some()
    }

    pub(crate) const fn intent_staged_revision(&self) -> Option<u64> {
        self.intent_staged_revision
    }

    pub(crate) const fn expected_submission_id(&self) -> Option<&[u8; 32]> {
        self.expected_submission_id.as_ref()
    }

    /// Validates and applies this exact transition to a copy of the coordinator.
    ///
    /// # Errors
    ///
    /// Rejects context/revision/identity drift, illegal ordering, or core transition failure.
    pub fn apply_to(
        &self,
        agreement: &ZecAgreementV1,
        coordinator: &SwapCoordinator,
        current_revision: u64,
    ) -> Result<SwapCoordinator, RefundError> {
        if self.swap_id != *agreement.coordinator().id()
            || self.agreement_commitment != *agreement.agreement_commitment()
            || self.predecessor_revision != current_revision
        {
            return Err(RefundError::ContextMismatch);
        }
        self.evidence
            .step
            .validate_active_phase(agreement, coordinator.phase())?;
        self.evidence
            .step
            .validate_deadline(agreement, self.evidence.position)?;
        if let Some(staged) = self.intent_staged_revision
            && staged > current_revision
        {
            return Err(RefundError::StagedRevisionAhead {
                staged,
                current: current_revision,
            });
        }
        if self
            .expected_submission_id
            .as_ref()
            .is_some_and(|expected| expected != &self.evidence.observed_submission_id)
        {
            return Err(RefundError::SubmissionIdentityMismatch(self.evidence.step));
        }
        let owner = self.evidence.step.owner(agreement);
        if self.intent_staged_revision.is_some() {
            if self.local_participant != owner {
                return Err(RefundError::WrongRole {
                    step: self.evidence.step,
                    expected: owner,
                    actual: self.local_participant,
                });
            }
        } else if self.local_participant == owner {
            return Err(RefundError::WrongObserverRole {
                step: self.evidence.step,
                owner,
            });
        }
        let mut next = coordinator.clone();
        if owner == Participant::Maker {
            next.refund_maker_leg(self.evidence.position)
        } else {
            next.refund_taker_leg(self.evidence.position)
        }
        .map_err(RefundError::Coordinator)?;
        Ok(next)
    }
}

/// Refund contract validation failure.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RefundError {
    /// Prepared submission is empty.
    #[error("{0:?} refund submission is empty")]
    EmptySubmission(RefundStepV1),
    /// Prepared submission exceeds the public bound.
    #[error("{step:?} refund submission has {actual} bytes; maximum is {maximum}")]
    OversizedSubmission {
        /// Refund step.
        step: RefundStepV1,
        /// Actual byte length.
        actual: usize,
        /// Maximum accepted byte length.
        maximum: usize,
    },
    /// Expected or observed chain identity is empty.
    #[error("{0:?} refund identity is empty")]
    EmptyExpectedIdentity(RefundStepV1),
    /// Canonical transaction identifier is empty or oversized.
    #[error("refund transaction identifier is invalid")]
    InvalidTransactionId,
    /// Canonical evidence is below the agreement's role-specific confirmation policy.
    #[error("{step:?} refund has {actual} confirmations; requires {required}")]
    InsufficientConfirmations {
        /// Refund step.
        step: RefundStepV1,
        /// Agreement-required confirmations for the funded role.
        required: u32,
        /// Canonical confirmations observed.
        actual: u32,
    },
    /// Step is not legal in this phase.
    #[error("{step:?} refund is unavailable in phase {phase:?}")]
    WrongPhase {
        /// Refund step.
        step: RefundStepV1,
        /// Current phase.
        phase: Phase,
    },
    /// Evidence uses the wrong chain.
    #[error("{step:?} refund expected {expected:?}, observed {actual:?}", expected = step.chain())]
    WrongChain {
        /// Refund step.
        step: RefundStepV1,
        /// Actual observed chain.
        actual: Chain,
    },
    /// Typed refund deadline has not arrived.
    #[error("{0:?} refund deadline has not arrived")]
    DeadlineNotReached(RefundStepV1),
    /// Local role does not own the refund.
    #[error("{step:?} refund requires {expected:?}; local role is {actual:?}")]
    WrongRole {
        /// Refund step.
        step: RefundStepV1,
        /// Agreement-derived owner.
        expected: Participant,
        /// Fixed local role.
        actual: Participant,
    },
    /// The owner cannot use the observation-only path.
    #[error("{step:?} refund owner {owner:?} cannot commit an observer transition")]
    WrongObserverRole {
        /// Refund step.
        step: RefundStepV1,
        /// Agreement-derived owner.
        owner: Participant,
    },
    /// Loaded intent is from a future coordinator revision.
    #[error("refund intent revision {staged} is ahead of current revision {current}")]
    StagedRevisionAhead {
        /// Durable intent revision.
        staged: u64,
        /// Current revision.
        current: u64,
    },
    /// Agreement, role, or revision context changed.
    #[error("refund recovery context does not match the active agreement")]
    ContextMismatch,
    /// Intent and evidence steps differ.
    #[error("refund intent and evidence steps differ")]
    StepMismatch,
    /// Observed transaction identity differs from the exact durable intent.
    #[error("observed {0:?} refund identity does not match the durable submission")]
    SubmissionIdentityMismatch(RefundStepV1),
    /// Core coordinator rejected the typed transition.
    #[error(transparent)]
    Coordinator(#[from] lez_swap_core::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn awaiting_maker_confirmations_only_admits_the_earlier_taker_owned_lez_refund() {
        assert!(RefundStepV1::one_leg_lez_refund_is_valid(
            SwapDirection::TakerSellsLez,
            Phase::AwaitingMakerConfirmations,
        ));
        assert!(!RefundStepV1::one_leg_lez_refund_is_valid(
            SwapDirection::TakerSellsForeign,
            Phase::AwaitingMakerConfirmations,
        ));
        assert!(!RefundStepV1::one_leg_lez_refund_is_valid(
            SwapDirection::TakerSellsForeign,
            Phase::TakerLockConfirmed,
        ));
    }

    #[test]
    fn prepared_refund_is_bounded_and_redacted() {
        assert!(matches!(
            PreparedRefundSubmissionV1::new(RefundStepV1::Lez, [1; 32], Vec::new()),
            Err(RefundError::EmptySubmission(RefundStepV1::Lez))
        ));
        let prepared = PreparedRefundSubmissionV1::new(
            RefundStepV1::Zcash,
            [2; 32],
            b"signed-refund".to_vec(),
        )
        .expect("valid refund");
        let debug = format!("{prepared:?}");
        assert!(!debug.contains("signed-refund"));
        assert!(debug.contains("[REDACTED]"));
    }
}
