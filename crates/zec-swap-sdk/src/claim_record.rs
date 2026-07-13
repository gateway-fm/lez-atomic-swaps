//! Secret-free primitive persistence records for claim intents and transitions.
//!
//! Exact claim transaction bytes belong in an authenticated protected envelope. A revealing
//! preimage is supplied separately after decryption; neither value is serializable here.

use lez_swap_core::{SwapCoordinator, SwapId};
use serde::{Deserialize, Serialize};

use crate::{
    AcceptedZecAgreementV1, CanonicalLezClaimSnapshotRecordV1, ClaimError, ClaimIntentV1,
    ClaimPreimage, ClaimStepV1, FollowupClaimEvidenceV1, FollowupClaimTransitionV1,
    LezClaimObservationError, ObservedFollowupClaimTransitionV1,
    ObservedRevealingClaimTransitionV1, RevealingClaimEvidenceV1, RevealingClaimTransitionV1,
    first_lock_record::{parse_participant, participant_name},
};

/// Stable payload version for claim records.
pub const CLAIM_RECORD_SCHEMA_V1: u16 = 1;
/// Canonical LEZ revealing records carrying a secret-free primitive node snapshot.
pub const CLAIM_RECORD_SCHEMA_V2: u16 = 2;

/// Primitive secret-free binding to a separately protected exact claim submission.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimIntentRecordV1 {
    schema_version: u16,
    swap_id: Box<str>,
    agreement_commitment: [u8; 32],
    local_participant: Box<str>,
    staged_revision: u64,
    step: Box<str>,
    expected_submission_id: [u8; 32],
    protected_payload_fingerprint: [u8; 32],
}

impl From<&ClaimIntentV1> for ClaimIntentRecordV1 {
    fn from(value: &ClaimIntentV1) -> Self {
        Self {
            schema_version: CLAIM_RECORD_SCHEMA_V1,
            swap_id: value.swap_id().as_str().into(),
            agreement_commitment: *value.agreement_commitment(),
            local_participant: participant_name(value.local_participant()).into(),
            staged_revision: value.staged_revision(),
            step: step_name(value.step()).into(),
            expected_submission_id: *value.expected_submission_id(),
            protected_payload_fingerprint: *value.protected_payload_fingerprint(),
        }
    }
}

impl ClaimIntentRecordV1 {
    /// Primitive payload schema.
    #[must_use]
    pub const fn schema_version(&self) -> u16 {
        self.schema_version
    }

    /// Rebuilds a trusted claim intent against the active aggregate head.
    ///
    /// # Errors
    ///
    /// Rejects unknown schemas, malformed or substituted context, the wrong role/step,
    /// an invalid phase, or a future staged revision.
    pub fn revalidate(
        &self,
        accepted: &AcceptedZecAgreementV1,
        coordinator: &SwapCoordinator,
        current_revision: u64,
    ) -> Result<ClaimIntentV1, ClaimRecordError> {
        require_schema("claim intent", self.schema_version)?;
        validate_common_context(
            &self.swap_id,
            &self.agreement_commitment,
            &self.local_participant,
            accepted,
        )?;
        if self.staged_revision > current_revision {
            return Err(ClaimRecordError::RevisionMismatch);
        }
        let local = parse_participant(&self.local_participant)
            .map_err(|_| ClaimRecordError::RoleMismatch)?;
        let trusted = ClaimIntentV1::from_protected_binding(
            accepted.agreement(),
            coordinator,
            local,
            self.staged_revision,
            parse_step(&self.step)?,
            self.expected_submission_id,
            self.protected_payload_fingerprint,
        )?;
        if Self::from(&trusted) != *self {
            return Err(ClaimRecordError::IntentMismatch);
        }
        Ok(trusted)
    }
}

/// Secret-free primitive payload for an applied LEZ revealing claim.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RevealingClaimTransitionRecordV1 {
    schema_version: u16,
    transition_kind: Box<str>,
    swap_id: Box<str>,
    agreement_commitment: [u8; 32],
    local_participant: Box<str>,
    predecessor_revision: u64,
    intent_staged_revision: u64,
    observed_submission_id: [u8; 32],
    transaction_id: Box<str>,
    confirmations: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    canonical_lez_snapshot: Option<CanonicalLezClaimSnapshotRecordV1>,
}

