//! Secret-safe claim intents, canonical evidence, and lifecycle transitions.

use lez_swap_core::{ChainProof, ClaimEvidence, Participant, Phase, SwapCoordinator, SwapId};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use zeroize::Zeroize;

use crate::{CanonicalLezClaimSnapshotRecordV1, ClaimPreimage, ZecAgreementV1};

/// Maximum exact signed claim submission accepted before protected persistence.
pub const MAX_CLAIM_SUBMISSION_BYTES: usize = 2_000_000;

/// One chain-ordered claim action.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ClaimStepV1 {
    /// The LEZ recipient claims first and reveals the agreement preimage.
    RevealingLez,
    /// The other participant consumes the revealed preimage on Zcash.
    FollowupZcash,
}

/// One bounded result from advancing the agreement-directed claim lifecycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClaimDriveOutcome {
    /// Exact protected bytes were submitted to the step chain adapter.
    Submitted(ClaimStepV1),
    /// The expected transaction is absent or not yet stable enough to project.
    AwaitingStableObservation(ClaimStepV1),
    /// Stable canonical evidence was durably projected.
    Projected { step: ClaimStepV1, revision: u64 },
    /// Both claims were already durably replayed or projected.
    Completed { revision: u64 },
}

/// Canonical LEZ revealing-claim observation.
#[derive(Debug)]
pub enum RevealingClaimObservationV1 {
    /// The agreement-derived transaction identity is stably absent.
    Absent,
    /// A candidate exists but is not stable enough to project.
    Unstable,
    /// Stable evidence includes the transient revealed preimage.
    Confirmed(RevealingClaimEvidenceV1),
}

/// Canonical Zcash follow-up-claim observation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FollowupClaimObservationV1 {
    /// The agreement-derived transaction identity is stably absent.
    Absent,
    /// A candidate exists but is not stable enough to project.
    Unstable,
    /// Stable evidence for the exact follow-up transaction.
    Confirmed(FollowupClaimEvidenceV1),
}

impl ClaimStepV1 {
    /// Derives the only claim step owned by a fixed agreement role.
    #[must_use]
    pub fn for_local(agreement: &ZecAgreementV1, local: Participant) -> Self {
        if local == agreement.lez_claimant() {
            Self::RevealingLez
        } else {
            Self::FollowupZcash
        }
    }

    const fn claimant(self, agreement: &ZecAgreementV1) -> Participant {
        match self {
            Self::RevealingLez => agreement.lez_claimant(),
            Self::FollowupZcash => agreement.lez_claimant().other(),
        }
    }

    const fn expected_phase(self) -> Phase {
        match self {
            Self::RevealingLez => Phase::BothLegsLocked,
            Self::FollowupZcash => Phase::ClaimEvidenceAvailable,
        }
    }

    const fn observed_leg_funder(self, agreement: &ZecAgreementV1) -> Participant {
        match self {
            Self::RevealingLez => agreement.lez_depositor(),
            Self::FollowupZcash => agreement.lez_claimant(),
        }
    }
}

/// Exact claim bytes held only long enough to enter protected persistence.
#[derive(Eq, PartialEq)]
pub struct PreparedClaimSubmissionV1 {
    step: ClaimStepV1,
    expected_submission_id: [u8; 32],
    exact_submission: Vec<u8>,
}

impl Drop for PreparedClaimSubmissionV1 {
    fn drop(&mut self) {
        self.exact_submission.zeroize();
    }
}

impl PreparedClaimSubmissionV1 {
    /// Validates a nonempty bounded claim transaction and its expected identity.
    ///
    /// # Errors
    ///
    /// Rejects empty or oversized transaction bytes and an empty expected identity.
    pub fn new(
        step: ClaimStepV1,
        expected_submission_id: [u8; 32],
        exact_submission: Vec<u8>,
    ) -> Result<Self, ClaimError> {
        if exact_submission.is_empty() {
            return Err(ClaimError::EmptySubmission(step));
        }
        if exact_submission.len() > MAX_CLAIM_SUBMISSION_BYTES {
            return Err(ClaimError::OversizedSubmission {
                step,
                actual: exact_submission.len(),
                maximum: MAX_CLAIM_SUBMISSION_BYTES,
            });
        }
        if expected_submission_id == [0; 32] {
            return Err(ClaimError::EmptyExpectedIdentity(step));
        }
        Ok(Self {
            step,
            expected_submission_id,
            exact_submission,
        })
    }

    /// Chain-ordered claim step represented by these bytes.
    #[must_use]
    pub const fn step(&self) -> ClaimStepV1 {
        self.step
    }

    /// Chain-derived identity expected when the transaction is observed.
    #[must_use]
    pub const fn expected_submission_id(&self) -> &[u8; 32] {
        &self.expected_submission_id
    }

    /// Exact bytes to encrypt before durable persistence or submit after decryption.
    #[must_use]
    pub fn exact_submission(&self) -> &[u8] {
        &self.exact_submission
    }
}

