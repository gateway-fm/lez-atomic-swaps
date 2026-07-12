//! Primitive durable DTOs for maker second-lock intents and transitions.

use lez_swap_core::{Participant, SwapId};
use serde::{Deserialize, Serialize};

use crate::{
    AcceptedZecAgreementV1, FirstLockRecordError, MakerLockError, MakerLockIntentV1,
    MakerLockTransitionV1,
    first_lock_record::{
        FirstLockEvidenceRecordV1, FirstLockPlanRecordV1, parse_participant, participant_name,
    },
};

/// Stable payload version for schema-v7 maker-lock rows.
pub const MAKER_LOCK_RECORD_SCHEMA_V1: u16 = 1;

/// Primitive payload for an immutable maker second-lock intent.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MakerLockIntentRecordV1 {
    schema_version: u16,
    swap_id: Box<str>,
    agreement_commitment: [u8; 32],
    local_participant: Box<str>,
    staged_revision: u64,
    plan: FirstLockPlanRecordV1,
}

impl From<&MakerLockIntentV1> for MakerLockIntentRecordV1 {
    fn from(value: &MakerLockIntentV1) -> Self {
        Self {
            schema_version: MAKER_LOCK_RECORD_SCHEMA_V1,
            swap_id: value.swap_id().as_str().into(),
            agreement_commitment: *value.agreement_commitment(),
            local_participant: participant_name(value.local_participant()).into(),
            staged_revision: value.staged_revision(),
            plan: value.plan().into(),
        }
    }
}

impl MakerLockIntentRecordV1 {
    /// Primitive payload schema.
    #[must_use]
    pub const fn schema_version(&self) -> u16 {
        self.schema_version
    }

    /// Rebuilds a trusted intent against the independently accepted agreement.
    ///
    /// # Errors
    ///
    /// Rejects unknown schemas, malformed context fields, non-maker roles, or
    /// an invalid opposite-chain plan.
    pub fn revalidate(
        &self,
        accepted: &AcceptedZecAgreementV1,
    ) -> Result<MakerLockIntentV1, MakerLockRecordError> {
        require_schema("maker-lock intent", self.schema_version)?;
        let swap_id = SwapId::new(self.swap_id.clone()).map_err(MakerLockRecordError::Core)?;
        if &swap_id != accepted.agreement().coordinator().id() {
            return Err(MakerLockRecordError::SwapIdMismatch);
        }
        if self.agreement_commitment != *accepted.agreement().agreement_commitment() {
            return Err(MakerLockRecordError::AgreementCommitmentMismatch);
        }
        let local_participant =
            parse_participant(&self.local_participant).map_err(MakerLockRecordError::FirstLock)?;
        if local_participant != Participant::Maker
            || local_participant != accepted.local_participant()
        {
            return Err(MakerLockRecordError::RoleMismatch);
        }
        let trusted = MakerLockIntentV1::from_active(
            accepted.agreement(),
            local_participant,
            self.staged_revision,
            self.plan
                .revalidate()
                .map_err(MakerLockRecordError::FirstLock)?,
        )?;
        if Self::from(&trusted) != *self {
            return Err(MakerLockRecordError::IntentMismatch);
        }
        Ok(trusted)
    }
}

/// Primitive payload for an atomically committed maker funding transition.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MakerLockTransitionRecordV1 {
    schema_version: u16,
    swap_id: Box<str>,
    agreement_commitment: [u8; 32],
    local_participant: Box<str>,
    predecessor_revision: u64,
    intent_staged_revision: u64,
    evidence: FirstLockEvidenceRecordV1,
}

impl From<&MakerLockTransitionV1> for MakerLockTransitionRecordV1 {
    fn from(value: &MakerLockTransitionV1) -> Self {
        Self {
            schema_version: value.schema_version(),
            swap_id: value.swap_id().as_str().into(),
            agreement_commitment: *value.agreement_commitment(),
            local_participant: participant_name(value.local_participant()).into(),
            predecessor_revision: value.predecessor_revision(),
            intent_staged_revision: value.intent_staged_revision(),
            evidence: value.evidence().into(),
        }
    }
}

