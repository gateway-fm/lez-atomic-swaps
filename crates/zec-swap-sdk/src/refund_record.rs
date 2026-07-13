//! Primitive durable DTOs for refund intents and committed transitions.
//!
//! Deserialized records are untrusted. Callers must revalidate them against the independently
//! resumed accepted agreement, aggregate coordinator, and exact predecessor revision before use.

use lez_swap_core::{Chain, ChainPosition, ClockBasis, SwapCoordinator};
use serde::{Deserialize, Serialize};

use crate::{
    AcceptedZecAgreementV1, PreparedRefundSubmissionV1, RefundError, RefundEvidenceV1,
    RefundIntentV1, RefundStepV1, RefundTransitionV1,
    first_lock_record::{parse_participant, participant_name},
};

/// Stable payload version for refund intent and transition records.
pub const REFUND_RECORD_SCHEMA_V1: u16 = 1;

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct PreparedRefundRecordV1 {
    step: Box<str>,
    expected_submission_id: [u8; 32],
    exact_submission: Vec<u8>,
}

impl From<&PreparedRefundSubmissionV1> for PreparedRefundRecordV1 {
    fn from(value: &PreparedRefundSubmissionV1) -> Self {
        Self {
            step: step_name(value.step()).into(),
            expected_submission_id: *value.expected_submission_id(),
            exact_submission: value.exact_submission().to_vec(),
        }
    }
}

impl PreparedRefundRecordV1 {
    fn revalidate(&self) -> Result<PreparedRefundSubmissionV1, RefundRecordError> {
        PreparedRefundSubmissionV1::new(
            parse_step(&self.step)?,
            self.expected_submission_id,
            self.exact_submission.clone(),
        )
        .map_err(RefundRecordError::Refund)
    }
}

impl std::fmt::Debug for PreparedRefundRecordV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedRefundRecordV1")
            .field("step", &self.step)
            .field("expected_submission_id", &"[REDACTED]")
            .field("exact_submission", &"[REDACTED]")
            .finish()
    }
}

/// Versioned primitive record containing exact owner refund bytes.
///
/// The store must create this record before broadcast. When the owner transition commits, copy
/// this exact record into the committed transition row before deleting the pending intent.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RefundIntentRecordV1 {
    schema_version: u16,
    swap_id: Box<str>,
    agreement_commitment: [u8; 32],
    local_participant: Box<str>,
    staged_revision: u64,
    prepared: PreparedRefundRecordV1,
}

impl From<&RefundIntentV1> for RefundIntentRecordV1 {
    fn from(value: &RefundIntentV1) -> Self {
        Self {
            schema_version: REFUND_RECORD_SCHEMA_V1,
            swap_id: value.swap_id().as_str().into(),
            agreement_commitment: *value.agreement_commitment(),
            local_participant: participant_name(value.local_participant()).into(),
            staged_revision: value.staged_revision(),
            prepared: value.prepared().into(),
        }
    }
}

impl RefundIntentRecordV1 {
    /// Primitive payload schema.
    #[must_use]
    pub const fn schema_version(&self) -> u16 {
        self.schema_version
    }

    /// Revision at which the exact signed bytes became durable.
    #[must_use]
    pub const fn staged_revision(&self) -> u64 {
        self.staged_revision
    }

    /// Rebuilds one trusted intent against the active aggregate head.
    ///
    /// # Errors
    ///
    /// Rejects unknown schemas, changed agreement/role/revision context, malformed bytes,
    /// wrong refund order, an invalid phase, or a future staged revision.
    pub fn revalidate(
        &self,
        accepted: &AcceptedZecAgreementV1,
        coordinator: &SwapCoordinator,
        current_revision: u64,
    ) -> Result<RefundIntentV1, RefundRecordError> {
        require_schema("refund intent", self.schema_version)?;
        validate_context(
            &self.swap_id,
            &self.agreement_commitment,
            &self.local_participant,
            accepted,
        )?;
        if self.staged_revision > current_revision {
            return Err(RefundRecordError::RevisionMismatch);
        }
        let local = parse_participant(&self.local_participant)
            .map_err(|_| RefundRecordError::RoleMismatch)?;
        let trusted = RefundIntentV1::from_active(
            accepted.agreement(),
            coordinator,
            local,
            self.staged_revision,
            self.prepared.revalidate()?,
        )?;
        trusted.validate_for_active(accepted.agreement(), coordinator, current_revision)?;
        if Self::from(&trusted) != *self {
            return Err(RefundRecordError::IntentMismatch);
        }
        Ok(trusted)
    }
}

impl std::fmt::Debug for RefundIntentRecordV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RefundIntentRecordV1")
            .field("schema_version", &self.schema_version)
            .field("swap_id", &self.swap_id)
            .field("agreement_commitment", &"[REDACTED]")
            .field("local_participant", &self.local_participant)
            .field("staged_revision", &self.staged_revision)
            .field("prepared", &self.prepared)
            .finish()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct RefundEvidenceRecordV1 {
    step: Box<str>,
    observed_submission_id: [u8; 32],
    transaction_id: Box<str>,
    position_chain: Box<str>,
    position_basis: Box<str>,
    position_value: u64,
    confirmations: u32,
}