impl std::fmt::Debug for PreparedClaimSubmissionV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedClaimSubmissionV1")
            .field("step", &self.step)
            .field("expected_submission_id", &"[REDACTED]")
            .field("exact_submission", &"[REDACTED]")
            .finish()
    }
}

/// Secret-free binding between an active swap and one protected claim payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaimIntentV1 {
    swap_id: SwapId,
    agreement_commitment: [u8; 32],
    local_participant: Participant,
    staged_revision: u64,
    step: ClaimStepV1,
    expected_submission_id: [u8; 32],
    protected_payload_fingerprint: [u8; 32],
}

impl ClaimIntentV1 {
    /// Builds the secret-free durable binding for an already protected exact submission.
    ///
    /// The supplied fingerprint must come from the authenticated protected envelope.
    ///
    /// # Errors
    ///
    /// Rejects the wrong role, step, phase, agreement context, or an empty fingerprint.
    pub fn from_active(
        agreement: &ZecAgreementV1,
        coordinator: &SwapCoordinator,
        local_participant: Participant,
        staged_revision: u64,
        prepared: &PreparedClaimSubmissionV1,
        protected_payload_fingerprint: [u8; 32],
    ) -> Result<Self, ClaimError> {
        validate_active_context(agreement, coordinator, prepared.step, local_participant)?;
        if protected_payload_fingerprint == [0; 32] {
            return Err(ClaimError::EmptyProtectedPayloadFingerprint);
        }
        Ok(Self {
            swap_id: agreement.coordinator().id().clone(),
            agreement_commitment: *agreement.agreement_commitment(),
            local_participant,
            staged_revision,
            step: prepared.step,
            expected_submission_id: prepared.expected_submission_id,
            protected_payload_fingerprint,
        })
    }

    pub(crate) fn from_protected_binding(
        agreement: &ZecAgreementV1,
        coordinator: &SwapCoordinator,
        local_participant: Participant,
        staged_revision: u64,
        step: ClaimStepV1,
        expected_submission_id: [u8; 32],
        protected_payload_fingerprint: [u8; 32],
    ) -> Result<Self, ClaimError> {
        validate_active_context(agreement, coordinator, step, local_participant)?;
        if expected_submission_id == [0; 32] {
            return Err(ClaimError::EmptyExpectedIdentity(step));
        }
        if protected_payload_fingerprint == [0; 32] {
            return Err(ClaimError::EmptyProtectedPayloadFingerprint);
        }
        Ok(Self {
            swap_id: agreement.coordinator().id().clone(),
            agreement_commitment: *agreement.agreement_commitment(),
            local_participant,
            staged_revision,
            step,
            expected_submission_id,
            protected_payload_fingerprint,
        })
    }

    /// Application swap identity derived from the signed agreement.
    #[must_use]
    pub const fn swap_id(&self) -> &SwapId {
        &self.swap_id
    }

    /// Commitment to the exact executable agreement.
    #[must_use]
    pub const fn agreement_commitment(&self) -> &[u8; 32] {
        &self.agreement_commitment
    }

    /// Fixed role that owns this claim effect.
    #[must_use]
    pub const fn local_participant(&self) -> Participant {
        self.local_participant
    }

    /// Aggregate revision at which the claim effect became durable.
    #[must_use]
    pub const fn staged_revision(&self) -> u64 {
        self.staged_revision
    }

    /// Chain-ordered claim step.
    #[must_use]
    pub const fn step(&self) -> ClaimStepV1 {
        self.step
    }

    /// Expected identity of the protected exact transaction.
    #[must_use]
    pub const fn expected_submission_id(&self) -> &[u8; 32] {
        &self.expected_submission_id
    }

    /// SHA-256 fingerprint binding this intent to its protected payload.
    #[must_use]
    pub const fn protected_payload_fingerprint(&self) -> &[u8; 32] {
        &self.protected_payload_fingerprint
    }

    /// Verifies a protected envelope's own context-bound fingerprint.
    ///
    /// # Errors
    ///
    /// Rejects an empty or substituted envelope fingerprint.
    pub fn validate_protected_payload_fingerprint(
        &self,
        actual: &[u8; 32],
    ) -> Result<(), ClaimError> {
        if actual
            .ct_eq(&self.protected_payload_fingerprint)
            .unwrap_u8()
            == 1
        {
            Ok(())
        } else {
            Err(ClaimError::ProtectedPayloadMismatch)
        }
    }

    pub(crate) fn validate_for_active(
        &self,
        agreement: &ZecAgreementV1,
        coordinator: &SwapCoordinator,
        current_revision: u64,
    ) -> Result<(), ClaimError> {
        if self.staged_revision > current_revision {
            return Err(ClaimError::StagedRevisionAhead {
                staged: self.staged_revision,
                current: current_revision,
            });
        }
        validate_active_context(agreement, coordinator, self.step, self.local_participant)?;
        if self.swap_id != *agreement.coordinator().id()
            || self.agreement_commitment != *agreement.agreement_commitment()
        {
            return Err(ClaimError::ContextMismatch);
        }
        Ok(())
    }
}

