//! Primitive durable DTO for taker-local maker-lock observations.

use lez_swap_core::Participant;
use serde::{Deserialize, Serialize};

use crate::{
    AcceptedZecAgreementV1, FirstLockConfirmedEvidenceV1, FirstLockStepV1, ObservedMakerLockError,
    ObservedMakerLockTransitionV1, observed_maker_lock::OBSERVED_MAKER_LOCK_SCHEMA_V1,
};

const OBSERVED_MAKER_LOCK_KIND_V1: &str = "taker_observed_maker_lock";

/// Primitive untrusted persistence record for a taker-local maker-lock observation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ObservedMakerLockTransitionRecordV1 {
    schema_version: u16,
    transition_kind: Box<str>,
    swap_id: Box<str>,
    agreement_commitment: [u8; 32],
    local_participant: Box<str>,
    predecessor_revision: u64,
    evidence_schema_version: u16,
    step: Box<str>,
    expected_submission_id: [u8; 32],
    transaction_id: Box<str>,
    confirmations: u32,
}

impl From<&ObservedMakerLockTransitionV1> for ObservedMakerLockTransitionRecordV1 {
    fn from(value: &ObservedMakerLockTransitionV1) -> Self {
        Self {
            schema_version: value.schema_version(),
            transition_kind: OBSERVED_MAKER_LOCK_KIND_V1.into(),
            swap_id: value.swap_id().as_str().into(),
            agreement_commitment: *value.agreement_commitment(),
            local_participant: "taker".into(),
            predecessor_revision: value.predecessor_revision(),
            evidence_schema_version: value.evidence().schema_version(),
            step: step_name(value.evidence().step()).into(),
            expected_submission_id: *value.evidence().expected_submission_id(),
            transaction_id: value.evidence().transaction_id().into(),
            confirmations: value.evidence().confirmations(),
        }
    }
}

impl ObservedMakerLockTransitionRecordV1 {
    /// Primitive payload schema.
    #[must_use]
    pub const fn schema_version(&self) -> u16 {
        self.schema_version
    }

    /// Rebuilds a trusted transition against the independently accepted agreement.
    ///
    /// # Errors
    ///
    /// Rejects unknown schemas, substituted context or role fields, a stale
    /// predecessor, malformed identities, the wrong maker chain, or insufficient depth.
    pub fn revalidate(
        &self,
        accepted: &AcceptedZecAgreementV1,
        predecessor_revision: u64,
    ) -> Result<ObservedMakerLockTransitionV1, ObservedMakerLockError> {
        if self.schema_version != OBSERVED_MAKER_LOCK_SCHEMA_V1
            || self.transition_kind.as_ref() != OBSERVED_MAKER_LOCK_KIND_V1
            || self.evidence_schema_version != OBSERVED_MAKER_LOCK_SCHEMA_V1
            || self.swap_id.as_ref() != accepted.agreement().coordinator().id().as_str()
            || self.agreement_commitment != *accepted.agreement().agreement_commitment()
            || self.local_participant.as_ref() != "taker"
            || accepted.local_participant() != Participant::Taker
            || self.predecessor_revision != predecessor_revision
        {
            return Err(ObservedMakerLockError::ContextMismatch);
        }
        let evidence = FirstLockConfirmedEvidenceV1::new(
            parse_step(&self.step)?,
            self.expected_submission_id,
            self.transaction_id.clone(),
            self.confirmations,
        )?;
        let trusted = ObservedMakerLockTransitionV1::from_active(
            accepted.agreement(),
            Participant::Taker,
            predecessor_revision,
            evidence,
        )?;
        if Self::from(&trusted) != *self {
            return Err(ObservedMakerLockError::ContextMismatch);
        }
        Ok(trusted)
    }
}

const fn step_name(step: FirstLockStepV1) -> &'static str {
    match step {
        FirstLockStepV1::ZcashFund => "zcash_fund",
        FirstLockStepV1::LezInitialize => "lez_initialize",
        FirstLockStepV1::LezFund => "lez_fund",
    }
}

fn parse_step(value: &str) -> Result<FirstLockStepV1, ObservedMakerLockError> {
    match value {
        "zcash_fund" => Ok(FirstLockStepV1::ZcashFund),
        "lez_initialize" => Ok(FirstLockStepV1::LezInitialize),
        "lez_fund" => Ok(FirstLockStepV1::LezFund),
        _ => Err(ObservedMakerLockError::ContextMismatch),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(step: &str) -> ObservedMakerLockTransitionRecordV1 {
        ObservedMakerLockTransitionRecordV1 {
            schema_version: OBSERVED_MAKER_LOCK_SCHEMA_V1,
            transition_kind: OBSERVED_MAKER_LOCK_KIND_V1.into(),
            swap_id: "swap-1".into(),
            agreement_commitment: [7; 32],
            local_participant: "taker".into(),
            predecessor_revision: 2,
            evidence_schema_version: OBSERVED_MAKER_LOCK_SCHEMA_V1,
            step: step.into(),
            expected_submission_id: [8; 32],
            transaction_id: "maker-chain-transaction".into(),
            confirmations: 3,
        }
    }

    #[test]
    fn primitive_record_round_trip_preserves_both_identities() {
        let original = record("lez_fund");
        let encoded = serde_json::to_vec(&original).expect("record serializes");
        let decoded = serde_json::from_slice(&encoded).expect("record deserializes");
        assert_eq!(original, decoded);
    }

    #[test]
    fn primitive_record_rejects_unknown_fields() {
        let mut value = serde_json::to_value(record("zcash_fund")).expect("record serializes");
        value
            .as_object_mut()
            .expect("record is an object")
            .insert("future_field".into(), serde_json::json!(true));
        assert!(serde_json::from_value::<ObservedMakerLockTransitionRecordV1>(value).is_err());
    }

    #[test]
    fn primitive_step_parser_fails_closed() {
        assert_eq!(
            parse_step("lez_initialize_v2"),
            Err(ObservedMakerLockError::ContextMismatch)
        );
    }
}