impl From<&RevealingClaimTransitionV1> for RevealingClaimTransitionRecordV1 {
    fn from(value: &RevealingClaimTransitionV1) -> Self {
        Self {
            schema_version: if value.evidence().canonical_lez_snapshot().is_some() {
                CLAIM_RECORD_SCHEMA_V2
            } else {
                CLAIM_RECORD_SCHEMA_V1
            },
            transition_kind: "revealing_lez".into(),
            swap_id: value.swap_id().as_str().into(),
            agreement_commitment: *value.agreement_commitment(),
            local_participant: participant_name(value.local_participant()).into(),
            predecessor_revision: value.predecessor_revision(),
            intent_staged_revision: value.intent_staged_revision(),
            observed_submission_id: *value.evidence().observed_submission_id(),
            transaction_id: value.evidence().transaction_id().into(),
            confirmations: value.evidence().confirmations(),
            canonical_lez_snapshot: value.evidence().canonical_lez_snapshot().cloned(),
        }
    }
}

impl RevealingClaimTransitionRecordV1 {
    /// Primitive payload schema.
    #[must_use]
    pub const fn schema_version(&self) -> u16 {
        self.schema_version
    }

    /// Rebuilds a trusted revealing transition using a separately decrypted preimage.
    ///
    /// # Errors
    ///
    /// Rejects unknown schemas, substituted context/intent/evidence, a wrong phase or role,
    /// insufficient LEZ depth, or a preimage that does not satisfy the agreement digest.
    pub fn revalidate(
        &self,
        accepted: &AcceptedZecAgreementV1,
        coordinator: &SwapCoordinator,
        retained_intent: &ClaimIntentRecordV1,
        predecessor_revision: u64,
        preimage: ClaimPreimage,
    ) -> Result<RevealingClaimTransitionV1, ClaimRecordError> {
        require_revealing_schema("revealing claim transition", self.schema_version)?;
        if self.transition_kind.as_ref() != "revealing_lez"
            || self.predecessor_revision != predecessor_revision
            || self.intent_staged_revision > predecessor_revision
        {
            return Err(ClaimRecordError::RevisionMismatch);
        }
        validate_common_context(
            &self.swap_id,
            &self.agreement_commitment,
            &self.local_participant,
            accepted,
        )?;
        let intent = retained_intent.revalidate(accepted, coordinator, predecessor_revision)?;
        if intent.staged_revision() != self.intent_staged_revision
            || intent.step() != ClaimStepV1::RevealingLez
        {
            return Err(ClaimRecordError::RevisionMismatch);
        }
        let evidence = match (self.schema_version, &self.canonical_lez_snapshot) {
            (CLAIM_RECORD_SCHEMA_V1, None) => RevealingClaimEvidenceV1::from_legacy_recovery_parts(
                accepted.agreement(),
                self.observed_submission_id,
                self.transaction_id.clone(),
                self.confirmations,
                preimage,
            )?,
            (CLAIM_RECORD_SCHEMA_V2, Some(snapshot)) => {
                RevealingClaimEvidenceV1::from_lez_claim_snapshot_record(
                    accepted.agreement(),
                    Some(intent.expected_submission_id()),
                    snapshot,
                    preimage,
                )?
            }
            _ => return Err(ClaimRecordError::TransitionMismatch),
        };
        let trusted = RevealingClaimTransitionV1::from_active(
            accepted.agreement(),
            coordinator,
            &intent,
            predecessor_revision,
            evidence,
        )?;
        if Self::from(&trusted) != *self {
            return Err(ClaimRecordError::TransitionMismatch);
        }
        Ok(trusted)
    }
}

/// Secret-free observer-store payload for a canonical LEZ revealing claim.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ObservedRevealingClaimTransitionRecordV1 {
    schema_version: u16,
    transition_kind: Box<str>,
    swap_id: Box<str>,
    agreement_commitment: [u8; 32],
    local_participant: Box<str>,
    predecessor_revision: u64,
    observed_submission_id: [u8; 32],
    transaction_id: Box<str>,
    confirmations: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    canonical_lez_snapshot: Option<CanonicalLezClaimSnapshotRecordV1>,
}

