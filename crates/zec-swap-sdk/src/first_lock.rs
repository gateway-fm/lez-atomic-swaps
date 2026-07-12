//! Versioned, bounded first-lock intent records staged before any node call.

use lez_swap_core::{Participant, SwapDirection, SwapId};

use crate::ZecAgreementV1;

/// Maximum exact signed submission retained in one first-lock step.
pub const MAX_FIRST_LOCK_SUBMISSION_BYTES: usize = 2_000_000;

const FIRST_LOCK_EFFECT_V1_DOMAIN: &[u8] = b"logos.gateway.lez-zec.first-lock-effect.v1\0";

/// One independently recoverable first-lock submission.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FirstLockStepV1 {
    /// Submit the exact signed Zcash transparent funding transaction.
    ZcashFund,
    /// Submit the exact signed LEZ escrow-initialize transaction.
    LezInitialize,
    /// Submit the exact signed LEZ escrow-funding transaction after initialization is observed.
    LezFund,
}

impl FirstLockStepV1 {
    const fn tag(self) -> u8 {
        match self {
            Self::ZcashFund => 0,
            Self::LezInitialize => 1,
            Self::LezFund => 2,
        }
    }
}

/// Exact signed bytes and the chain-derived identity expected after submission.
#[derive(Clone, Eq, PartialEq)]
pub struct PreparedFirstLockSubmissionV1 {
    step: FirstLockStepV1,
    expected_submission_id: [u8; 32],
    exact_submission: Vec<u8>,
}

impl PreparedFirstLockSubmissionV1 {
    /// Validates a bounded, nonempty exact submission and nonzero expected identity.
    ///
    /// # Errors
    ///
    /// Rejects empty/oversized bytes or an empty expected identity before persistence.
    pub fn new(
        step: FirstLockStepV1,
        expected_submission_id: [u8; 32],
        exact_submission: Vec<u8>,
    ) -> Result<Self, FirstLockIntentError> {
        if exact_submission.is_empty() {
            return Err(FirstLockIntentError::EmptySubmission(step));
        }
        if exact_submission.len() > MAX_FIRST_LOCK_SUBMISSION_BYTES {
            return Err(FirstLockIntentError::OversizedSubmission {
                step,
                actual: exact_submission.len(),
                maximum: MAX_FIRST_LOCK_SUBMISSION_BYTES,
            });
        }
        if expected_submission_id == [0; 32] {
            return Err(FirstLockIntentError::EmptyExpectedIdentity(step));
        }
        Ok(Self {
            step,
            expected_submission_id,
            exact_submission,
        })
    }

    /// Independently recoverable action kind.
    #[must_use]
    pub const fn step(&self) -> FirstLockStepV1 {
        self.step
    }

    /// Identity that an observation adapter must find before treating the submission as present.
    #[must_use]
    pub const fn expected_submission_id(&self) -> &[u8; 32] {
        &self.expected_submission_id
    }

    /// Exact signed bytes that may be rebroadcast byte-for-byte after an unknown outcome.
    #[must_use]
    pub fn exact_submission(&self) -> &[u8] {
        &self.exact_submission
    }
}

impl std::fmt::Debug for PreparedFirstLockSubmissionV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedFirstLockSubmissionV1")
            .field("step", &self.step)
            .field("expected_submission_id", &"[REDACTED]")
            .field("exact_submission", &"[REDACTED]")
            .finish()
    }
}

/// Complete first-lock recovery plan for the chain fixed by the signed direction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FirstLockPlanV1 {
    /// One exact Zcash funding transaction.
    Zcash {
        /// Prepared funding submission.
        funding: PreparedFirstLockSubmissionV1,
    },
    /// Separate LEZ initialize and fund submissions, both durable before either node call.
    Lez {
        /// Prepared escrow initialization.
        initialize: PreparedFirstLockSubmissionV1,
        /// Prepared funding transaction, submitted only after initialization is observed.
        fund: PreparedFirstLockSubmissionV1,
    },
}

impl FirstLockPlanV1 {
    /// Builds the only valid Zcash plan shape.
    ///
    /// # Errors
    ///
    /// Rejects any step other than [`FirstLockStepV1::ZcashFund`].
    pub fn zcash(funding: PreparedFirstLockSubmissionV1) -> Result<Self, FirstLockIntentError> {
        require_step(&funding, FirstLockStepV1::ZcashFund)?;
        Ok(Self::Zcash { funding })
    }

    /// Builds the ordered LEZ initialize/fund plan shape.
    ///
    /// # Errors
    ///
    /// Rejects wrong step kinds or aliased expected transaction identities.
    pub fn lez(
        initialize: PreparedFirstLockSubmissionV1,
        fund: PreparedFirstLockSubmissionV1,
    ) -> Result<Self, FirstLockIntentError> {
        require_step(&initialize, FirstLockStepV1::LezInitialize)?;
        require_step(&fund, FirstLockStepV1::LezFund)?;
        if initialize.expected_submission_id == fund.expected_submission_id {
            return Err(FirstLockIntentError::AliasedLezSubmissionIdentity);
        }
        Ok(Self::Lez { initialize, fund })
    }
}

/// Immutable, role-local record that a store must create before any first-lock node call.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FirstLockIntentV1 {
    swap_id: SwapId,
    agreement_commitment: [u8; 32],
    local_participant: Participant,
    predecessor_revision: u64,
    plan: FirstLockPlanV1,
}