impl From<&RefundEvidenceV1> for RefundEvidenceRecordV1 {
    fn from(value: &RefundEvidenceV1) -> Self {
        Self {
            step: step_name(value.step()).into(),
            observed_submission_id: *value.observed_submission_id(),
            transaction_id: value.transaction_id().into(),
            position_chain: chain_name(value.position().chain()).into(),
            position_basis: basis_name(value.position().basis()).into(),
            position_value: value.position().value(),
            confirmations: value.confirmations(),
        }
    }
}

impl RefundEvidenceRecordV1 {
    fn revalidate(
        &self,
        accepted: &AcceptedZecAgreementV1,
    ) -> Result<RefundEvidenceV1, RefundRecordError> {
        let step = parse_step(&self.step)?;
        let chain = parse_chain(&self.position_chain)?;
        let basis = parse_basis(&self.position_basis)?;
        let position = match basis {
            ClockBasis::BlockHeight => ChainPosition::block_height(chain, self.position_value),
            ClockBasis::Timestamp => ChainPosition::timestamp(chain, self.position_value),
        };
        RefundEvidenceV1::new(
            accepted.agreement(),
            step,
            self.observed_submission_id,
            self.transaction_id.clone(),
            position,
            self.confirmations,
        )
        .map_err(RefundRecordError::Refund)
    }
}

/// Versioned primitive record for one owner or observer refund projection.
///
/// An owned record must be retained beside the exact [`RefundIntentRecordV1`] that existed before
/// broadcast. An observed record must have no retained owner intent.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RefundTransitionRecordV1 {
    schema_version: u16,
    transition_kind: Box<str>,
    swap_id: Box<str>,
    agreement_commitment: [u8; 32],
    local_participant: Box<str>,
    predecessor_revision: u64,
    intent_staged_revision: Option<u64>,
    expected_submission_id: Option<[u8; 32]>,
    evidence: RefundEvidenceRecordV1,
}

impl From<&RefundTransitionV1> for RefundTransitionRecordV1 {
    fn from(value: &RefundTransitionV1) -> Self {
        Self {
            schema_version: REFUND_RECORD_SCHEMA_V1,
            transition_kind: if value.is_owned() {
                "owned"
            } else {
                "observed"
            }
            .into(),
            swap_id: value.swap_id().as_str().into(),
            agreement_commitment: *value.agreement_commitment(),
            local_participant: participant_name(value.local_participant()).into(),
            predecessor_revision: value.predecessor_revision(),
            intent_staged_revision: value.intent_staged_revision(),
            expected_submission_id: value.expected_submission_id().copied(),
            evidence: value.evidence().into(),
        }
    }
}

impl RefundTransitionRecordV1 {
    /// Primitive payload schema.
    #[must_use]
    pub const fn schema_version(&self) -> u16 {
        self.schema_version
    }

    /// Exact predecessor slot occupied by this role-local transition.
    #[must_use]
    pub const fn predecessor_revision(&self) -> u64 {
        self.predecessor_revision
    }

    /// Whether revalidation requires a retained exact owner intent record.
    #[must_use]
    pub fn is_owned(&self) -> bool {
        self.transition_kind.as_ref() == "owned"
    }

    /// Rebuilds a trusted transition against the exact predecessor head.
    ///
    /// `retained_intent` must be the exact pre-broadcast record copied into the committed row for
    /// an owner transition, and must be absent for an observer transition.
    ///
    /// # Errors
    ///
    /// Rejects unknown schemas/kinds, substituted context/evidence/intent, wrong roles or order,
    /// an invalid deadline/depth/phase, or a changed predecessor revision.
    pub fn revalidate(
        &self,
        accepted: &AcceptedZecAgreementV1,
        coordinator: &SwapCoordinator,
        predecessor_revision: u64,
        retained_intent: Option<&RefundIntentRecordV1>,
    ) -> Result<RefundTransitionV1, RefundRecordError> {
        require_schema("refund transition", self.schema_version)?;
        validate_context(
            &self.swap_id,
            &self.agreement_commitment,
            &self.local_participant,
            accepted,
        )?;
        if self.predecessor_revision != predecessor_revision {
            return Err(RefundRecordError::RevisionMismatch);
        }
        let local = parse_participant(&self.local_participant)
            .map_err(|_| RefundRecordError::RoleMismatch)?;
        let evidence = self.evidence.revalidate(accepted)?;
        let trusted = match self.transition_kind.as_ref() {
            "owned" => {
                let intent_record = retained_intent.ok_or(RefundRecordError::MissingIntent)?;
                let intent =
                    intent_record.revalidate(accepted, coordinator, predecessor_revision)?;
                RefundTransitionV1::from_owned(
                    accepted.agreement(),
                    coordinator,
                    &intent,
                    predecessor_revision,
                    evidence,
                )?
            }
            "observed" => {
                if retained_intent.is_some() {
                    return Err(RefundRecordError::UnexpectedIntent);
                }
                RefundTransitionV1::from_observed(
                    accepted.agreement(),
                    coordinator,
                    local,
                    predecessor_revision,
                    evidence,
                )?
            }
            _ => return Err(RefundRecordError::UnknownTransitionKind),
        };
        if Self::from(&trusted) != *self {
            return Err(RefundRecordError::TransitionMismatch);
        }
        Ok(trusted)
    }
}