impl From<&ObservedRevealingClaimTransitionV1> for ObservedRevealingClaimTransitionRecordV1 {
    fn from(value: &ObservedRevealingClaimTransitionV1) -> Self {
        Self {
            schema_version: if value.evidence().canonical_lez_snapshot().is_some() {
                CLAIM_RECORD_SCHEMA_V2
            } else {
                CLAIM_RECORD_SCHEMA_V1
            },
            transition_kind: "observed_revealing_lez".into(),
            swap_id: value.swap_id().as_str().into(),
            agreement_commitment: *value.agreement_commitment(),
            local_participant: participant_name(value.local_participant()).into(),
            predecessor_revision: value.predecessor_revision(),
            observed_submission_id: *value.evidence().observed_submission_id(),
            transaction_id: value.evidence().transaction_id().into(),
            confirmations: value.evidence().confirmations(),
            canonical_lez_snapshot: value.evidence().canonical_lez_snapshot().cloned(),
        }
    }
}

impl ObservedRevealingClaimTransitionRecordV1 {
    /// Primitive payload schema.
    #[must_use]
    pub const fn schema_version(&self) -> u16 {
        self.schema_version
    }

    /// Rebuilds observer-local LEZ evidence using separately decrypted extracted material.
    ///
    /// The canonical adapter supplies both the node-reported identity and the official-decoder
    /// hash. Replay requires those identities to agree even though an observer has no local
    /// exact-submission plan to compare.
    ///
    /// # Errors
    ///
    /// Rejects unknown schemas, owner-local or substituted context, stale revisions,
    /// insufficient depth, or a preimage that does not satisfy the agreement digest.
    pub fn revalidate(
        &self,
        accepted: &AcceptedZecAgreementV1,
        coordinator: &SwapCoordinator,
        predecessor_revision: u64,
        preimage: ClaimPreimage,
    ) -> Result<ObservedRevealingClaimTransitionV1, ClaimRecordError> {
        require_revealing_schema("observed revealing claim transition", self.schema_version)?;
        if self.transition_kind.as_ref() != "observed_revealing_lez"
            || self.predecessor_revision != predecessor_revision
        {
            return Err(ClaimRecordError::RevisionMismatch);
        }
        validate_common_context(
            &self.swap_id,
            &self.agreement_commitment,
            &self.local_participant,
            accepted,
        )?;
        let evidence = match (self.schema_version, &self.canonical_lez_snapshot) {
            (CLAIM_RECORD_SCHEMA_V1, None) => RevealingClaimEvidenceV1::from_legacy_recovery_parts(
                accepted.agreement(),
                self.observed_submission_id,
                self.transaction_id.clone(),
                self.confirmations,
                preimage,
            )?,
            (CLAIM_RECORD_SCHEMA_V2, Some(snapshot)) => {
                RevealingClaimEvidenceV1::from_lez_claim_snapshot_record(
                    accepted.agreement(),
                    None,
                    snapshot,
                    preimage,
                )?
            }
            _ => return Err(ClaimRecordError::TransitionMismatch),
        };
        let trusted = ObservedRevealingClaimTransitionV1::from_active(
            accepted.agreement(),
            coordinator,
            accepted.local_participant(),
            predecessor_revision,
            evidence,
        )?;
        if Self::from(&trusted) != *self {
            return Err(ClaimRecordError::TransitionMismatch);
        }
        Ok(trusted)
    }
}

/// Primitive payload for an applied Zcash follow-up claim.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FollowupClaimTransitionRecordV1 {
    schema_version: u16,
    transition_kind: Box<str>,
    swap_id: Box<str>,
    agreement_commitment: [u8; 32],
    local_participant: Box<str>,
    predecessor_revision: u64,
    intent_staged_revision: u64,
    observed_submission_id: [u8; 32],
    transaction_id: Box<str>,
    confirmations: u32,
}

impl From<&FollowupClaimTransitionV1> for FollowupClaimTransitionRecordV1 {
    fn from(value: &FollowupClaimTransitionV1) -> Self {
        Self {
            schema_version: value.schema_version(),
            transition_kind: "followup_zcash".into(),
            swap_id: value.swap_id().as_str().into(),
            agreement_commitment: *value.agreement_commitment(),
            local_participant: participant_name(value.local_participant()).into(),
            predecessor_revision: value.predecessor_revision(),
            intent_staged_revision: value.intent_staged_revision(),
            observed_submission_id: *value.evidence().observed_submission_id(),
            transaction_id: value.evidence().transaction_id().into(),
            confirmations: value.evidence().confirmations(),
        }
    }
}

