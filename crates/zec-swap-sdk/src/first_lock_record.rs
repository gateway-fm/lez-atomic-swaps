//! Primitive durable DTOs for first-lock intents and committed transitions.
//!
//! These records are deliberately untrusted after deserialization. They never deserialize
//! directly into lifecycle domain types; callers must revalidate every field against the
//! accepted agreement and current durable revision.

use lez_swap_core::{Participant, SwapId};
use serde::{Deserialize, Serialize};

use crate::{
    AcceptedZecAgreementV1, FirstLockConfirmedEvidenceV1, FirstLockIntentError, FirstLockIntentV1,
    FirstLockPlanV1, FirstLockStepV1, FirstLockTransitionError, FirstLockTransitionV1,
    PreparedFirstLockSubmissionV1,
};

/// Stable payload version stored beside schema-v5 SDK recovery JSON.
pub const FIRST_LOCK_RECORD_SCHEMA_V1: u16 = 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FirstLockSubmissionRecordV1 {
    step: Box<str>,
    expected_submission_id: [u8; 32],
    exact_submission: Vec<u8>,
}

impl From<&PreparedFirstLockSubmissionV1> for FirstLockSubmissionRecordV1 {
    fn from(value: &PreparedFirstLockSubmissionV1) -> Self {
        Self {
            step: step_name(value.step()).into(),
            expected_submission_id: *value.expected_submission_id(),
            exact_submission: value.exact_submission().to_vec(),
        }
    }
}

impl FirstLockSubmissionRecordV1 {
    pub(crate) fn revalidate(&self) -> Result<PreparedFirstLockSubmissionV1, FirstLockRecordError> {
        PreparedFirstLockSubmissionV1::new(
            parse_step(&self.step)?,
            self.expected_submission_id,
            self.exact_submission.clone(),
        )
        .map_err(FirstLockRecordError::Intent)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "plan", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum FirstLockPlanRecordV1 {
    Zcash {
        funding: FirstLockSubmissionRecordV1,
    },
    Lez {
        initialize: FirstLockSubmissionRecordV1,
        fund: FirstLockSubmissionRecordV1,
    },
}

impl From<&FirstLockPlanV1> for FirstLockPlanRecordV1 {
    fn from(value: &FirstLockPlanV1) -> Self {
        match value {
            FirstLockPlanV1::Zcash { funding } => Self::Zcash {
                funding: funding.into(),
            },
            FirstLockPlanV1::Lez { initialize, fund } => Self::Lez {
                initialize: initialize.into(),
                fund: fund.into(),
            },
        }
    }
}

impl FirstLockPlanRecordV1 {
    pub(crate) fn revalidate(&self) -> Result<FirstLockPlanV1, FirstLockRecordError> {
        match self {
            Self::Zcash { funding } => {
                FirstLockPlanV1::zcash(funding.revalidate()?).map_err(FirstLockRecordError::Intent)
            }
            Self::Lez { initialize, fund } => {
                FirstLockPlanV1::lez(initialize.revalidate()?, fund.revalidate()?)
                    .map_err(FirstLockRecordError::Intent)
            }
        }
    }
}

/// Primitive version-1 persistence payload for an immutable first-lock intent.
///
/// Deserialization does not produce a trusted effect. Call revalidate with the independently
/// resumed accepted agreement and aggregate predecessor revision.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FirstLockIntentRecordV1 {
    schema_version: u16,
    swap_id: Box<str>,
    agreement_commitment: [u8; 32],
    local_participant: Box<str>,
    predecessor_revision: u64,
    plan: FirstLockPlanRecordV1,
}

impl From<&FirstLockIntentV1> for FirstLockIntentRecordV1 {
    fn from(value: &FirstLockIntentV1) -> Self {
        Self {
            schema_version: FIRST_LOCK_RECORD_SCHEMA_V1,
            swap_id: value.swap_id().as_str().into(),
            agreement_commitment: *value.agreement_commitment(),
            local_participant: participant_name(value.local_participant()).into(),
            predecessor_revision: value.predecessor_revision(),
            plan: value.plan().into(),
        }
    }
}

impl FirstLockIntentRecordV1 {
    /// Primitive payload version for the enclosing database row.
    #[must_use]
    pub const fn schema_version(&self) -> u16 {
        self.schema_version
    }