fn validate_context(
    swap_id: &str,
    agreement_commitment: &[u8; 32],
    local_participant: &str,
    accepted: &AcceptedZecAgreementV1,
) -> Result<(), RefundRecordError> {
    if swap_id != accepted.agreement().coordinator().id().as_str()
        || agreement_commitment != accepted.agreement().agreement_commitment()
    {
        return Err(RefundRecordError::ContextMismatch);
    }
    let local =
        parse_participant(local_participant).map_err(|_| RefundRecordError::RoleMismatch)?;
    if local != accepted.local_participant() {
        return Err(RefundRecordError::RoleMismatch);
    }
    Ok(())
}

fn require_schema(label: &'static str, actual: u16) -> Result<(), RefundRecordError> {
    if actual == REFUND_RECORD_SCHEMA_V1 {
        Ok(())
    } else {
        Err(RefundRecordError::UnsupportedSchema { label, actual })
    }
}

const fn step_name(step: RefundStepV1) -> &'static str {
    match step {
        RefundStepV1::Lez => "lez",
        RefundStepV1::Zcash => "zcash",
    }
}

fn parse_step(value: &str) -> Result<RefundStepV1, RefundRecordError> {
    match value {
        "lez" => Ok(RefundStepV1::Lez),
        "zcash" => Ok(RefundStepV1::Zcash),
        _ => Err(RefundRecordError::UnknownStep),
    }
}

const fn chain_name(chain: Chain) -> &'static str {
    match chain {
        Chain::Lez => "lez",
        Chain::Bitcoin => "bitcoin",
        Chain::Monero => "monero",
        Chain::Zcash => "zcash",
    }
}

fn parse_chain(value: &str) -> Result<Chain, RefundRecordError> {
    match value {
        "lez" => Ok(Chain::Lez),
        "bitcoin" => Ok(Chain::Bitcoin),
        "monero" => Ok(Chain::Monero),
        "zcash" => Ok(Chain::Zcash),
        _ => Err(RefundRecordError::UnknownChain),
    }
}

const fn basis_name(basis: ClockBasis) -> &'static str {
    match basis {
        ClockBasis::BlockHeight => "block_height",
        ClockBasis::Timestamp => "timestamp",
    }
}

fn parse_basis(value: &str) -> Result<ClockBasis, RefundRecordError> {
    match value {
        "block_height" => Ok(ClockBasis::BlockHeight),
        "timestamp" => Ok(ClockBasis::Timestamp),
        _ => Err(RefundRecordError::UnknownClockBasis),
    }
}

/// Primitive refund record validation failure.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RefundRecordError {
    /// Payload schema is not understood.
    #[error("unsupported {label} schema {actual}")]
    UnsupportedSchema {
        /// Record kind.
        label: &'static str,
        /// Unsupported version.
        actual: u16,
    },
    /// Role encoding is invalid or disagrees with the role-local accepted agreement.
    #[error("refund record role does not match the accepted agreement")]
    RoleMismatch,
    /// Swap ID or agreement commitment changed.
    #[error("refund record context does not match the accepted agreement")]
    ContextMismatch,
    /// Staged or predecessor revision changed.
    #[error("refund record revision does not match the durable head")]
    RevisionMismatch,
    /// Step encoding is unknown.
    #[error("refund record step is unknown")]
    UnknownStep,
    /// Chain encoding is unknown.
    #[error("refund record chain is unknown")]
    UnknownChain,
    /// Clock-basis encoding is unknown.
    #[error("refund record clock basis is unknown")]
    UnknownClockBasis,
    /// Transition kind is neither owner nor observer.
    #[error("refund transition kind is unknown")]
    UnknownTransitionKind,
    /// An owned transition omitted its exact retained pre-broadcast intent.
    #[error("owned refund transition is missing its retained exact intent")]
    MissingIntent,
    /// An observer transition incorrectly carries owner signing material.
    #[error("observed refund transition unexpectedly carries an owner intent")]
    UnexpectedIntent,
    /// Reconstructed intent differs from the serialized primitive record.
    #[error("refund intent record does not reconstruct byte-for-byte")]
    IntentMismatch,
    /// Reconstructed transition differs from the serialized primitive record.
    #[error("refund transition record does not reconstruct byte-for-byte")]
    TransitionMismatch,
    /// Domain validation rejected prepared bytes, evidence, role, phase, order, or deadline.
    #[error(transparent)]
    Refund(#[from] RefundError),
}