impl FollowupClaimTransitionRecordV1 {
    /// Primitive payload schema.
    #[must_use]
    pub const fn schema_version(&self) -> u16 {
        self.schema_version
    }

    /// Rebuilds a trusted Zcash follow-up transition from its retained intent.
    ///
    /// # Errors
    ///
    /// Rejects unknown schemas, substituted context/intent/evidence, a wrong phase or role,
    /// or confirmation depth below the Zcash funder's signed policy.
    pub fn revalidate(
        &self,
        accepted: &AcceptedZecAgreementV1,
        coordinator: &SwapCoordinator,
        retained_intent: &ClaimIntentRecordV1,
        predecessor_revision: u64,
    ) -> Result<FollowupClaimTransitionV1, ClaimRecordError> {
        require_schema("follow-up claim transition", self.schema_version)?;
        if self.transition_kind.as_ref() != "followup_zcash"
            || self.predecessor_revision != predecessor_revision
            || self.intent_staged_revision > predecessor_revision
        {
            return Err(ClaimRecordError::RevisionMismatch);
        }
        validate_common_context(
            &self.swap_id,
            &self.agreement_commitment,
            &self.local_participant,
            accepted,
        )?;
        let intent = retained_intent.revalidate(accepted, coordinator, predecessor_revision)?;
        if intent.staged_revision() != self.intent_staged_revision
            || intent.step() != ClaimStepV1::FollowupZcash
        {
            return Err(ClaimRecordError::RevisionMismatch);
        }
        let evidence = FollowupClaimEvidenceV1::new(
            accepted.agreement(),
            self.observed_submission_id,
            self.transaction_id.clone(),
            self.confirmations,
        )?;
        let trusted = FollowupClaimTransitionV1::from_active(
            accepted.agreement(),
            coordinator,
            &intent,
            predecessor_revision,
            evidence,
        )?;
        if Self::from(&trusted) != *self {
            return Err(ClaimRecordError::TransitionMismatch);
        }
        Ok(trusted)
    }
}

/// Observer-store payload for a canonical Zcash follow-up claim.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ObservedFollowupClaimTransitionRecordV1 {
    schema_version: u16,
    transition_kind: Box<str>,
    swap_id: Box<str>,
    agreement_commitment: [u8; 32],
    local_participant: Box<str>,
    predecessor_revision: u64,
    observed_submission_id: [u8; 32],
    transaction_id: Box<str>,
    confirmations: u32,
}

impl From<&ObservedFollowupClaimTransitionV1> for ObservedFollowupClaimTransitionRecordV1 {
    fn from(value: &ObservedFollowupClaimTransitionV1) -> Self {
        Self {
            schema_version: value.schema_version(),
            transition_kind: "observed_followup_zcash".into(),
            swap_id: value.swap_id().as_str().into(),
            agreement_commitment: *value.agreement_commitment(),
            local_participant: participant_name(value.local_participant()).into(),
            predecessor_revision: value.predecessor_revision(),
            observed_submission_id: *value.evidence().observed_submission_id(),
            transaction_id: value.evidence().transaction_id().into(),
            confirmations: value.evidence().confirmations(),
        }
    }
}

impl ObservedFollowupClaimTransitionRecordV1 {
    /// Primitive payload schema.
    #[must_use]
    pub const fn schema_version(&self) -> u16 {
        self.schema_version
    }