impl FirstLockIntentV1 {
    pub(crate) fn from_active(
        agreement: &ZecAgreementV1,
        local_participant: Participant,
        predecessor_revision: u64,
        plan: FirstLockPlanV1,
    ) -> Result<Self, FirstLockIntentError> {
        if local_participant != Participant::Taker {
            return Err(FirstLockIntentError::WrongRole(local_participant));
        }
        match (agreement.direction(), &plan) {
            (SwapDirection::TakerSellsForeign, FirstLockPlanV1::Zcash { .. })
            | (SwapDirection::TakerSellsLez, FirstLockPlanV1::Lez { .. }) => {}
            (direction, _) => return Err(FirstLockIntentError::WrongPlanForDirection(direction)),
        }
        Ok(Self {
            swap_id: agreement.coordinator().id().clone(),
            agreement_commitment: *agreement.agreement_commitment(),
            local_participant,
            predecessor_revision,
            plan,
        })
    }

    /// Application swap identity derived from the signed agreement.
    #[must_use]
    pub const fn swap_id(&self) -> &SwapId {
        &self.swap_id
    }

    /// Commitment to the exact dual-signed executable agreement.
    #[must_use]
    pub const fn agreement_commitment(&self) -> &[u8; 32] {
        &self.agreement_commitment
    }

    /// Fixed role that owns this independent recovery record.
    #[must_use]
    pub const fn local_participant(&self) -> Participant {
        self.local_participant
    }

    /// Durable agreement revision that must precede this effect.
    #[must_use]
    pub const fn predecessor_revision(&self) -> u64 {
        self.predecessor_revision
    }

    /// Exact chain-specific submission plan.
    #[must_use]
    pub const fn plan(&self) -> &FirstLockPlanV1 {
        &self.plan
    }

    /// Stable step identity independent of submission bytes, for exact-retry conflict checks.
    #[must_use]
    pub fn effect_id(&self, step: FirstLockStepV1) -> [u8; 32] {
        use sha2::{Digest, Sha256};

        let mut hasher = Sha256::new();
        hasher.update(FIRST_LOCK_EFFECT_V1_DOMAIN);
        hasher.update(self.agreement_commitment);
        hasher.update([match self.local_participant {
            Participant::Maker => 0,
            Participant::Taker => 1,
        }]);
        hasher.update([step.tag()]);
        hasher.finalize().into()
    }

    pub(crate) fn validate_for_active(
        &self,
        agreement: &ZecAgreementV1,
        local_participant: Participant,
        predecessor_revision: u64,
    ) -> Result<(), FirstLockIntentError> {
        let expected = Self::from_active(
            agreement,
            local_participant,
            predecessor_revision,
            self.plan.clone(),
        )?;
        if &expected == self {
            Ok(())
        } else {
            Err(FirstLockIntentError::DurableIntentMismatch)
        }
    }
}

/// Atomic outcome of staging one immutable first-lock plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CreateFirstLockOutcome {
    /// No plan existed and the exact intent is now durable.
    Created,
    /// The same plan was already durable; retry is idempotent.
    ExistingSame,
    /// The same role-local swap key contains different first-lock material.
    Conflict,
}

/// Stable observation result returned by a chain-specific adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FirstLockObservation {
    /// The expected chain identity is absent from a stable fresh query.
    Absent,
    /// The query cannot yet prove stable presence or stable absence.
    Unstable,
    /// The exact expected submission is canonical at the agreement's required depth.
    Confirmed,
}

/// One safe outcome of driving a previously durable first-lock plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FirstLockDriveOutcome {
    /// Exact durable bytes were submitted or rebroadcast for this step.
    Submitted(FirstLockStepV1),
    /// No bytes were submitted because the observation was unstable.
    AwaitingStableObservation(FirstLockStepV1),
    /// Every first-lock step is observed; a later atomic evidence projection may advance core.
    ReadyForFundingProjection,
}

/// Invalid first-lock recovery material or agreement binding.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum FirstLockIntentError {
    /// Signed submission bytes must not be empty.
    #[error("{0:?} exact submission bytes are empty")]
    EmptySubmission(FirstLockStepV1),
    /// Signed submission bytes exceed the fixed network/storage boundary.
    #[error("{step:?} submission has {actual} bytes; maximum is {maximum}")]
    OversizedSubmission {
        /// Independently recoverable step.
        step: FirstLockStepV1,
        /// Observed byte length.
        actual: usize,
        /// Fixed maximum byte length.
        maximum: usize,
    },
    /// The expected chain identity must be concrete.
    #[error("{0:?} expected submission identity is empty")]
    EmptyExpectedIdentity(FirstLockStepV1),
    /// A plan slot contains a different action kind.
    #[error("first-lock plan expected {expected:?}, got {actual:?}")]
    WrongStep {
        /// Required slot kind.
        expected: FirstLockStepV1,
        /// Supplied kind.
        actual: FirstLockStepV1,
    },
    /// LEZ initialization and funding must have distinct chain identities.
    #[error("LEZ initialize and fund submissions use the same expected identity")]
    AliasedLezSubmissionIdentity,
    /// Only the role-fixed taker may create the first lock from `Offered`.
    #[error("first lock requires Taker; local participant is {0:?}")]
    WrongRole(Participant),
    /// The plan's chain disagrees with the signed trade direction.
    #[error("first-lock plan does not match signed direction {0:?}")]
    WrongPlanForDirection(SwapDirection),
    /// A loaded record does not exactly match this accepted agreement, role, and revision.
    #[error("durable first-lock intent does not match the active agreement context")]
    DurableIntentMismatch,
}

fn require_step(
    submission: &PreparedFirstLockSubmissionV1,
    expected: FirstLockStepV1,
) -> Result<(), FirstLockIntentError> {
    if submission.step == expected {
        Ok(())
    } else {
        Err(FirstLockIntentError::WrongStep {
            expected,
            actual: submission.step,
        })
    }
}