impl MakerLockTransitionRecordV1 {
    /// Primitive payload schema.
    #[must_use]
    pub const fn schema_version(&self) -> u16 {
        self.schema_version
    }

    /// Rebuilds a trusted transition from its exact retained intent.
    ///
    /// # Errors
    ///
    /// Rejects malformed context, a substituted intent, a future staged revision,
    /// or evidence inconsistent with the maker plan and signed confirmation policy.
    pub fn revalidate(
        &self,
        accepted: &AcceptedZecAgreementV1,
        retained_intent: &MakerLockIntentRecordV1,
        predecessor_revision: u64,
    ) -> Result<MakerLockTransitionV1, MakerLockRecordError> {
        require_schema("maker-lock transition", self.schema_version)?;
        let swap_id = SwapId::new(self.swap_id.clone()).map_err(MakerLockRecordError::Core)?;
        if &swap_id != accepted.agreement().coordinator().id() {
            return Err(MakerLockRecordError::SwapIdMismatch);
        }
        if self.agreement_commitment != *accepted.agreement().agreement_commitment() {
            return Err(MakerLockRecordError::AgreementCommitmentMismatch);
        }
        let role =
            parse_participant(&self.local_participant).map_err(MakerLockRecordError::FirstLock)?;
        if role != Participant::Maker || role != accepted.local_participant() {
            return Err(MakerLockRecordError::RoleMismatch);
        }
        if self.predecessor_revision != predecessor_revision
            || self.intent_staged_revision > predecessor_revision
        {
            return Err(MakerLockRecordError::RevisionMismatch);
        }
        let intent = retained_intent.revalidate(accepted)?;
        if intent.staged_revision() != self.intent_staged_revision {
            return Err(MakerLockRecordError::RevisionMismatch);
        }
        let trusted = MakerLockTransitionV1::from_active(
            accepted.agreement(),
            &intent,
            predecessor_revision,
            self.evidence
                .revalidate()
                .map_err(MakerLockRecordError::FirstLock)?,
        )?;
        if Self::from(&trusted) != *self {
            return Err(MakerLockRecordError::TransitionMismatch);
        }
        Ok(trusted)
    }
}

/// Failure to reconstruct trusted maker-lock domain values from primitive rows.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum MakerLockRecordError {
    /// Payload schema is unsupported.
    #[error("unsupported {record} schema {actual}")]
    UnsupportedSchema {
        /// Record family.
        record: &'static str,
        /// Untrusted schema number.
        actual: u16,
    },
    /// Swap ID differs from the accepted agreement.
    #[error("maker-lock record swap ID mismatch")]
    SwapIdMismatch,
    /// Agreement commitment differs from the accepted agreement.
    #[error("maker-lock record agreement commitment mismatch")]
    AgreementCommitmentMismatch,
    /// Durable role is not the independently accepted maker role.
    #[error("maker-lock record local role mismatch")]
    RoleMismatch,
    /// Staging, predecessor, or intent-link revision is inconsistent.
    #[error("maker-lock record revision mismatch")]
    RevisionMismatch,
    /// Reconstructed intent is not canonical.
    #[error("maker-lock intent record is not canonical")]
    IntentMismatch,
    /// Reconstructed transition is not canonical.
    #[error("maker-lock transition record is not canonical")]
    TransitionMismatch,
    /// Shared first-lock primitive decoding failed.
    #[error(transparent)]
    FirstLock(FirstLockRecordError),
    /// Maker-lock domain validation failed.
    #[error(transparent)]
    Maker(#[from] MakerLockError),
    /// Primitive swap identity violates core bounds.
    #[error(transparent)]
    Core(lez_swap_core::Error),
}

fn require_schema(record: &'static str, actual: u16) -> Result<(), MakerLockRecordError> {
    if actual == MAKER_LOCK_RECORD_SCHEMA_V1 {
        Ok(())
    } else {
        Err(MakerLockRecordError::UnsupportedSchema { record, actual })
    }
}