/// Canonical revealing-claim evidence carrying transient secret material.
pub struct RevealingClaimEvidenceV1 {
    observed_submission_id: [u8; 32],
    transaction_id: Box<str>,
    confirmations: u32,
    preimage: ClaimPreimage,
    canonical_lez_snapshot: Option<Box<CanonicalLezClaimSnapshotRecordV1>>,
}

impl RevealingClaimEvidenceV1 {
    /// Reconstructs a pre-v2 recovery record that predates primitive LEZ snapshots.
    ///
    /// This compatibility boundary is deliberately crate-private: live adapters cannot
    /// create legacy opaque evidence, and every newly persisted transition carries a
    /// canonical primitive snapshot.
    pub(crate) fn from_legacy_recovery_parts(
        agreement: &ZecAgreementV1,
        observed_submission_id: [u8; 32],
        transaction_id: impl Into<Box<str>>,
        confirmations: u32,
        preimage: ClaimPreimage,
    ) -> Result<Self, ClaimError> {
        if observed_submission_id == [0; 32] {
            return Err(ClaimError::EmptyExpectedIdentity(ClaimStepV1::RevealingLez));
        }
        let transaction_id = transaction_id.into();
        ChainProof::new(transaction_id.clone(), confirmations).map_err(ClaimError::Core)?;
        validate_preimage(agreement, &preimage)?;
        require_confirmations(agreement, ClaimStepV1::RevealingLez, confirmations)?;
        Ok(Self {
            observed_submission_id,
            transaction_id,
            confirmations,
            preimage,
            canonical_lez_snapshot: None,
        })
    }

    pub(crate) fn from_validated_lez_snapshot_parts(
        agreement: &ZecAgreementV1,
        observed_submission_id: [u8; 32],
        transaction_id: impl Into<Box<str>>,
        confirmations: u32,
        preimage: ClaimPreimage,
        canonical_lez_snapshot: CanonicalLezClaimSnapshotRecordV1,
    ) -> Result<Self, ClaimError> {
        let mut evidence = Self::from_legacy_recovery_parts(
            agreement,
            observed_submission_id,
            transaction_id,
            confirmations,
            preimage,
        )?;
        evidence.canonical_lez_snapshot = Some(Box::new(canonical_lez_snapshot));
        Ok(evidence)
    }

    /// Canonical adapter-derived identity of the observed transaction.
    #[must_use]
    pub const fn observed_submission_id(&self) -> &[u8; 32] {
        &self.observed_submission_id
    }

    /// Canonical chain transaction identifier.
    #[must_use]
    pub fn transaction_id(&self) -> &str {
        &self.transaction_id
    }

    /// Canonical confirmation depth.
    #[must_use]
    pub const fn confirmations(&self) -> u32 {
        self.confirmations
    }

    /// Transient preimage used only for protected storage or core application.
    #[must_use]
    pub const fn preimage(&self) -> &ClaimPreimage {
        &self.preimage
    }

    pub(crate) fn canonical_lez_snapshot(&self) -> Option<&CanonicalLezClaimSnapshotRecordV1> {
        self.canonical_lez_snapshot.as_deref()
    }
}

impl std::fmt::Debug for RevealingClaimEvidenceV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RevealingClaimEvidenceV1")
            .field("observed_submission_id", &"[REDACTED]")
            .field("transaction_id", &self.transaction_id)
            .field("confirmations", &self.confirmations)
            .field("preimage", &"[REDACTED]")
            .field(
                "canonical_lez_snapshot",
                &self.canonical_lez_snapshot.is_some(),
            )
            .finish()
    }
}

/// Canonical Zcash follow-up claim evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FollowupClaimEvidenceV1 {
    observed_submission_id: [u8; 32],
    transaction_id: Box<str>,
    confirmations: u32,
}

impl FollowupClaimEvidenceV1 {
    /// Validates canonical identity and the Zcash leg's signed depth policy.
    ///
    /// # Errors
    ///
    /// Rejects malformed identities or insufficient confirmation depth.
    pub fn new(
        agreement: &ZecAgreementV1,
        observed_submission_id: [u8; 32],
        transaction_id: impl Into<Box<str>>,
        confirmations: u32,
    ) -> Result<Self, ClaimError> {
        if observed_submission_id == [0; 32] {
            return Err(ClaimError::EmptyExpectedIdentity(
                ClaimStepV1::FollowupZcash,
            ));
        }
        let transaction_id = transaction_id.into();
        ChainProof::new(transaction_id.clone(), confirmations).map_err(ClaimError::Core)?;
        require_confirmations(agreement, ClaimStepV1::FollowupZcash, confirmations)?;
        Ok(Self {
            observed_submission_id,
            transaction_id,
            confirmations,
        })
    }