    /// Rebuilds observer-local canonical Zcash follow-up evidence.
    ///
    /// # Errors
    ///
    /// Rejects unknown schemas, owner-local or substituted context, stale revisions,
    /// malformed canonical identity, or insufficient Zcash confirmation depth.
    pub fn revalidate(
        &self,
        accepted: &AcceptedZecAgreementV1,
        coordinator: &SwapCoordinator,
        predecessor_revision: u64,
    ) -> Result<ObservedFollowupClaimTransitionV1, ClaimRecordError> {
        require_schema("observed follow-up claim transition", self.schema_version)?;
        if self.transition_kind.as_ref() != "observed_followup_zcash"
            || self.predecessor_revision != predecessor_revision
        {
            return Err(ClaimRecordError::RevisionMismatch);
        }
        validate_common_context(
            &self.swap_id,
            &self.agreement_commitment,
            &self.local_participant,
            accepted,
        )?;
        let evidence = FollowupClaimEvidenceV1::new(
            accepted.agreement(),
            self.observed_submission_id,
            self.transaction_id.clone(),
            self.confirmations,
        )?;
        let trusted = ObservedFollowupClaimTransitionV1::from_active(
            accepted.agreement(),
            coordinator,
            accepted.local_participant(),
            predecessor_revision,
            evidence,
        )?;
        if Self::from(&trusted) != *self {
            return Err(ClaimRecordError::TransitionMismatch);
        }
        Ok(trusted)
    }
}

/// Failure to reconstruct trusted claim domain values from primitive rows.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ClaimRecordError {
    /// Payload schema is unsupported.
    #[error("unsupported {record} schema {actual}")]
    UnsupportedSchema {
        /// Primitive record family.
        record: &'static str,
        /// Untrusted schema version.
        actual: u16,
    },
    /// Swap or agreement commitment differs from the accepted agreement.
    #[error("claim record agreement context mismatch")]
    AgreementContextMismatch,
    /// Primitive role is malformed or differs from local acceptance.
    #[error("claim record local role mismatch")]
    RoleMismatch,
    /// Staging, predecessor, intent link, kind, or step is inconsistent.
    #[error("claim record revision or intent link mismatch")]
    RevisionMismatch,
    /// Reconstructed intent is not canonical.
    #[error("claim intent record is not canonical")]
    IntentMismatch,
    /// Reconstructed transition is not canonical.
    #[error("claim transition record is not canonical")]
    TransitionMismatch,
    /// Claim domain validation failed.
    #[error(transparent)]
    Claim(#[from] ClaimError),
    /// Canonical LEZ primitive snapshot validation failed during v2 replay.
    #[error(transparent)]
    LezClaim(#[from] LezClaimObservationError),
    /// Primitive swap identity violates core bounds.
    #[error(transparent)]
    Core(lez_swap_core::Error),
}

fn validate_common_context(
    swap_id: &str,
    agreement_commitment: &[u8; 32],
    local_participant: &str,
    accepted: &AcceptedZecAgreementV1,
) -> Result<(), ClaimRecordError> {
    let parsed_swap_id = SwapId::new(swap_id).map_err(ClaimRecordError::Core)?;
    if &parsed_swap_id != accepted.agreement().coordinator().id()
        || agreement_commitment != accepted.agreement().agreement_commitment()
    {
        return Err(ClaimRecordError::AgreementContextMismatch);
    }
    let local = parse_participant(local_participant).map_err(|_| ClaimRecordError::RoleMismatch)?;
    if local != accepted.local_participant() {
        return Err(ClaimRecordError::RoleMismatch);
    }
    Ok(())
}

fn require_schema(record: &'static str, actual: u16) -> Result<(), ClaimRecordError> {
    if actual == CLAIM_RECORD_SCHEMA_V1 {
        Ok(())
    } else {
        Err(ClaimRecordError::UnsupportedSchema { record, actual })
    }
}

fn require_revealing_schema(record: &'static str, actual: u16) -> Result<(), ClaimRecordError> {
    if matches!(actual, CLAIM_RECORD_SCHEMA_V1 | CLAIM_RECORD_SCHEMA_V2) {
        Ok(())
    } else {
        Err(ClaimRecordError::UnsupportedSchema { record, actual })
    }
}

const fn step_name(step: ClaimStepV1) -> &'static str {
    match step {
        ClaimStepV1::RevealingLez => "revealing_lez",
        ClaimStepV1::FollowupZcash => "followup_zcash",
    }
}