    /// Rebuilds a trusted intent only after exact agreement, role, revision, plan, step, identity,
    /// and byte-bound validation.
    ///
    /// # Errors
    ///
    /// Rejects future schemas, malformed primitive identifiers, a different accepted context, or
    /// any invalid/oversized chain-specific submission plan.
    pub fn revalidate(
        &self,
        accepted: &AcceptedZecAgreementV1,
        predecessor_revision: u64,
    ) -> Result<FirstLockIntentV1, FirstLockRecordError> {
        require_schema("first-lock intent", self.schema_version)?;
        let swap_id = SwapId::new(self.swap_id.clone()).map_err(FirstLockRecordError::Core)?;
        if &swap_id != accepted.agreement().coordinator().id() {
            return Err(FirstLockRecordError::SwapIdMismatch);
        }
        if self.agreement_commitment != *accepted.agreement().agreement_commitment() {
            return Err(FirstLockRecordError::AgreementCommitmentMismatch);
        }
        let local_participant = parse_participant(&self.local_participant)?;
        if local_participant != accepted.local_participant() {
            return Err(FirstLockRecordError::RoleMismatch);
        }
        if self.predecessor_revision != predecessor_revision {
            return Err(FirstLockRecordError::RevisionMismatch);
        }
        let trusted = FirstLockIntentV1::from_active(
            accepted.agreement(),
            local_participant,
            predecessor_revision,
            self.plan.revalidate()?,
        )
        .map_err(FirstLockRecordError::Intent)?;
        if Self::from(&trusted) != *self {
            return Err(FirstLockRecordError::IntentMismatch);
        }
        Ok(trusted)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FirstLockEvidenceRecordV1 {
    schema_version: u16,
    step: Box<str>,
    expected_submission_id: [u8; 32],
    transaction_id: Box<str>,
    confirmations: u32,
}

impl From<&FirstLockConfirmedEvidenceV1> for FirstLockEvidenceRecordV1 {
    fn from(value: &FirstLockConfirmedEvidenceV1) -> Self {
        Self {
            schema_version: value.schema_version(),
            step: step_name(value.step()).into(),
            expected_submission_id: *value.expected_submission_id(),
            transaction_id: value.transaction_id().into(),
            confirmations: value.confirmations(),
        }
    }
}

impl FirstLockEvidenceRecordV1 {
    pub(crate) fn revalidate(&self) -> Result<FirstLockConfirmedEvidenceV1, FirstLockRecordError> {
        require_schema("first-lock evidence", self.schema_version)?;
        FirstLockConfirmedEvidenceV1::new(
            parse_step(&self.step)?,
            self.expected_submission_id,
            self.transaction_id.clone(),
            self.confirmations,
        )
        .map_err(FirstLockRecordError::Transition)
    }
}

/// Primitive version-1 payload for an atomically committed first-lock transition.
///
/// The separately retained, closed intent remains part of the trust boundary: replay only
/// succeeds when that intent also revalidates exactly against the accepted agreement.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FirstLockTransitionRecordV1 {
    schema_version: u16,
    swap_id: Box<str>,
    agreement_commitment: [u8; 32],
    local_participant: Box<str>,
    predecessor_revision: u64,
    evidence: FirstLockEvidenceRecordV1,
}

impl From<&FirstLockTransitionV1> for FirstLockTransitionRecordV1 {
    fn from(value: &FirstLockTransitionV1) -> Self {
        Self {
            schema_version: value.schema_version(),
            swap_id: value.swap_id().as_str().into(),
            agreement_commitment: *value.agreement_commitment(),
            local_participant: participant_name(value.local_participant()).into(),
            predecessor_revision: value.predecessor_revision(),
            evidence: value.evidence().into(),
        }
    }
}

impl FirstLockTransitionRecordV1 {
    /// Primitive payload version for the enclosing database row.
    #[must_use]
    pub const fn schema_version(&self) -> u16 {
        self.schema_version
    }