    /// Canonical adapter-derived identity of the observed transaction.
    #[must_use]
    pub const fn observed_submission_id(&self) -> &[u8; 32] {
        &self.observed_submission_id
    }

    /// Canonical Zcash transaction identifier.
    #[must_use]
    pub fn transaction_id(&self) -> &str {
        &self.transaction_id
    }

    /// Canonical confirmation depth.
    #[must_use]
    pub const fn confirmations(&self) -> u32 {
        self.confirmations
    }
}

/// Revealing LEZ claim committed at an exact aggregate revision.
pub struct RevealingClaimTransitionV1 {
    schema_version: u16,
    swap_id: SwapId,
    agreement_commitment: [u8; 32],
    local_participant: Participant,
    predecessor_revision: u64,
    intent_staged_revision: u64,
    evidence: RevealingClaimEvidenceV1,
}

impl RevealingClaimTransitionV1 {
    pub(crate) fn from_active(
        agreement: &ZecAgreementV1,
        coordinator: &SwapCoordinator,
        intent: &ClaimIntentV1,
        predecessor_revision: u64,
        evidence: RevealingClaimEvidenceV1,
    ) -> Result<Self, ClaimError> {
        require_transition_context(
            agreement,
            coordinator,
            intent,
            predecessor_revision,
            ClaimStepV1::RevealingLez,
            evidence.observed_submission_id(),
        )?;
        Ok(Self {
            schema_version: 1,
            swap_id: agreement.coordinator().id().clone(),
            agreement_commitment: *agreement.agreement_commitment(),
            local_participant: agreement.lez_claimant(),
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

    /// Agreement-derived swap identity.
    #[must_use]
    pub const fn swap_id(&self) -> &SwapId {
        &self.swap_id
    }

    /// Exact predecessor revision.
    #[must_use]
    pub const fn predecessor_revision(&self) -> u64 {
        self.predecessor_revision
    }

    /// Revision owning the retained intent.
    #[must_use]
    pub const fn intent_staged_revision(&self) -> u64 {
        self.intent_staged_revision
    }

    /// Canonical evidence including transient decrypted material.
    #[must_use]
    pub const fn evidence(&self) -> &RevealingClaimEvidenceV1 {
        &self.evidence
    }

    pub(crate) const fn agreement_commitment(&self) -> &[u8; 32] {
        &self.agreement_commitment
    }

    pub(crate) const fn local_participant(&self) -> Participant {
        self.local_participant
    }

    /// Revalidates and applies the LEZ revealing claim to an exact aggregate head.
    ///
    /// # Errors
    ///
    /// Rejects substituted context, revision, identity, depth, preimage, or phase.
    pub fn apply_to(
        &self,
        agreement: &ZecAgreementV1,
        coordinator: &SwapCoordinator,
        revision: u64,
    ) -> Result<SwapCoordinator, ClaimError> {
        validate_transition_header(
            &TransitionHeader {
                schema_version: self.schema_version,
                swap_id: &self.swap_id,
                agreement_commitment: &self.agreement_commitment,
                local: self.local_participant,
                predecessor_revision: self.predecessor_revision,
                intent_staged_revision: self.intent_staged_revision,
            },
            agreement,
            coordinator,
            revision,
            ClaimStepV1::RevealingLez,
        )?;
        validate_preimage(agreement, self.evidence.preimage())?;
        require_confirmations(
            agreement,
            ClaimStepV1::RevealingLez,
            self.evidence.confirmations(),
        )?;
        let mut next = coordinator.clone();
        next.observe_revealing_claim(
            agreement.lez_claimant(),
            ChainProof::new(
                self.evidence.transaction_id().to_owned(),
                self.evidence.confirmations(),
            )
            .map_err(ClaimError::Core)?,
            ClaimEvidence::new(*self.evidence.preimage().expose_secret()),
        )
        .map_err(ClaimError::Core)?;
        Ok(next)
    }
}

impl std::fmt::Debug for RevealingClaimTransitionV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RevealingClaimTransitionV1")
            .field("schema_version", &self.schema_version)
            .field("swap_id", &self.swap_id)
            .field("agreement_commitment", &"[REDACTED]")
            .field("local_participant", &self.local_participant)
            .field("predecessor_revision", &self.predecessor_revision)
            .field("intent_staged_revision", &self.intent_staged_revision)
            .field("evidence", &self.evidence)
            .finish()
    }
}

/// Counterparty-local observation of the canonical LEZ revealing claim.
///
/// This transition has no local submission intent. Its opaque observed identity and canonical
/// transaction ID are both supplied by the chain adapter; unlike the owner path, the domain has
/// no exact local transaction plan against which it can independently compare that identity.
pub struct ObservedRevealingClaimTransitionV1 {
    schema_version: u16,
    swap_id: SwapId,
    agreement_commitment: [u8; 32],
    local_participant: Participant,
    predecessor_revision: u64,
    evidence: RevealingClaimEvidenceV1,
}

impl ObservedRevealingClaimTransitionV1 {
    pub(crate) fn from_active(
        agreement: &ZecAgreementV1,
        coordinator: &SwapCoordinator,
        local_participant: Participant,
        predecessor_revision: u64,
        evidence: RevealingClaimEvidenceV1,
    ) -> Result<Self, ClaimError> {
        validate_observer_context(
            agreement,
            coordinator,
            local_participant,
            ClaimStepV1::RevealingLez,
        )?;
        Ok(Self {
            schema_version: 1,
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

    /// Agreement-derived swap identity.
    #[must_use]
    pub const fn swap_id(&self) -> &SwapId {
        &self.swap_id
    }

    /// Exact local predecessor revision.
    #[must_use]
    pub const fn predecessor_revision(&self) -> u64 {
        self.predecessor_revision
    }

    /// Canonical evidence including transient extracted material.
    #[must_use]
    pub const fn evidence(&self) -> &RevealingClaimEvidenceV1 {
        &self.evidence
    }

    pub(crate) const fn agreement_commitment(&self) -> &[u8; 32] {
        &self.agreement_commitment
    }

    pub(crate) const fn local_participant(&self) -> Participant {
        self.local_participant
    }

    /// Revalidates and applies the observed LEZ effect using its fixed on-chain claimant.
    ///
    /// # Errors
    ///
    /// Rejects owner-local, substituted, stale, shallow, wrong-secret, or wrong-phase evidence.
    pub fn apply_to(
        &self,
        agreement: &ZecAgreementV1,
        coordinator: &SwapCoordinator,
        revision: u64,
    ) -> Result<SwapCoordinator, ClaimError> {
        validate_observer_header(
            &ObserverHeader {
                schema_version: self.schema_version,
                swap_id: &self.swap_id,
                agreement_commitment: &self.agreement_commitment,
                local: self.local_participant,
                predecessor_revision: self.predecessor_revision,
            },
            agreement,
            coordinator,
            revision,
            ClaimStepV1::RevealingLez,
        )?;
        validate_preimage(agreement, self.evidence.preimage())?;
        require_confirmations(
            agreement,
            ClaimStepV1::RevealingLez,
            self.evidence.confirmations(),
        )?;
        apply_revealing_claim(agreement, coordinator, &self.evidence)
    }
}

impl std::fmt::Debug for ObservedRevealingClaimTransitionV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ObservedRevealingClaimTransitionV1")
            .field("schema_version", &self.schema_version)
            .field("swap_id", &self.swap_id)
            .field("agreement_commitment", &"[REDACTED]")
            .field("local_participant", &self.local_participant)
            .field("predecessor_revision", &self.predecessor_revision)
            .field("evidence", &self.evidence)
            .finish()
    }
}

/// Follow-up Zcash claim committed at an exact aggregate revision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FollowupClaimTransitionV1 {
    schema_version: u16,
    swap_id: SwapId,
    agreement_commitment: [u8; 32],
    local_participant: Participant,
    predecessor_revision: u64,
    intent_staged_revision: u64,
    evidence: FollowupClaimEvidenceV1,
}

impl FollowupClaimTransitionV1 {
    pub(crate) fn from_active(
        agreement: &ZecAgreementV1,
        coordinator: &SwapCoordinator,
        intent: &ClaimIntentV1,
        predecessor_revision: u64,
        evidence: FollowupClaimEvidenceV1,
    ) -> Result<Self, ClaimError> {
        require_transition_context(
            agreement,
            coordinator,
            intent,
            predecessor_revision,
            ClaimStepV1::FollowupZcash,
            evidence.observed_submission_id(),
        )?;
        Ok(Self {
            schema_version: 1,
            swap_id: agreement.coordinator().id().clone(),
            agreement_commitment: *agreement.agreement_commitment(),
            local_participant: agreement.lez_claimant().other(),
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

    /// Agreement-derived swap identity.
    #[must_use]
    pub const fn swap_id(&self) -> &SwapId {
        &self.swap_id
    }

    /// Exact predecessor revision.
    #[must_use]
    pub const fn predecessor_revision(&self) -> u64 {
        self.predecessor_revision
    }

    /// Revision owning the retained intent.
    #[must_use]
    pub const fn intent_staged_revision(&self) -> u64 {
        self.intent_staged_revision
    }

    /// Canonical Zcash follow-up evidence.
    #[must_use]
    pub const fn evidence(&self) -> &FollowupClaimEvidenceV1 {
        &self.evidence
    }

    pub(crate) const fn agreement_commitment(&self) -> &[u8; 32] {
        &self.agreement_commitment
    }

    pub(crate) const fn local_participant(&self) -> Participant {
        self.local_participant
    }

    /// Revalidates and applies the Zcash follow-up claim to an exact aggregate head.
    ///
    /// # Errors
    ///
    /// Rejects substituted context, revision, identity, depth, or phase.
    pub fn apply_to(
        &self,
        agreement: &ZecAgreementV1,
        coordinator: &SwapCoordinator,
        revision: u64,
    ) -> Result<SwapCoordinator, ClaimError> {
        validate_transition_header(
            &TransitionHeader {
                schema_version: self.schema_version,
                swap_id: &self.swap_id,
                agreement_commitment: &self.agreement_commitment,
                local: self.local_participant,
                predecessor_revision: self.predecessor_revision,
                intent_staged_revision: self.intent_staged_revision,
            },
            agreement,
            coordinator,
            revision,
            ClaimStepV1::FollowupZcash,
        )?;
        require_confirmations(
            agreement,
            ClaimStepV1::FollowupZcash,
            self.evidence.confirmations(),
        )?;
        let mut next = coordinator.clone();
        next.observe_followup_claim(
            agreement.lez_claimant().other(),
            ChainProof::new(
                self.evidence.transaction_id().to_owned(),
                self.evidence.confirmations(),
            )
            .map_err(ClaimError::Core)?,
        )
        .map_err(ClaimError::Core)?;
        Ok(next)
    }
}

/// Counterparty-local observation of the canonical Zcash follow-up claim.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservedFollowupClaimTransitionV1 {
    schema_version: u16,
    swap_id: SwapId,
    agreement_commitment: [u8; 32],
    local_participant: Participant,
    predecessor_revision: u64,
    evidence: FollowupClaimEvidenceV1,
}

impl ObservedFollowupClaimTransitionV1 {
    pub(crate) fn from_active(
        agreement: &ZecAgreementV1,
        coordinator: &SwapCoordinator,
        local_participant: Participant,
        predecessor_revision: u64,
        evidence: FollowupClaimEvidenceV1,
    ) -> Result<Self, ClaimError> {
        validate_observer_context(
            agreement,
            coordinator,
            local_participant,
            ClaimStepV1::FollowupZcash,
        )?;
        Ok(Self {
            schema_version: 1,
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

    /// Agreement-derived swap identity.
    #[must_use]
    pub const fn swap_id(&self) -> &SwapId {
        &self.swap_id
    }

    /// Exact local predecessor revision.
    #[must_use]
    pub const fn predecessor_revision(&self) -> u64 {
        self.predecessor_revision
    }

    /// Canonical Zcash evidence.
    #[must_use]
    pub const fn evidence(&self) -> &FollowupClaimEvidenceV1 {
        &self.evidence
    }

    pub(crate) const fn agreement_commitment(&self) -> &[u8; 32] {
        &self.agreement_commitment
    }

    pub(crate) const fn local_participant(&self) -> Participant {
        self.local_participant
    }

    /// Revalidates and applies the observed Zcash effect using its fixed on-chain claimant.
    ///
    /// # Errors
    ///
    /// Rejects owner-local, substituted, stale, shallow, or wrong-phase evidence.
    pub fn apply_to(
        &self,
        agreement: &ZecAgreementV1,
        coordinator: &SwapCoordinator,
        revision: u64,
    ) -> Result<SwapCoordinator, ClaimError> {
        validate_observer_header(
            &ObserverHeader {
                schema_version: self.schema_version,
                swap_id: &self.swap_id,
                agreement_commitment: &self.agreement_commitment,
                local: self.local_participant,
                predecessor_revision: self.predecessor_revision,
            },
            agreement,
            coordinator,
            revision,
            ClaimStepV1::FollowupZcash,
        )?;
        require_confirmations(
            agreement,
            ClaimStepV1::FollowupZcash,
            self.evidence.confirmations(),
        )?;
        apply_followup_claim(agreement, coordinator, &self.evidence)
    }
}

/// Invalid claim submission, evidence, or durable transition.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ClaimError {
    /// Exact transaction bytes are empty.
    #[error("{0:?} claim submission is empty")]
    EmptySubmission(ClaimStepV1),
    /// Exact transaction bytes exceed the durable protected-payload bound.
    #[error("{step:?} claim submission has {actual} bytes; maximum is {maximum}")]
    OversizedSubmission {
        /// Claim step.
        step: ClaimStepV1,
        /// Supplied byte length.
        actual: usize,
        /// Accepted upper bound.
        maximum: usize,
    },
    /// Expected transaction identity is all zeroes.
    #[error("{0:?} claim expected identity is empty")]
    EmptyExpectedIdentity(ClaimStepV1),
    /// Protected envelope fingerprint is all zeroes.
    #[error("protected claim payload fingerprint is empty")]
    EmptyProtectedPayloadFingerprint,
    /// Fixed local role cannot own this step.
    #[error("{step:?} requires {expected:?}; local participant is {actual:?}")]
    WrongRole {
        /// Claim step.
        step: ClaimStepV1,
        /// Agreement-derived claimant.
        expected: Participant,
        /// Supplied fixed role.
        actual: Participant,
    },
    /// Claim cannot be staged or applied in the active phase.
    #[error("{step:?} requires {expected:?}; active phase is {actual:?}")]
    WrongPhase {
        /// Claim step.
        step: ClaimStepV1,
        /// Required phase.
        expected: Phase,
        /// Active phase.
        actual: Phase,
    },
    /// Staged intent revision is ahead of the aggregate.
    #[error("claim intent was staged at revision {staged}; active revision is {current}")]
    StagedRevisionAhead {
        /// Intent revision.
        staged: u64,
        /// Active aggregate revision.
        current: u64,
    },
    /// Durable context, step, or identity differs from the active agreement.
    #[error("durable claim context mismatch")]
    ContextMismatch,
    /// Decrypted exact bytes do not match the durable protected-payload fingerprint.
    #[error("decrypted claim payload does not match its durable fingerprint")]
    ProtectedPayloadMismatch,
    /// Canonical evidence does not name the exact protected submission.
    #[error("claim evidence identity does not match durable intent")]
    SubmissionIdentityMismatch,
    /// Revealed preimage does not satisfy the agreement digest.
    #[error("revealing claim preimage does not match agreement digest")]
    SecretDigestMismatch,
    /// Evidence depth is below the policy of the participant who funded that chain leg.
    #[error("{step:?} claim has {actual} confirmations; requires {required}")]
    InsufficientConfirmations {
        /// Claim step.
        step: ClaimStepV1,
        /// Signed threshold.
        required: u32,
        /// Observed canonical depth.
        actual: u32,
    },
    /// Core rejected an identity or lifecycle transition.
    #[error(transparent)]
    Core(lez_swap_core::Error),
}

fn validate_active_context(
    agreement: &ZecAgreementV1,
    coordinator: &SwapCoordinator,
    step: ClaimStepV1,
    local: Participant,
) -> Result<(), ClaimError> {
    if coordinator.id() != agreement.coordinator().id() {
        return Err(ClaimError::ContextMismatch);
    }
    let expected_role = step.claimant(agreement);
    if local != expected_role {
        return Err(ClaimError::WrongRole {
            step,
            expected: expected_role,
            actual: local,
        });
    }
    if coordinator.phase() != step.expected_phase() {
        return Err(ClaimError::WrongPhase {
            step,
            expected: step.expected_phase(),
            actual: coordinator.phase(),
        });
    }
    Ok(())
}

fn require_transition_context(
    agreement: &ZecAgreementV1,
    coordinator: &SwapCoordinator,
    intent: &ClaimIntentV1,
    predecessor_revision: u64,
    step: ClaimStepV1,
    evidence_identity: &[u8; 32],
) -> Result<(), ClaimError> {
    intent.validate_for_active(agreement, coordinator, predecessor_revision)?;
    if intent.step() != step {
        return Err(ClaimError::ContextMismatch);
    }
    if intent.expected_submission_id() != evidence_identity {
        return Err(ClaimError::SubmissionIdentityMismatch);
    }
    Ok(())
}

struct TransitionHeader<'a> {
    schema_version: u16,
    swap_id: &'a SwapId,
    agreement_commitment: &'a [u8; 32],
    local: Participant,
    predecessor_revision: u64,
    intent_staged_revision: u64,
}

fn validate_transition_header(
    header: &TransitionHeader<'_>,
    agreement: &ZecAgreementV1,
    coordinator: &SwapCoordinator,
    revision: u64,
    step: ClaimStepV1,
) -> Result<(), ClaimError> {
    if header.schema_version != 1
        || header.swap_id != agreement.coordinator().id()
        || header.agreement_commitment != agreement.agreement_commitment()
        || header.local != step.claimant(agreement)
        || header.predecessor_revision != revision
        || header.intent_staged_revision > header.predecessor_revision
        || coordinator.id() != header.swap_id
    {
        return Err(ClaimError::ContextMismatch);
    }
    if coordinator.phase() != step.expected_phase() {
        return Err(ClaimError::WrongPhase {
            step,
            expected: step.expected_phase(),
            actual: coordinator.phase(),
        });
    }
    Ok(())
}

struct ObserverHeader<'a> {
    schema_version: u16,
    swap_id: &'a SwapId,
    agreement_commitment: &'a [u8; 32],
    local: Participant,
    predecessor_revision: u64,
}

fn validate_observer_context(
    agreement: &ZecAgreementV1,
    coordinator: &SwapCoordinator,
    local: Participant,
    step: ClaimStepV1,
) -> Result<(), ClaimError> {
    if coordinator.id() != agreement.coordinator().id() {
        return Err(ClaimError::ContextMismatch);
    }
    let expected = step.claimant(agreement).other();
    if local != expected {
        return Err(ClaimError::WrongRole {
            step,
            expected,
            actual: local,
        });
    }
    if coordinator.phase() != step.expected_phase() {
        return Err(ClaimError::WrongPhase {
            step,
            expected: step.expected_phase(),
            actual: coordinator.phase(),
        });
    }
    Ok(())
}

fn validate_observer_header(
    header: &ObserverHeader<'_>,
    agreement: &ZecAgreementV1,
    coordinator: &SwapCoordinator,
    revision: u64,
    step: ClaimStepV1,
) -> Result<(), ClaimError> {
    validate_observer_context(agreement, coordinator, header.local, step)?;
    if header.schema_version != 1
        || header.swap_id != agreement.coordinator().id()
        || header.agreement_commitment != agreement.agreement_commitment()
        || header.predecessor_revision != revision
        || coordinator.id() != header.swap_id
    {
        return Err(ClaimError::ContextMismatch);
    }
    Ok(())
}

fn apply_revealing_claim(
    agreement: &ZecAgreementV1,
    coordinator: &SwapCoordinator,
    evidence: &RevealingClaimEvidenceV1,
) -> Result<SwapCoordinator, ClaimError> {
    let mut next = coordinator.clone();
    next.observe_revealing_claim(
        agreement.lez_claimant(),
        ChainProof::new(
            evidence.transaction_id().to_owned(),
            evidence.confirmations(),
        )
        .map_err(ClaimError::Core)?,
        ClaimEvidence::new(*evidence.preimage().expose_secret()),
    )
    .map_err(ClaimError::Core)?;
    Ok(next)
}

fn apply_followup_claim(
    agreement: &ZecAgreementV1,
    coordinator: &SwapCoordinator,
    evidence: &FollowupClaimEvidenceV1,
) -> Result<SwapCoordinator, ClaimError> {
    let mut next = coordinator.clone();
    next.observe_followup_claim(
        agreement.lez_claimant().other(),
        ChainProof::new(
            evidence.transaction_id().to_owned(),
            evidence.confirmations(),
        )
        .map_err(ClaimError::Core)?,
    )
    .map_err(ClaimError::Core)?;
    Ok(next)
}

pub(crate) fn validate_preimage(
    agreement: &ZecAgreementV1,
    preimage: &ClaimPreimage,
) -> Result<(), ClaimError> {
    let actual: [u8; 32] = Sha256::digest(preimage.expose_secret()).into();
    if actual.ct_eq(agreement.secret_digest()).unwrap_u8() == 1 {
        Ok(())
    } else {
        Err(ClaimError::SecretDigestMismatch)
    }
}

fn require_confirmations(
    agreement: &ZecAgreementV1,
    step: ClaimStepV1,
    actual: u32,
) -> Result<(), ClaimError> {
    let required = agreement
        .coordinator()
        .required_confirmations(step.observed_leg_funder(agreement));
    if actual < required {
        Err(ClaimError::InsufficientConfirmations {
            step,
            required,
            actual,
        })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepared_claim_rejects_empty_and_oversized_bytes() {
        assert_eq!(
            PreparedClaimSubmissionV1::new(ClaimStepV1::RevealingLez, [7; 32], Vec::new()),
            Err(ClaimError::EmptySubmission(ClaimStepV1::RevealingLez))
        );
        assert!(matches!(
            PreparedClaimSubmissionV1::new(
                ClaimStepV1::FollowupZcash,
                [7; 32],
                vec![0; MAX_CLAIM_SUBMISSION_BYTES + 1],
            ),
            Err(ClaimError::OversizedSubmission { .. })
        ));
    }

    #[test]
    fn prepared_claim_debug_redacts_identity_and_exact_bytes() {
        let prepared = PreparedClaimSubmissionV1::new(
            ClaimStepV1::RevealingLez,
            [7; 32],
            b"secret-bearing-transaction".to_vec(),
        )
        .expect("bounded transaction");
        let debug = format!("{prepared:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("secret-bearing-transaction"));
    }

    #[test]
    fn protected_payload_fingerprint_uses_constant_time_validation() {
        let protected_fingerprint = [8; 32];
        let intent = ClaimIntentV1 {
            swap_id: SwapId::new("swap-1").expect("valid swap ID"),
            agreement_commitment: [3; 32],
            local_participant: Participant::Maker,
            staged_revision: 7,
            step: ClaimStepV1::RevealingLez,
            expected_submission_id: [4; 32],
            protected_payload_fingerprint: protected_fingerprint,
        };
        assert_eq!(
            intent.validate_protected_payload_fingerprint(&protected_fingerprint),
            Ok(())
        );
        assert_eq!(
            intent.validate_protected_payload_fingerprint(&[9; 32]),
            Err(ClaimError::ProtectedPayloadMismatch)
        );
    }
}