fn parse_step(value: &str) -> Result<ClaimStepV1, ClaimRecordError> {
    match value {
        "revealing_lez" => Ok(ClaimStepV1::RevealingLez),
        "followup_zcash" => Ok(ClaimStepV1::FollowupZcash),
        _ => Err(ClaimRecordError::RevisionMismatch),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intent_record() -> ClaimIntentRecordV1 {
        ClaimIntentRecordV1 {
            schema_version: CLAIM_RECORD_SCHEMA_V1,
            swap_id: "swap-1".into(),
            agreement_commitment: [3; 32],
            local_participant: "maker".into(),
            staged_revision: 4,
            step: "revealing_lez".into(),
            expected_submission_id: [5; 32],
            protected_payload_fingerprint: [6; 32],
        }
    }

    #[test]
    fn primitive_intent_contains_only_secret_free_binding_fields() {
        let json = serde_json::to_string(&intent_record()).expect("record serializes");
        assert!(json.contains("protected_payload_fingerprint"));
        assert!(!json.contains("preimage"));
        assert!(!json.contains("exact_submission"));
    }

    #[test]
    fn primitive_claim_records_reject_unknown_fields() {
        let mut value = serde_json::to_value(intent_record()).expect("record serializes");
        value.as_object_mut().expect("record is an object").insert(
            "future_secret".into(),
            serde_json::to_value(vec![7_u8; 32]).expect("array serializes"),
        );
        assert!(serde_json::from_value::<ClaimIntentRecordV1>(value).is_err());
    }

    #[test]
    fn revealing_transition_record_has_no_preimage_field() {
        let record = RevealingClaimTransitionRecordV1 {
            schema_version: CLAIM_RECORD_SCHEMA_V1,
            transition_kind: "revealing_lez".into(),
            swap_id: "swap-1".into(),
            agreement_commitment: [3; 32],
            local_participant: "maker".into(),
            predecessor_revision: 5,
            intent_staged_revision: 4,
            observed_submission_id: [5; 32],
            transaction_id: "lez-claim-id".into(),
            confirmations: 2,
            canonical_lez_snapshot: None,
        };
        let json = serde_json::to_string(&record).expect("record serializes");
        assert!(!json.contains("preimage"));
        assert!(!json.contains("exact_submission"));
    }

    #[test]
    fn owner_and_observer_records_are_distinct_for_both_claim_steps() {
        let observed_revealing = ObservedRevealingClaimTransitionRecordV1 {
            schema_version: CLAIM_RECORD_SCHEMA_V1,
            transition_kind: "observed_revealing_lez".into(),
            swap_id: "swap-1".into(),
            agreement_commitment: [3; 32],
            local_participant: "taker".into(),
            predecessor_revision: 5,
            observed_submission_id: [5; 32],
            transaction_id: "lez-claim-id".into(),
            confirmations: 2,
            canonical_lez_snapshot: None,
        };
        let owned_followup = FollowupClaimTransitionRecordV1 {
            schema_version: CLAIM_RECORD_SCHEMA_V1,
            transition_kind: "followup_zcash".into(),
            swap_id: "swap-1".into(),
            agreement_commitment: [3; 32],
            local_participant: "taker".into(),
            predecessor_revision: 6,
            intent_staged_revision: 6,
            observed_submission_id: [8; 32],
            transaction_id: "zcash-claim-id".into(),
            confirmations: 3,
        };
        let observed_followup = ObservedFollowupClaimTransitionRecordV1 {
            schema_version: CLAIM_RECORD_SCHEMA_V1,
            transition_kind: "observed_followup_zcash".into(),
            swap_id: "swap-1".into(),
            agreement_commitment: [3; 32],
            local_participant: "maker".into(),
            predecessor_revision: 6,
            observed_submission_id: [8; 32],
            transaction_id: "zcash-claim-id".into(),
            confirmations: 3,
        };

        let revealing_json = serde_json::to_string(&observed_revealing).expect("serializes");
        let owned_followup_json = serde_json::to_string(&owned_followup).expect("serializes");
        let observed_followup_json = serde_json::to_string(&observed_followup).expect("serializes");

        assert!(!revealing_json.contains("intent_staged_revision"));
        assert!(owned_followup_json.contains("intent_staged_revision"));
        assert!(!observed_followup_json.contains("intent_staged_revision"));
        for json in [revealing_json, owned_followup_json, observed_followup_json] {
            assert!(!json.contains("preimage"));
            assert!(!json.contains("exact_submission"));
        }
    }
}