    /// Rebuilds a trusted transition from primitive storage fields and its exact closed intent.
    ///
    /// # Errors
    ///
    /// Rejects future schemas; malformed or mismatched swap, role, commitment, or revision
    /// fields; corrupt closed intent material; a non-final/wrong-chain step or identity; and
    /// evidence below the signed confirmation policy.
    pub fn revalidate(
        &self,
        accepted: &AcceptedZecAgreementV1,
        retained_closed_intent: &FirstLockIntentRecordV1,
        predecessor_revision: u64,
    ) -> Result<FirstLockTransitionV1, FirstLockRecordError> {
        require_schema("first-lock transition", self.schema_version)?;
        let swap_id = SwapId::new(self.swap_id.clone()).map_err(FirstLockRecordError::Core)?;
        if &swap_id != accepted.agreement().coordinator().id() {
            return Err(FirstLockRecordError::SwapIdMismatch);
        }
        if self.agreement_commitment != *accepted.agreement().agreement_commitment() {
            return Err(FirstLockRecordError::AgreementCommitmentMismatch);
        }
        let local_participant = parse_participant(&self.local_participant)?;
        if local_participant != accepted.local_participant() {
            return Err(FirstLockRecordError::RoleMismatch);
        }
        if self.predecessor_revision != predecessor_revision {
            return Err(FirstLockRecordError::RevisionMismatch);
        }
        let intent = retained_closed_intent
            .revalidate(accepted, predecessor_revision)
            .map_err(|error| FirstLockRecordError::ClosedIntent(Box::new(error)))?;
        let trusted = FirstLockTransitionV1::from_active(
            accepted.agreement(),
            &intent,
            predecessor_revision,
            self.evidence.revalidate()?,
        )
        .map_err(FirstLockRecordError::Transition)?;
        if Self::from(&trusted) != *self {
            return Err(FirstLockRecordError::TransitionMismatch);
        }
        Ok(trusted)
    }
}

/// Failure to reconstruct a trusted first-lock domain value from primitive durable fields.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum FirstLockRecordError {
    /// A durable payload uses an unsupported schema.
    #[error("unsupported {record} schema {actual}")]
    UnsupportedSchema {
        /// Primitive record kind.
        record: &'static str,
        /// Untrusted schema number.
        actual: u16,
    },
    /// A role field is not one of the stable primitive spellings.
    #[error("invalid first-lock participant role {0}")]
    InvalidRole(Box<str>),
    /// A step field is not one of the stable primitive spellings.
    #[error("invalid first-lock step {0}")]
    InvalidStep(Box<str>),
    /// The swap ID differs from the independently accepted agreement.
    #[error("first-lock record swap ID mismatch")]
    SwapIdMismatch,
    /// The agreement commitment differs from the independently accepted agreement.
    #[error("first-lock record agreement commitment mismatch")]
    AgreementCommitmentMismatch,
    /// The role differs from the independently accepted local role.
    #[error("first-lock record local role mismatch")]
    RoleMismatch,
    /// The predecessor revision differs from the independently loaded aggregate revision.
    #[error("first-lock record predecessor revision mismatch")]
    RevisionMismatch,
    /// Reconstructed intent is not byte-for-byte equivalent to its primitive record.
    #[error("first-lock intent record is not canonical")]
    IntentMismatch,
    /// Reconstructed transition is not byte-for-byte equivalent to its primitive record.
    #[error("first-lock transition record is not canonical")]
    TransitionMismatch,
    /// The separately retained closed intent is corrupt or context-substituted.
    #[error("retained closed first-lock intent is invalid: {0}")]
    ClosedIntent(Box<FirstLockRecordError>),
    /// Intent shape or bounded submission validation failed.
    #[error(transparent)]
    Intent(FirstLockIntentError),
    /// Final-step evidence or transition validation failed.
    #[error(transparent)]
    Transition(FirstLockTransitionError),
    /// A primitive identifier violates core bounds.
    #[error(transparent)]
    Core(lez_swap_core::Error),
}

pub(crate) fn require_schema(
    record: &'static str,
    actual: u16,
) -> Result<(), FirstLockRecordError> {
    if actual == FIRST_LOCK_RECORD_SCHEMA_V1 {
        Ok(())
    } else {
        Err(FirstLockRecordError::UnsupportedSchema { record, actual })
    }
}

pub(crate) const fn participant_name(participant: Participant) -> &'static str {
    match participant {
        Participant::Maker => "maker",
        Participant::Taker => "taker",
    }
}

pub(crate) fn parse_participant(value: &str) -> Result<Participant, FirstLockRecordError> {
    match value {
        "maker" => Ok(Participant::Maker),
        "taker" => Ok(Participant::Taker),
        _ => Err(FirstLockRecordError::InvalidRole(value.into())),
    }
}

const fn step_name(step: FirstLockStepV1) -> &'static str {
    match step {
        FirstLockStepV1::ZcashFund => "zcash_fund",
        FirstLockStepV1::LezInitialize => "lez_initialize",
        FirstLockStepV1::LezFund => "lez_fund",
    }
}

fn parse_step(value: &str) -> Result<FirstLockStepV1, FirstLockRecordError> {
    match value {
        "zcash_fund" => Ok(FirstLockStepV1::ZcashFund),
        "lez_initialize" => Ok(FirstLockStepV1::LezInitialize),
        "lez_fund" => Ok(FirstLockStepV1::LezFund),
        _ => Err(FirstLockRecordError::InvalidStep(value.into())),
    }
}
